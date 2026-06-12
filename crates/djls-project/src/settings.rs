use std::collections::BTreeSet;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use crate::db::Db as ProjectDb;
use crate::extraction::DjangoSettings;
use crate::extraction::RegistrationKind;
use crate::extraction::SettingsSource;
use crate::extraction::SettingsSourceResolver;
use crate::extraction::SettingsStarImport;
use crate::extraction::StaticKnowledge;
use crate::extraction::TemplateDirPath;
use crate::extraction::collect_registrations_from_body;
use crate::extraction::extract_settings;
use crate::names::LibraryName;
use crate::names::PyModuleName;
use crate::names::TemplateSymbolName;
use crate::parse::parse_python_module;
use crate::project::Project;
use crate::resolve::module_file_in_search_path;
use crate::symbols::SymbolDefinition;
use crate::symbols::TemplateLibraries;
use crate::symbols::TemplateLibrary;
use crate::symbols::TemplateSymbol;
use crate::symbols::TemplateSymbolKind;

const DEFAULT_TEMPLATE_BUILTINS: &[&str] = &[
    "django.template.defaulttags",
    "django.template.defaultfilters",
    "django.template.loader_tags",
];

#[salsa::tracked]
pub fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    let django_settings_module = project.django_settings_module(db).as_deref()?;
    module_file(db, project, django_settings_module)
}

#[salsa::tracked(returns(ref))]
pub fn django_settings(db: &dyn ProjectDb, project: Project) -> DjangoSettings {
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

    let path = file.path(db).to_path_buf();
    let Ok(source) = db.read_file(&path) else {
        return vec![file];
    };

    // The refresh bump set must cover the same settings source graph the
    // extractor would read against current disk content.
    let mut resolver = DiskSettingsResolver {
        db,
        project,
        touched: Vec::new(),
    };
    let _ = extract_settings(&source, &path, &mut resolver);

    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    if seen.insert(path) {
        files.push(file);
    }
    for file in resolver.touched {
        let path = file.path(db).to_path_buf();
        if seen.insert(path) {
            files.push(file);
        }
    }
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
                let Some(app_dir) = package_dir(
                    resolver.db,
                    resolver.project,
                    installed_app_package_module(app),
                ) else {
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

    let (knowledge, discovered_libraries) = templatetag_package_libraries(&resolver, "django");
    libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
    for (load_name, library) in discovered_libraries {
        libraries.insert_loadable(load_name, library);
    }

    if settings.installed_apps.knowledge != StaticKnowledge::Unknown {
        for installed_app in &settings.installed_apps.values {
            let (knowledge, discovered_libraries) = templatetag_package_libraries(
                &resolver,
                installed_app_package_module(installed_app),
            );
            libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
            for (load_name, library) in discovered_libraries {
                libraries.insert_loadable(load_name, library);
            }
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
            let Ok(load_name) = LibraryName::parse(load_name) else {
                libraries.knowledge = libraries.knowledge.demoted_to_partial();
                continue;
            };
            let (knowledge, library) = library_from_module_path(&resolver, module_path);
            libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
            if let Some(library) = library {
                libraries.insert_loadable(load_name, library);
            }
        }

        for module_path in DEFAULT_TEMPLATE_BUILTINS
            .iter()
            .copied()
            .chain(backend.builtins.iter().map(String::as_str))
        {
            let (knowledge, library) = library_from_module_path(&resolver, module_path);
            libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
            if let Some(library) = library {
                libraries.push_builtin(library);
            }
        }
    }

    libraries
}

fn templatetag_package_libraries(
    resolver: &SalsaSettingsResolver<'_>,
    package_module: &str,
) -> (StaticKnowledge, Vec<(LibraryName, TemplateLibrary)>) {
    let mut knowledge = StaticKnowledge::Known;
    let mut libraries = Vec::new();

    if package_module.is_empty() {
        return (knowledge.demoted_to_partial(), libraries);
    }

    if PyModuleName::parse(package_module).is_err() {
        return (knowledge.demoted_to_partial(), libraries);
    }

    let Some(package_dir) = package_dir(resolver.db, resolver.project, package_module) else {
        return (knowledge.demoted_to_partial(), libraries);
    };

    let templatetags_dir = package_dir.join("templatetags");
    if !resolver
        .db
        .path_is_file(&templatetags_dir.join("__init__.py"))
    {
        return (knowledge, libraries);
    }

    let entries = match resolver
        .db
        .walk_entries(&templatetags_dir, &WalkOptions::shallow())
    {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Failed to walk template tag package {templatetags_dir}: {err}");
            return (knowledge.demoted_to_partial(), libraries);
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
            knowledge = knowledge.demoted_to_partial();
            continue;
        };
        let module_path = format!("{package_module}.templatetags.{stem}");
        let Ok(module) = PyModuleName::parse(&module_path) else {
            knowledge = knowledge.demoted_to_partial();
            continue;
        };

        let file = resolver.db.get_or_create_file(&entry.path);
        let analysis = TemplateLibraryAnalysis::from_file(resolver.db, file);
        if !analysis.defines_library && analysis.symbols.is_empty() {
            continue;
        }

        let mut library = TemplateLibrary::new(module);
        library.merge_symbols(analysis.symbols);
        libraries.push((load_name, library));
    }

    (knowledge, libraries)
}

fn library_from_module_path(
    resolver: &SalsaSettingsResolver<'_>,
    module_path: &str,
) -> (StaticKnowledge, Option<TemplateLibrary>) {
    let Ok(module) = PyModuleName::parse(module_path) else {
        return (StaticKnowledge::Partial, None);
    };

    let mut library = TemplateLibrary::new(module);
    if let Some(file) = module_file(resolver.db, resolver.project, library.module().as_str()) {
        library.merge_symbols(TemplateLibraryAnalysis::from_file(resolver.db, file).symbols);
        (StaticKnowledge::Known, Some(library))
    } else {
        (StaticKnowledge::Partial, Some(library))
    }
}

pub(crate) struct TemplateLibraryAnalysis {
    pub(crate) defines_library: bool,
    pub(crate) symbols: Vec<TemplateSymbol>,
}

impl TemplateLibraryAnalysis {
    pub(crate) fn from_file(db: &dyn ProjectDb, file: File) -> Self {
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
            let kind = match registration.kind {
                RegistrationKind::Tag
                | RegistrationKind::SimpleTag
                | RegistrationKind::InclusionTag
                | RegistrationKind::SimpleBlockTag => TemplateSymbolKind::Tag,
                RegistrationKind::Filter => TemplateSymbolKind::Filter,
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

impl SettingsSourceResolver for SalsaSettingsResolver<'_> {
    fn resolve_star_import(
        &mut self,
        import: &SettingsStarImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource> {
        let module_path = resolve_star_import_module(self.db, self.project, importer, import)?;
        let file = module_file(self.db, self.project, &module_path)?;
        Some(SettingsSource {
            source: file.source(self.db).as_str().to_string(),
            path: file.path(self.db).to_path_buf(),
        })
    }
}

struct DiskSettingsResolver<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    touched: Vec<File>,
}

impl SettingsSourceResolver for DiskSettingsResolver<'_> {
    fn resolve_star_import(
        &mut self,
        import: &SettingsStarImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource> {
        let module_path = resolve_star_import_module(self.db, self.project, importer, import)?;
        let file = module_file(self.db, self.project, &module_path)?;
        let path = file.path(self.db).to_path_buf();
        self.touched.push(file);
        let source = self.db.read_file(&path).ok()?;

        Some(SettingsSource { source, path })
    }
}

fn module_file(db: &dyn ProjectDb, project: Project, module_path: &str) -> Option<File> {
    project.touch_search_path_roots(db);

    for search_path in project.search_paths(db).iter() {
        let Some(path) =
            module_file_in_search_path(db.file_system(), module_path, search_path.path())
        else {
            continue;
        };
        return Some(db.get_or_create_file(&path));
    }

    None
}

fn package_dir(db: &dyn ProjectDb, project: Project, package_module: &str) -> Option<Utf8PathBuf> {
    if package_module.is_empty() {
        return None;
    }

    let relative = package_module.replace('.', "/");
    for search_path in project.search_paths(db).iter() {
        let candidate = search_path.path().join(&relative);
        if db.path_is_dir(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn resolve_star_import_module(
    db: &dyn ProjectDb,
    project: Project,
    base: &Utf8Path,
    import: &SettingsStarImport,
) -> Option<String> {
    if import.level == 0 {
        return import.module.clone();
    }

    let module_file = ModuleFileParts::from_path(db, project, base)?;
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
