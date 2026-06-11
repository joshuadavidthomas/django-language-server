use std::collections::BTreeSet;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::DjangoSettings;
use djls_project::SettingsSource;
use djls_project::SettingsSourceResolver;
use djls_project::SettingsStarImport;
use djls_project::StaticKnowledge;
use djls_project::TemplateDirPath;
use djls_project::extract_settings;
use djls_source::File;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use crate::project::db::Db as ProjectDb;
use crate::project::input::Project;
use crate::project::names::LibraryName;
use crate::project::names::PyModuleName;
use crate::project::names::TemplateSymbolName;
use crate::project::resolve::module_file_in_search_path;
use crate::project::symbols::LibraryOrigin;
use crate::project::symbols::SymbolDefinition;
use crate::project::symbols::TemplateLibraries;
use crate::project::symbols::TemplateLibrary;
use crate::project::symbols::TemplateSymbol;
use crate::project::symbols::TemplateSymbolKind;
use crate::python::SymbolKind;
use crate::python::collect_registrations_from_body;
use crate::python::parse_python_module;

const DEFAULT_TEMPLATE_BUILTINS: &[&str] = &[
    "django.template.defaulttags",
    "django.template.defaultfilters",
    "django.template.loader_tags",
];

#[salsa::tracked]
pub(crate) fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    let django_settings_module = project.django_settings_module(db).as_deref()?;
    SalsaSettingsResolver { db, project }.module_file(django_settings_module)
}

#[salsa::tracked(returns(ref))]
pub(crate) fn django_settings(db: &dyn ProjectDb, project: Project) -> DjangoSettings {
    let Some(file) = settings_module_file(db, project) else {
        return DjangoSettings::default();
    };

    let source = file.source(db);
    let path = file.path(db);
    let mut resolver = SalsaSettingsResolver { db, project };
    extract_settings(source.as_str(), path, &mut resolver)
}

pub(super) fn settings_source_files(db: &dyn ProjectDb, project: Project) -> Vec<File> {
    let Some(file) = settings_module_file(db, project) else {
        return Vec::new();
    };

    let resolver = SalsaSettingsResolver { db, project };
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    resolver.collect_settings_source_files(file.path(db), &mut seen, &mut files);
    files
}

#[salsa::tracked(returns(ref))]
pub fn template_dirs(db: &dyn ProjectDb, project: Project) -> (Vec<Utf8PathBuf>, StaticKnowledge) {
    let resolver = SalsaSettingsResolver { db, project };
    project.touch_search_path_roots(db);

    let settings = django_settings(db, project);
    let mut dirs = Vec::new();
    let mut knowledge = settings.templates.knowledge;
    let backend_count = settings.templates.backends.len();

    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend(backend_count))
    {
        knowledge = knowledge.weakened_by(backend.knowledge);

        for dir in &backend.dirs {
            match dir {
                TemplateDirPath::Resolved(path) => dirs.push(path.clone()),
                TemplateDirPath::Unknown => knowledge = knowledge.demoted_to_partial(),
            }
        }

        if backend.app_dirs == Some(true) {
            knowledge = knowledge.weakened_by(settings.installed_apps.knowledge);
            for app in &settings.installed_apps.values {
                let Some(app_dir) = resolver.package_dir(installed_app_package_module(app)) else {
                    knowledge = knowledge.demoted_to_partial();
                    continue;
                };

                let templates_dir = app_dir.join("templates");
                if db.path_is_dir(&templates_dir) {
                    dirs.push(templates_dir);
                }
            }
        }
    }

    (dirs, knowledge)
}

#[salsa::tracked(returns(ref))]
pub fn template_libraries(db: &dyn ProjectDb, project: Project) -> TemplateLibraries {
    let resolver = SalsaSettingsResolver { db, project };
    project.touch_search_path_roots(db);

    if settings_module_file(db, project).is_none() {
        return TemplateLibraries::default();
    }

    let settings = django_settings(db, project);

    let mut libraries = TemplateLibraries {
        knowledge: match settings.installed_apps.knowledge {
            StaticKnowledge::Known => StaticKnowledge::Known,
            StaticKnowledge::Partial | StaticKnowledge::Unknown => StaticKnowledge::Partial,
        },
        ..TemplateLibraries::default()
    };

    if settings.templates.knowledge != StaticKnowledge::Known {
        libraries.knowledge = libraries.knowledge.demoted_to_partial();
    }

    let derived = templatetag_package_libraries(&resolver, "django");
    libraries.knowledge = libraries.knowledge.weakened_by(derived.knowledge);
    libraries.extend_loadable(derived.libraries);

    if settings.installed_apps.knowledge != StaticKnowledge::Unknown {
        for installed_app in &settings.installed_apps.values {
            let derived = templatetag_package_libraries(
                &resolver,
                installed_app_package_module(installed_app),
            );
            libraries.knowledge = libraries.knowledge.weakened_by(derived.knowledge);
            libraries.extend_loadable(derived.libraries);
        }
    }

    let backend_count = settings.templates.backends.len();
    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend(backend_count))
    {
        libraries.knowledge = libraries.knowledge.weakened_by(backend.knowledge);

        for (load_name, module_path) in &backend.libraries {
            let derived = configured_library(&resolver, load_name, module_path);
            libraries.knowledge = libraries.knowledge.weakened_by(derived.knowledge);
            if let Some(library) = derived.library {
                libraries.set_loadable(library);
            }
        }

        for module_path in DEFAULT_TEMPLATE_BUILTINS {
            let derived = builtin_library(&resolver, module_path);
            libraries.knowledge = libraries.knowledge.weakened_by(derived.knowledge);
            if let Some(library) = derived.library {
                libraries.push_builtin(library);
            }
        }
        for module_path in &backend.builtins {
            let derived = builtin_library(&resolver, module_path);
            libraries.knowledge = libraries.knowledge.weakened_by(derived.knowledge);
            if let Some(library) = derived.library {
                libraries.push_builtin(library);
            }
        }
    }

    libraries
}

struct DerivedTemplateLibraries {
    knowledge: StaticKnowledge,
    libraries: Vec<TemplateLibrary>,
}

impl Default for DerivedTemplateLibraries {
    fn default() -> Self {
        Self {
            knowledge: StaticKnowledge::Known,
            libraries: Vec::new(),
        }
    }
}

impl DerivedTemplateLibraries {
    fn demote_to_partial(&mut self) {
        self.knowledge = self.knowledge.demoted_to_partial();
    }
}

struct DerivedTemplateLibrary {
    knowledge: StaticKnowledge,
    library: Option<TemplateLibrary>,
}

impl DerivedTemplateLibrary {
    fn known(library: TemplateLibrary) -> Self {
        Self {
            knowledge: StaticKnowledge::Known,
            library: Some(library),
        }
    }

    fn partial(library: Option<TemplateLibrary>) -> Self {
        Self {
            knowledge: StaticKnowledge::Partial,
            library,
        }
    }
}

fn templatetag_package_libraries(
    resolver: &SalsaSettingsResolver<'_>,
    package_module: &str,
) -> DerivedTemplateLibraries {
    let mut derived = DerivedTemplateLibraries::default();

    if package_module.is_empty() {
        derived.demote_to_partial();
        return derived;
    }

    let Ok(app_module) = PyModuleName::parse(package_module) else {
        derived.demote_to_partial();
        return derived;
    };

    let Some(package_dir) = resolver.package_dir(package_module) else {
        derived.demote_to_partial();
        return derived;
    };

    let templatetags_dir = package_dir.join("templatetags");
    if !resolver
        .db
        .path_is_file(&templatetags_dir.join("__init__.py"))
    {
        return derived;
    }

    let entries = match resolver
        .db
        .walk_entries(&templatetags_dir, &WalkOptions::shallow())
    {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Failed to walk template tag package {templatetags_dir}: {err}");
            derived.demote_to_partial();
            return derived;
        }
    };

    for entry in entries {
        if entry.kind != WalkEntryKind::File || entry.path.extension() != Some("py") {
            continue;
        }

        let Some(stem) = entry.path.file_stem() else {
            continue;
        };
        if stem.starts_with('_') {
            continue;
        }

        let Ok(load_name) = LibraryName::parse(stem) else {
            derived.demote_to_partial();
            continue;
        };
        let module_path = format!("{package_module}.templatetags.{stem}");
        let Ok(module) = PyModuleName::parse(&module_path) else {
            derived.demote_to_partial();
            continue;
        };

        let file = resolver.db.get_or_create_file(&entry.path);
        let analysis = TemplateLibraryAnalysis::from_file(resolver.db, file);
        if !analysis.defines_library && analysis.symbols.is_empty() {
            continue;
        }

        let origin = LibraryOrigin {
            app: app_module.clone(),
            module: module.clone(),
            path: entry.path,
        };
        let mut library = TemplateLibrary::new_active(load_name, module, Some(origin));
        library.merge_symbols(analysis.symbols);
        derived.libraries.push(library);
    }

    derived
}

fn configured_library(
    resolver: &SalsaSettingsResolver<'_>,
    load_name: &str,
    module_path: &str,
) -> DerivedTemplateLibrary {
    let Ok(load_name) = LibraryName::parse(load_name) else {
        return DerivedTemplateLibrary::partial(None);
    };
    let Ok(module) = PyModuleName::parse(module_path) else {
        return DerivedTemplateLibrary::partial(None);
    };

    library_with_symbols(
        resolver,
        TemplateLibrary::new_active(load_name, module, None),
    )
}

fn builtin_library(
    resolver: &SalsaSettingsResolver<'_>,
    module_path: &str,
) -> DerivedTemplateLibrary {
    let Ok(module) = PyModuleName::parse(module_path) else {
        return DerivedTemplateLibrary::partial(None);
    };
    let Ok(name) = LibraryName::parse(module.as_str().split('.').next_back().unwrap_or("builtin"))
    else {
        return DerivedTemplateLibrary::partial(None);
    };

    library_with_symbols(resolver, TemplateLibrary::new_builtin(name, module))
}

fn library_with_symbols(
    resolver: &SalsaSettingsResolver<'_>,
    mut library: TemplateLibrary,
) -> DerivedTemplateLibrary {
    if let Some(file) = resolver.module_file(library.module().as_str()) {
        library.merge_symbols(TemplateLibraryAnalysis::from_file(resolver.db, file).symbols);
        DerivedTemplateLibrary::known(library)
    } else {
        DerivedTemplateLibrary::partial(Some(library))
    }
}

struct TemplateLibraryAnalysis {
    defines_library: bool,
    symbols: Vec<TemplateSymbol>,
}

impl TemplateLibraryAnalysis {
    fn from_file(db: &dyn ProjectDb, file: File) -> Self {
        let Some(parsed) = parse_python_module(db, file) else {
            return Self {
                defines_library: false,
                symbols: Vec::new(),
            };
        };

        let mut symbols = Vec::new();
        for registration in collect_registrations_from_body(parsed.body(db)) {
            let Ok(name) = TemplateSymbolName::parse(&registration.name) else {
                continue;
            };
            let kind = match registration.kind.symbol_kind() {
                SymbolKind::Tag => TemplateSymbolKind::Tag,
                SymbolKind::Filter => TemplateSymbolKind::Filter,
            };
            symbols.push(TemplateSymbol {
                kind,
                name,
                definition: SymbolDefinition::Exact {
                    file: file.path(db).to_path_buf(),
                },
                doc: None,
            });
        }

        Self {
            defines_library: parsed.body(db).iter().any(Self::stmt_defines_library),
            symbols,
        }
    }

    fn stmt_defines_library(stmt: &Stmt) -> bool {
        let Stmt::Assign(StmtAssign { targets, value, .. }) = stmt else {
            return false;
        };
        if !targets.iter().any(
            |target| matches!(target, Expr::Name(ExprName { id, .. }) if id.as_str() == "register"),
        ) {
            return false;
        }

        let Expr::Call(ExprCall { func, .. }) = value.as_ref() else {
            return false;
        };
        match func.as_ref() {
            Expr::Attribute(ExprAttribute { value, attr, .. }) => {
                attr.as_str() == "Library"
                    && matches!(value.as_ref(), Expr::Name(ExprName { id, .. }) if id.as_str() == "template")
            }
            Expr::Name(ExprName { id, .. }) => id.as_str() == "Library",
            _ => false,
        }
    }
}

struct SalsaSettingsResolver<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
}

impl SalsaSettingsResolver<'_> {
    fn collect_settings_source_files(
        &self,
        path: &Utf8Path,
        seen: &mut BTreeSet<Utf8PathBuf>,
        files: &mut Vec<File>,
    ) {
        if !seen.insert(path.to_path_buf()) {
            return;
        }

        files.push(self.db.get_or_create_file(path));

        let Ok(source) = self.db.read_file(path) else {
            return;
        };
        let Ok(parsed) = ruff_python_parser::parse_module(&source) else {
            return;
        };

        for stmt in parsed.into_syntax().body {
            let Stmt::ImportFrom(import) = stmt else {
                continue;
            };
            if !import.names.iter().any(|alias| alias.name.as_str() == "*") {
                continue;
            }

            let star_import = SettingsStarImport {
                level: import.level,
                module: import.module.map(|module| module.to_string()),
            };
            let Some(module_path) = self.resolve_star_import_module(path, &star_import) else {
                continue;
            };
            let Some(file) = self.module_file(&module_path) else {
                continue;
            };
            self.collect_settings_source_files(file.path(self.db), seen, files);
        }
    }

    fn module_file(&self, module_path: &str) -> Option<File> {
        self.project.touch_search_path_roots(self.db);

        for search_path in self.project.search_paths(self.db).iter() {
            let Some(path) =
                module_file_in_search_path(self.db.file_system(), module_path, search_path.path())
            else {
                continue;
            };
            return Some(self.db.get_or_create_file(&path));
        }

        None
    }

    fn package_dir(&self, package_module: &str) -> Option<Utf8PathBuf> {
        if package_module.is_empty() {
            return None;
        }

        let relative = package_module.replace('.', "/");
        for search_path in self.project.search_paths(self.db).iter() {
            let candidate = search_path.path().join(&relative);
            if self.db.path_is_dir(&candidate) {
                return Some(candidate);
            }
        }

        None
    }

    fn resolve_star_import_module(
        &self,
        base: &Utf8Path,
        import: &SettingsStarImport,
    ) -> Option<String> {
        if import.level == 0 {
            return import.module.clone();
        }

        let module_file = ModuleFileParts::from_path(self.db, self.project, base)?;
        let mut parts = module_file.parts;
        if !module_file.is_package_init {
            parts.pop()?;
        }

        for _ in 1..import.level {
            parts.pop()?;
        }

        if let Some(module) = &import.module {
            parts.extend(
                module
                    .split('.')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        }

        (!parts.is_empty()).then(|| parts.join("."))
    }
}

impl SettingsSourceResolver for SalsaSettingsResolver<'_> {
    fn resolve_star_import(
        &mut self,
        import: &SettingsStarImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource> {
        let module_path = self.resolve_star_import_module(importer, import)?;
        let file = self.module_file(&module_path)?;
        Some(SettingsSource {
            source: file.source(self.db).as_str().to_string(),
            path: file.path(self.db).to_path_buf(),
        })
    }
}

struct ModuleFileParts {
    parts: Vec<String>,
    is_package_init: bool,
}

impl ModuleFileParts {
    fn from_path(db: &dyn ProjectDb, project: Project, file_path: &Utf8Path) -> Option<Self> {
        let root = project
            .search_paths(db)
            .iter()
            .filter(|search_path| file_path.starts_with(search_path.path()))
            .max_by_key(|search_path| search_path.path().as_str().len())?
            .path();
        let relative = file_path.strip_prefix(root).ok()?;
        let mut parts: Vec<String> = relative
            .components()
            .map(|component| component.as_str().to_string())
            .collect();
        let last = parts.pop()?;

        if last == "__init__.py" {
            return Some(Self {
                parts,
                is_package_init: true,
            });
        }

        let last_path = Utf8Path::new(&last);
        if last_path.extension() != Some("py") {
            return None;
        }
        parts.push(last_path.file_stem()?.to_string());

        Some(Self {
            parts,
            is_package_init: false,
        })
    }
}

fn installed_app_package_module(installed_app: &str) -> &str {
    if let Some((module, _)) = installed_app.split_once(".apps.") {
        module
    } else if installed_app.ends_with("Config") {
        installed_app
            .rsplit_once('.')
            .map_or(installed_app, |(module, _)| module)
    } else {
        installed_app
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as _;
    use djls_source::FileSystem;
    use djls_source::OsFileSystem;
    use djls_source::SourceFiles;
    use serde::Deserialize;

    use super::*;
    use crate::project::Interpreter;
    use crate::project::ProjectIntrospector;
    use crate::project::resolve::SearchPaths;
    use crate::project::system::mock as sys_mock;
    use crate::testing::ProjectFixture;
    use crate::testing::TestDatabase;

    #[derive(Deserialize)]
    struct DjangoFactsGolden {
        template_dirs: Vec<String>,
        template_libraries: GoldenTemplateLibraries,
    }

    #[derive(Deserialize)]
    struct GoldenTemplateLibraries {
        builtins: Vec<String>,
        libraries: BTreeMap<String, String>,
        symbols: Vec<GoldenTemplateSymbol>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
    struct GoldenTemplateSymbol {
        kind: TemplateSymbolKind,
        name: String,
        load_name: Option<String>,
        library_module: String,
        module: String,
    }

    #[salsa::db]
    #[derive(Clone)]
    struct OsTestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<OsFileSystem>,
        files: SourceFiles,
        project: Option<Project>,
        project_introspector: Arc<ProjectIntrospector>,
    }

    impl OsTestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(OsFileSystem),
                files: SourceFiles::default(),
                project: None,
                project_introspector: Arc::new(ProjectIntrospector::new()),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for OsTestDatabase {}

    #[salsa::db]
    impl djls_source::Db for OsTestDatabase {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn file_system(&self) -> &dyn FileSystem {
            self.fs.as_ref()
        }
    }

    #[salsa::db]
    impl ProjectDb for OsTestDatabase {
        fn project(&self) -> Option<Project> {
            self.project
        }

        fn project_introspector(&self) -> Arc<ProjectIntrospector> {
            self.project_introspector.clone()
        }
    }

    fn project_with_settings(
        db: &mut TestDatabase,
        settings_module: &str,
        files: &[(&str, &str)],
    ) -> Project {
        let mut fixture = ProjectFixture::new("/proj").django_settings_module(settings_module);
        for (path, source) in files {
            fixture = fixture.file(*path, *source);
        }
        fixture.install(db)
    }

    #[test]
    fn settings_module_file_resolves_python_module() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[("/proj/myproject/settings.py", "INSTALLED_APPS = []\n")],
        );

        let file = settings_module_file(&db, project).expect("settings module should resolve");

        assert_eq!(file.path(&db), Utf8Path::new("/proj/myproject/settings.py"));
    }

    #[test]
    fn settings_module_file_returns_none_for_missing_module() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(&mut db, "myproject.settings", &[]);

        assert!(settings_module_file(&db, project).is_none());
    }

    #[test]
    fn django_settings_resolves_relative_star_imports() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.prod",
            &[
                (
                    "/proj/myproject/base.py",
                    "INSTALLED_APPS = ['django.contrib.auth']\n",
                ),
                (
                    "/proj/myproject/prod.py",
                    "from .base import *\nINSTALLED_APPS += ['blog']\n",
                ),
            ],
        );

        let settings = django_settings(&db, project);

        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(
            settings.installed_apps.values,
            vec!["django.contrib.auth".to_string(), "blog".to_string()]
        );
    }

    #[test]
    fn django_settings_recovers_from_star_import_cycle() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[(
                "/proj/myproject/settings.py",
                "from .settings import *\nINSTALLED_APPS = ['blog']\n",
            )],
        );

        let settings = django_settings(&db, project);

        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(settings.installed_apps.values, vec!["blog".to_string()]);
    }

    #[test]
    fn template_dirs_include_dirs_entries_before_app_dirs() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                ("/proj/templates/base.html", "base"),
                ("/proj/blog/__init__.py", ""),
                ("/proj/blog/templates/blog/detail.html", "detail"),
                (
                    "/proj/myproject/settings.py",
                    "from pathlib import Path\nBASE_DIR = Path(__file__).resolve().parent.parent\nINSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE_DIR / 'templates'], 'APP_DIRS': True}]\n",
                ),
            ],
        );

        let (dirs, knowledge) = template_dirs(&db, project).clone();

        assert_eq!(knowledge, StaticKnowledge::Known);
        assert_eq!(
            dirs,
            vec![
                Utf8PathBuf::from("/proj/templates"),
                Utf8PathBuf::from("/proj/blog/templates"),
            ]
        );
    }

    #[test]
    fn template_dirs_resolve_app_config_entries() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                ("/proj/blog/apps.py", ""),
                ("/proj/blog/templates/blog/detail.html", "detail"),
                (
                    "/proj/myproject/settings.py",
                    "INSTALLED_APPS = ['blog.apps.BlogConfig']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
                ),
            ],
        );

        let (dirs, knowledge) = template_dirs(&db, project).clone();

        assert_eq!(knowledge, StaticKnowledge::Known);
        assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/blog/templates")]);
    }

    #[test]
    fn template_dirs_resolve_apps_from_site_packages_search_path() {
        let mut db = TestDatabase::new();
        db.add_file("/site/pkg/__init__.py", "");
        db.add_file("/site/pkg/templates/pkg/index.html", "index");
        db.add_file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = ['pkg']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        );
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/proj"),
            &Interpreter::Auto,
            &["/site".to_string()],
        );
        search_paths.register_roots(&db);
        let project = ProjectFixture::new("/proj")
            .django_settings_module("myproject.settings")
            .search_paths(search_paths)
            .install(&mut db);

        let (dirs, knowledge) = template_dirs(&db, project).clone();

        assert_eq!(knowledge, StaticKnowledge::Known);
        assert_eq!(dirs, vec![Utf8PathBuf::from("/site/pkg/templates")]);
    }

    #[test]
    fn template_dirs_demote_unresolved_app_to_partial() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[(
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['missing']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            )],
        );

        let (dirs, knowledge) = template_dirs(&db, project).clone();

        assert!(dirs.is_empty());
        assert_eq!(knowledge, StaticKnowledge::Partial);
    }

    #[test]
    fn template_libraries_discover_app_templatetags_and_builtins() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                (
                    "/proj/django/template/defaulttags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
                ),
                (
                    "/proj/django/template/defaultfilters.py",
                    "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
                ),
                (
                    "/proj/django/template/loader_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
                ),
                ("/proj/blog/templatetags/__init__.py", ""),
                (
                    "/proj/blog/templatetags/custom.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n",
                ),
                (
                    "/proj/myproject/settings.py",
                    "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
                ),
            ],
        );

        let libraries = template_libraries(&db, project);

        assert_eq!(libraries.knowledge, StaticKnowledge::Known);
        let custom = libraries
            .best_loadable_library_str("custom")
            .expect("custom library should be discovered");
        assert_eq!(custom.module().as_str(), "blog.templatetags.custom");
        assert!(custom.symbols.iter().any(|symbol| symbol.name() == "hello"));
        assert_eq!(
            libraries
                .builtin_modules()
                .map(PyModuleName::as_str)
                .collect::<Vec<_>>(),
            vec![
                "django.template.defaulttags",
                "django.template.defaultfilters",
                "django.template.loader_tags",
            ]
        );
    }

    #[test]
    fn template_libraries_include_empty_registered_modules() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                (
                    "/proj/django/template/defaulttags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
                ),
                (
                    "/proj/django/template/defaultfilters.py",
                    "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
                ),
                (
                    "/proj/django/template/loader_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
                ),
                ("/proj/blog/templatetags/__init__.py", ""),
                (
                    "/proj/blog/templatetags/empty.py",
                    "from django import template\nregister = template.Library()\n",
                ),
                (
                    "/proj/myproject/settings.py",
                    "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
                ),
            ],
        );

        let libraries = template_libraries(&db, project);

        let empty = libraries.best_loadable_library_str("empty").unwrap();
        assert_eq!(empty.module().as_str(), "blog.templatetags.empty");
        assert!(empty.symbols.is_empty());
    }

    #[test]
    fn template_libraries_demote_unresolved_app_to_partial() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[(
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['missing']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            )],
        );

        let libraries = template_libraries(&db, project);

        assert_eq!(libraries.knowledge, StaticKnowledge::Partial);
    }

    #[test]
    fn template_libraries_include_options_libraries_and_builtins() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                (
                    "/proj/django/template/defaulttags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
                ),
                (
                    "/proj/django/template/defaultfilters.py",
                    "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
                ),
                (
                    "/proj/django/template/loader_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
                ),
                (
                    "/proj/custom_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef configured():\n    pass\n",
                ),
                (
                    "/proj/custom_builtin.py",
                    "from django import template\nregister = template.Library()\n@register.filter\ndef configured_filter(value):\n    return value\n",
                ),
                (
                    "/proj/myproject/settings.py",
                    "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}, 'builtins': ['custom_builtin']}}]\n",
                ),
            ],
        );

        let libraries = template_libraries(&db, project);

        let custom = libraries.best_loadable_library_str("custom").unwrap();
        assert_eq!(custom.module().as_str(), "custom_tags");
        assert!(
            custom
                .symbols
                .iter()
                .any(|symbol| symbol.name() == "configured")
        );
        assert!(
            libraries
                .builtin_libraries()
                .flat_map(|library| &library.symbols)
                .any(|symbol| symbol.name() == "configured_filter")
        );
    }

    #[test]
    fn template_libraries_keep_configured_libraries_when_installed_apps_unknown() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                (
                    "/proj/django/template/defaulttags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
                ),
                (
                    "/proj/django/template/defaultfilters.py",
                    "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
                ),
                (
                    "/proj/django/template/loader_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
                ),
                (
                    "/proj/project_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef configured():\n    pass\n",
                ),
                (
                    "/proj/myproject/settings.py",
                    "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'project_tags'}}}]\n",
                ),
            ],
        );

        let libraries = template_libraries(&db, project);

        assert_eq!(libraries.knowledge, StaticKnowledge::Partial);
        let custom = libraries.best_loadable_library_str("custom").unwrap();
        assert_eq!(custom.module().as_str(), "project_tags");
        assert!(
            custom
                .symbols
                .iter()
                .any(|symbol| symbol.name() == "configured")
        );
    }

    #[test]
    fn template_libraries_options_override_app_library_load_name() {
        let mut db = TestDatabase::new();
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                (
                    "/proj/django/template/defaulttags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
                ),
                (
                    "/proj/django/template/defaultfilters.py",
                    "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
                ),
                (
                    "/proj/django/template/loader_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
                ),
                ("/proj/blog/templatetags/__init__.py", ""),
                (
                    "/proj/blog/templatetags/custom.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef old_tag():\n    pass\n",
                ),
                (
                    "/proj/project_tags.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n",
                ),
                (
                    "/proj/myproject/settings.py",
                    "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True, 'OPTIONS': {'libraries': {'custom': 'project_tags'}}}]\n",
                ),
            ],
        );

        let libraries = template_libraries(&db, project);

        let custom = libraries.best_loadable_library_str("custom").unwrap();
        assert_eq!(custom.module().as_str(), "project_tags");
        assert!(
            custom
                .symbols
                .iter()
                .any(|symbol| symbol.name() == "new_tag")
        );
        assert!(
            !custom
                .symbols
                .iter()
                .any(|symbol| symbol.name() == "old_tag")
        );
    }

    #[test]
    #[ignore = "requires the e2e Django virtualenv with Django installed"]
    fn django_facts_golden_template_dirs_match() {
        let Ok(venv) = std::env::var("VIRTUAL_ENV") else {
            eprintln!("skipping golden comparison because VIRTUAL_ENV is not set");
            return;
        };
        let _guard = sys_mock::MockGuard;
        sys_mock::set_env_var("VIRTUAL_ENV", venv);

        let workspace = Utf8PathBuf::from_path_buf(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap(),
        )
        .unwrap();
        let project_root = workspace.join("tests/project");
        let golden_path = workspace.join("tests/fixtures/django-facts/django-5.2.json");
        let golden: DjangoFactsGolden =
            serde_json::from_str(&std::fs::read_to_string(golden_path.as_std_path()).unwrap())
                .unwrap();

        let mut db = OsTestDatabase::new();
        let settings = djls_conf::Settings::new(project_root.as_path(), None).unwrap();
        let project = Project::bootstrap(&db, project_root.as_path(), &settings);
        db.project = Some(project);

        let site_packages = project
            .search_paths(&db)
            .iter()
            .find_map(|search_path| {
                let path = search_path.path();
                path.components()
                    .any(|component| {
                        matches!(component.as_str(), "site-packages" | "dist-packages")
                    })
                    .then_some(path)
            })
            .expect("e2e venv should provide site-packages");
        let expected: Vec<_> = golden
            .template_dirs
            .into_iter()
            .map(|path| {
                path.replace("${PROJECT}", project_root.as_str())
                    .replace("${SITE_PACKAGES}", site_packages.as_str())
            })
            .collect();
        let actual: Vec<_> = template_dirs(&db, project)
            .0
            .iter()
            .map(ToString::to_string)
            .collect();

        assert_eq!(actual, expected);
    }

    #[test]
    #[ignore = "requires the e2e Django virtualenv with Django installed"]
    fn django_facts_golden_template_libraries_match() {
        let Ok(venv) = std::env::var("VIRTUAL_ENV") else {
            eprintln!("skipping golden comparison because VIRTUAL_ENV is not set");
            return;
        };
        let _guard = sys_mock::MockGuard;
        sys_mock::set_env_var("VIRTUAL_ENV", venv);

        let workspace = Utf8PathBuf::from_path_buf(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap(),
        )
        .unwrap();
        let project_root = workspace.join("tests/project");
        let golden_path = workspace.join("tests/fixtures/django-facts/django-5.2.json");
        let golden: DjangoFactsGolden =
            serde_json::from_str(&std::fs::read_to_string(golden_path.as_std_path()).unwrap())
                .unwrap();

        let mut db = OsTestDatabase::new();
        let settings = djls_conf::Settings::new(project_root.as_path(), None).unwrap();
        let project = Project::bootstrap(&db, project_root.as_path(), &settings);
        db.project = Some(project);

        let libraries = template_libraries(&db, project);
        let actual_builtins: Vec<_> = libraries
            .builtin_modules()
            .map(|module| module.as_str().to_string())
            .collect();
        assert_eq!(actual_builtins, golden.template_libraries.builtins);

        let actual_libraries: BTreeMap<_, _> = libraries
            .loadable_libraries()
            .map(|(name, library)| {
                (
                    name.as_str().to_string(),
                    library.module().as_str().to_string(),
                )
            })
            .collect();
        assert_eq!(actual_libraries, golden.template_libraries.libraries);

        let mut actual_symbols = comparable_symbols(libraries);
        let mut expected_symbols = golden.template_libraries.symbols;
        actual_symbols.sort();
        expected_symbols.sort();
        assert_eq!(actual_symbols, expected_symbols);
    }

    fn comparable_symbols(libraries: &TemplateLibraries) -> Vec<GoldenTemplateSymbol> {
        let mut symbols = Vec::new();

        for library in libraries.builtin_libraries() {
            for symbol in &library.symbols {
                symbols.push(GoldenTemplateSymbol {
                    kind: symbol.kind,
                    name: symbol.name().to_string(),
                    load_name: None,
                    library_module: library.module().as_str().to_string(),
                    module: library.module().as_str().to_string(),
                });
            }
        }

        for (load_name, library) in libraries.loadable_libraries() {
            for symbol in &library.symbols {
                symbols.push(GoldenTemplateSymbol {
                    kind: symbol.kind,
                    name: symbol.name().to_string(),
                    load_name: Some(load_name.as_str().to_string()),
                    library_module: library.module().as_str().to_string(),
                    module: library.module().as_str().to_string(),
                });
            }
        }

        symbols
    }
}
