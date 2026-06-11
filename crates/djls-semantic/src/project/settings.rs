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

use crate::project::db::Db as ProjectDb;
use crate::project::input::Project;
use crate::project::resolve::module_file_in_search_path;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";

#[salsa::tracked]
pub(crate) fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    let django_settings_module = project.django_settings_module(db).as_deref()?;
    module_file_for_project(db, project, django_settings_module)
}

#[salsa::tracked(returns(ref))]
pub(crate) fn django_settings_for_file(
    db: &dyn ProjectDb,
    file: File,
    project: Project,
) -> DjangoSettings {
    let source = file.source(db);
    let path = file.path(db);
    let mut resolver = SalsaSettingsSources { db, project };
    extract_settings(source.as_str(), path, &mut resolver)
}

#[salsa::tracked(returns(ref))]
pub(crate) fn django_settings(db: &dyn ProjectDb, project: Project) -> DjangoSettings {
    settings_module_file(db, project).map_or_else(DjangoSettings::default, |file| {
        django_settings_for_file(db, file, project).clone()
    })
}

#[salsa::tracked(returns(ref))]
pub fn template_dirs(db: &dyn ProjectDb, project: Project) -> (Vec<Utf8PathBuf>, StaticKnowledge) {
    touch_search_path_roots(db, project);

    let settings = django_settings(db, project);
    let mut dirs = Vec::new();
    let mut knowledge = settings.templates.knowledge;
    let backend_count = settings.templates.backends.len();

    for backend in
        settings.templates.backends.iter().filter(|backend| {
            is_django_templates_backend(backend.backend.as_deref(), backend_count)
        })
    {
        knowledge = weakest(knowledge, backend.knowledge);

        for dir in &backend.dirs {
            match dir {
                TemplateDirPath::Resolved(path) => dirs.push(path.clone()),
                TemplateDirPath::Unknown => demote_to_partial(&mut knowledge),
            }
        }

        if backend.app_dirs == Some(true) {
            knowledge = weakest(knowledge, settings.installed_apps.knowledge);
            for app in &settings.installed_apps.values {
                let Some(app_dir) = resolve_installed_app_dir(db, project, app) else {
                    demote_to_partial(&mut knowledge);
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

struct SalsaSettingsSources<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
}

impl SettingsSourceResolver for SalsaSettingsSources<'_> {
    fn resolve_star_import(
        &mut self,
        import: &SettingsStarImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource> {
        let module_path = resolve_star_import_module(self.db, self.project, importer, import)?;
        let file = module_file_for_project(self.db, self.project, &module_path)?;
        Some(SettingsSource {
            source: file.source(self.db).as_str().to_string(),
            path: file.path(self.db).to_path_buf(),
        })
    }
}

fn module_file_for_project(
    db: &dyn ProjectDb,
    project: Project,
    module_path: &str,
) -> Option<File> {
    touch_search_path_roots(db, project);

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

fn touch_search_path_roots(db: &dyn ProjectDb, project: Project) {
    for search_path in project.search_paths(db).iter() {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        } else {
            tracing::warn!(
                "Search path has no registered source root: {}",
                search_path.path()
            );
        }
    }
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

    let module_file = module_parts_for_file(db, project, base)?;
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

fn module_parts_for_file(
    db: &dyn ProjectDb,
    project: Project,
    file_path: &Utf8Path,
) -> Option<ModuleFileParts> {
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
        return Some(ModuleFileParts {
            parts,
            is_package_init: true,
        });
    }

    let last_path = Utf8Path::new(&last);
    if last_path.extension() != Some("py") {
        return None;
    }
    parts.push(last_path.file_stem()?.to_string());

    Some(ModuleFileParts {
        parts,
        is_package_init: false,
    })
}

fn is_django_templates_backend(backend: Option<&str>, backend_count: usize) -> bool {
    match backend {
        Some(DJANGO_TEMPLATES_BACKEND) => true,
        None if backend_count == 1 => true,
        _ => false,
    }
}

fn resolve_installed_app_dir(
    db: &dyn ProjectDb,
    project: Project,
    installed_app: &str,
) -> Option<Utf8PathBuf> {
    let module = installed_app_package_module(installed_app);
    if module.is_empty() {
        return None;
    }

    let relative = module.replace('.', "/");
    for search_path in project.search_paths(db).iter() {
        let candidate = search_path.path().join(&relative);
        if db.path_is_dir(&candidate) {
            return Some(candidate);
        }
    }

    None
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

fn weakest(left: StaticKnowledge, right: StaticKnowledge) -> StaticKnowledge {
    match (left, right) {
        (StaticKnowledge::Unknown, _) | (_, StaticKnowledge::Unknown) => StaticKnowledge::Unknown,
        (StaticKnowledge::Partial, _) | (_, StaticKnowledge::Partial) => StaticKnowledge::Partial,
        (StaticKnowledge::Known, StaticKnowledge::Known) => StaticKnowledge::Known,
    }
}

fn demote_to_partial(knowledge: &mut StaticKnowledge) {
    if *knowledge == StaticKnowledge::Known {
        *knowledge = StaticKnowledge::Partial;
    }
}

#[cfg(test)]
mod tests {
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
    fn django_settings_for_file_resolves_relative_star_imports() {
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
    fn django_settings_for_file_recovers_from_star_import_cycle() {
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
    #[ignore = "requires the e2e Django virtualenv with Django installed"]
    fn template_dirs_match_django_facts_golden() {
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
}
