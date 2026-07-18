use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::compute_django_environment;
use djls_project::testing::compute_project_facts;
use djls_project::testing::django_settings;
use djls_project::testing::python_syntax_errors;
use djls_project::*;
use djls_source::ChangeEvent;
use djls_source::Db as _;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::RootWalk;
use djls_source::SourceChanges;
use djls_source::SourceFiles;
use djls_source::WalkOptions;
use djls_testing::Corpus;
use djls_testing::OsTestDatabase;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use serde::Deserialize;

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

fn library_name(name: &str) -> LibraryName {
    LibraryName::parse(name).unwrap()
}

fn active_builtin_modules(libraries: &TemplateLibraries) -> Vec<String> {
    TemplateEnvironment::from_project_inventory(libraries)
        .resolved_libraries()
        .into_iter()
        .filter(|&library| library.load_name().is_none())
        .map(|library| library.module_name().as_str().to_string())
        .collect()
}

fn has_case(value: &serde_json::Value, kind: &str) -> bool {
    value["cases"].as_array().is_some_and(|cases| {
        cases
            .iter()
            .any(|case| case.as_str() == Some(kind) || case.get(kind).is_some())
    })
}

fn execution_count(
    db: &(impl salsa::Database + ?Sized),
    events: &[salsa::Event],
    query_name: &str,
) -> usize {
    events
        .iter()
        .filter(|event| match &event.kind {
            salsa::EventKind::WillExecute { database_key } => db
                .ingredient_debug_name(database_key.ingredient_index())
                .contains(query_name),
            _ => false,
        })
        .count()
}

fn update_project_file(db: &mut TestDatabase, path: &str, source: &str) {
    db.add_file(path, source);
    SourceChanges::new([ChangeEvent::ContentChanged(path.into())]).apply(db);
}

fn update_settings_file(db: &mut TestDatabase, source: &str) {
    update_project_file(db, "/proj/myproject/settings.py", source);
}

#[test]
fn unrelated_recovered_syntax_error_does_not_degrade_settings() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = ['blog']\ndef broken(",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let app_cases = settings["installed_apps"]["cases"].as_array().unwrap();
    assert_eq!(app_cases.len(), 1);
    assert!(app_cases[0].get("known").is_some());
    assert!(settings.get("parse_status").is_none());
}

#[test]
fn named_imported_syntax_impact_only_weakens_affected_setting() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            "TEMPLATES = []\nif FLAG:\n    INSTALLED_APPS = ['blog']\n    broken(\n",
        )
        .file(
            "/proj/myproject/settings/local.py",
            "from .base import INSTALLED_APPS, TEMPLATES",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let app_cases = settings["installed_apps"]["cases"].as_array().unwrap();
    assert_eq!(app_cases.len(), 3);
    assert!(!app_cases.iter().any(|case| case == "unset"));
    assert_eq!(
        app_cases
            .iter()
            .filter(|case| case.get("known").is_some())
            .count(),
        1
    );
    assert_eq!(
        app_cases
            .iter()
            .filter(|case| case.get("dynamic").is_some())
            .count(),
        2
    );

    let template_cases = settings["templates"]["cases"].as_array().unwrap();
    assert_eq!(template_cases.len(), 1);
    assert!(template_cases[0].get("known").is_some());
    assert!(settings.get("parse_status").is_none());
}

#[test]
fn star_imported_name_scoped_syntax_impact_does_not_open_namespace() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            "TEMPLATES = []\nif FLAG:\n    INSTALLED_APPS = ['blog']\n    broken(\n",
        )
        .file("/proj/myproject/settings/local.py", "from .base import *\n")
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let app_cases = settings["installed_apps"]["cases"].as_array().unwrap();
    assert_eq!(app_cases.len(), 3);
    assert!(app_cases.iter().any(|case| case == "unset"));
    assert_eq!(
        app_cases
            .iter()
            .filter(|case| case.get("known").is_some())
            .count(),
        1
    );
    assert_eq!(
        app_cases
            .iter()
            .filter(|case| case.get("dynamic").is_some())
            .count(),
        1
    );

    let template_cases = settings["templates"]["cases"].as_array().unwrap();
    assert_eq!(template_cases.len(), 1);
    assert!(template_cases[0].get("known").is_some());
}

#[test]
fn later_exact_assignment_dominates_syntax_impact_through_named_import() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            "INSTALLED_APPS = [\n    'stale',\n    @\n]\nINSTALLED_APPS = ['base']\n",
        )
        .file(
            "/proj/myproject/settings/local.py",
            "from .base import INSTALLED_APPS\n",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let cases = settings["installed_apps"]["cases"].as_array().unwrap();

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "base");
}

#[test]
fn later_exact_assignment_dominates_syntax_impact_through_star_import() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            "INSTALLED_APPS = [\n    'stale',\n    @\n]\nINSTALLED_APPS = ['base']\n",
        )
        .file("/proj/myproject/settings/local.py", "from .base import *\n")
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let cases = settings["installed_apps"]["cases"].as_array().unwrap();

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "base");
}

#[test]
fn namespace_wide_syntax_exclusion_survives_named_and_star_imports() {
    for root_import in [
        "from .base import INSTALLED_APPS\n",
        "from .base import *\n",
    ] {
        let mut db = TestDatabase::new();
        let project = ProjectFixture::new("/proj")
            .django_settings_module("myproject.settings")
            .file("/proj/myproject/__init__.py", "")
            .file("/proj/myproject/clean.py", "")
            .file("/proj/myproject/apps.py", "APPS = ['base']\n")
            .file(
                "/proj/myproject/base.py",
                "if FLAG:\n    from .clean import *\n    broken(]\nfrom .apps import APPS as INSTALLED_APPS\n",
            )
            .file("/proj/myproject/settings.py", root_import)
            .install(&mut db);

        let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
        let cases = settings["installed_apps"]["cases"].as_array().unwrap();

        assert_eq!(cases.len(), 1, "{root_import}");
        assert_eq!(cases[0]["known"]["apps"][0]["value"], "base");
    }
}

#[test]
fn settings_accept_supported_python_newer_than_ruff_default_target() {
    let mut db = TestDatabase::new();
    let path = Utf8Path::new("/proj/myproject/settings.py");
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            path.as_str(),
            "type AppName = str\nINSTALLED_APPS = ['blog']\nTEMPLATES = []\n",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let errors = python_syntax_errors(&db, db.file(path)).expect("file should be Python");

    assert!(
        errors
            .iter()
            .any(|error| error.class == PythonSyntaxErrorClass::Unsupported)
    );
    assert!(
        errors
            .iter()
            .all(|error| error.class != PythonSyntaxErrorClass::Ordinary)
    );
    assert!(settings.get("parse_status").is_none());
    assert_eq!(
        settings["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "blog"
    );
}

#[test]
fn settings_consumers_share_one_core_evaluation_without_mutation() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", "INSTALLED_APPS = ['a']")
        .install(&mut db);

    let _ = django_settings(&db, project);
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log.take();

    assert_eq!(execution_count(&db, &events, "evaluate_python_module"), 1);
    assert_eq!(execution_count(&db, &events, "python_module_values"), 1);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        1
    );
    assert_eq!(execution_count(&db, &events, "django_settings"), 1);
    assert_eq!(execution_count(&db, &events, "settings_sources"), 1);
}

#[test]
fn comment_only_leaf_edit_backdates_before_evaluation_root_and_sibling() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "from .leaf import *\nfrom .sibling import *\n",
        )
        .file("/proj/myproject/leaf.py", "INSTALLED_APPS = ['a']\n")
        .file("/proj/myproject/sibling.py", "TEMPLATES = []\n")
        .install(&mut db);

    let _ = django_settings(&db, project);
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let _ = event_log.take();

    update_project_file(
        &mut db,
        "/proj/myproject/leaf.py",
        "INSTALLED_APPS = ['a']\n# comment only\n",
    );
    let _ = django_settings(&db, project);
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log.take();

    assert_eq!(execution_count(&db, &events, "parse_python_file"), 1);
    assert_eq!(execution_count(&db, &events, "evaluate_python_module"), 0);
    assert_eq!(execution_count(&db, &events, "python_module_values"), 0);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        0
    );
    assert_eq!(execution_count(&db, &events, "django_settings"), 0);
    assert_eq!(execution_count(&db, &events, "settings_sources"), 0);
}

#[test]
fn value_change_backdates_dependency_projection() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", "INSTALLED_APPS = ['a']")
        .install(&mut db);

    let _ = django_settings(&db, project);
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let _ = event_log.take();

    update_settings_file(&mut db, "INSTALLED_APPS = ['b']");
    let _ = django_settings(&db, project);
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log.take();

    assert_eq!(execution_count(&db, &events, "evaluate_python_module"), 1);
    assert_eq!(execution_count(&db, &events, "python_module_values"), 1);
    assert_eq!(execution_count(&db, &events, "django_settings"), 1);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        1
    );
    assert_eq!(execution_count(&db, &events, "settings_sources"), 0);
}

#[test]
fn dependency_change_backdates_value_projection() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/extra.py", "")
        .file("/proj/myproject/settings.py", "INSTALLED_APPS = ['a']")
        .install(&mut db);

    let before = serde_json::to_value(django_settings(&db, project)).unwrap();
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let _ = event_log.take();

    update_settings_file(&mut db, "INSTALLED_APPS = ['a']\nfrom .extra import *");
    let sources = ProjectFactsPhase::SettingsSources.run(&db, project);
    let after = serde_json::to_value(django_settings(&db, project)).unwrap();
    let events = event_log.take();

    assert_eq!(after, before);
    assert_eq!(sources.count(), 2);
    assert_eq!(execution_count(&db, &events, "evaluate_python_module"), 2);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        1
    );
    assert_eq!(execution_count(&db, &events, "settings_sources"), 1);
    assert_eq!(execution_count(&db, &events, "python_module_values"), 1);
    assert_eq!(execution_count(&db, &events, "django_settings"), 0);
}

#[test]
fn origin_shift_changes_values_but_backdates_dependency_projection() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", "INSTALLED_APPS = ['a']\n")
        .install(&mut db);

    let before = serde_json::to_value(django_settings(&db, project)).unwrap();
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let _ = event_log.take();

    update_settings_file(&mut db, "\nINSTALLED_APPS = ['a']\n");
    let after = serde_json::to_value(django_settings(&db, project)).unwrap();
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log.take();

    assert_ne!(after, before);
    assert_eq!(execution_count(&db, &events, "evaluate_python_module"), 1);
    assert_eq!(execution_count(&db, &events, "python_module_values"), 1);
    assert_eq!(execution_count(&db, &events, "django_settings"), 1);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        1
    );
    assert_eq!(execution_count(&db, &events, "settings_sources"), 0);
}

#[test]
fn unreachable_import_edit_keeps_root_paths_cold() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "if False:\n    from .unreachable import *\nINSTALLED_APPS = ['a']\n",
        )
        .file("/proj/myproject/unreachable.py", "VALUE = 'old'\n")
        .install(&mut db);
    let _unreachable = db.file(Utf8Path::new("/proj/myproject/unreachable.py"));

    let _ = django_settings(&db, project);
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let _ = event_log.take();

    update_project_file(&mut db, "/proj/myproject/unreachable.py", "VALUE = 'new'\n");
    let _ = django_settings(&db, project);
    let _ = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log.take();

    assert_eq!(execution_count(&db, &events, "evaluate_python_module"), 0);
    assert_eq!(execution_count(&db, &events, "python_module_values"), 0);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        0
    );
    assert_eq!(execution_count(&db, &events, "django_settings"), 0);
    assert_eq!(execution_count(&db, &events, "settings_sources"), 0);
}

#[test]
fn direct_settings_cycle_is_bounded_and_retains_local_values() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "from .settings import *\nINSTALLED_APPS = ['local']\n",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let sources = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log.take();

    assert_eq!(
        settings["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "local"
    );
    assert_eq!(sources.count(), 1);
    let evaluations = execution_count(&db, &events, "evaluate_python_module");
    assert!((1..=12).contains(&evaluations));
    assert_eq!(execution_count(&db, &events, "python_module_values"), 1);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        1
    );
}

#[test]
fn imported_uncertain_namespace_preserves_local_setting_alternatives() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nfrom .plugins import *\n",
        )
        .file("/proj/myproject/plugins.py", "from .missing import *\n")
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let cases = settings["installed_apps"]["cases"].as_array().unwrap();
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| known["apps"][0]["value"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(known, ["first", "second"].into_iter().collect());
    assert!(has_case(&settings["installed_apps"], "dynamic"));
}

#[test]
fn named_import_of_absent_open_setting_is_dynamic_without_domain_absence() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "from .plugins import TEMPLATES\n",
        )
        .file(
            "/proj/myproject/plugins.py",
            "if ENABLED:\n    from .missing import *\n",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    assert!(!has_case(&settings["templates"], "unset"), "{settings:#}");
    assert!(has_case(&settings["templates"], "dynamic"), "{settings:#}");
}

#[test]
fn conditional_star_binding_falls_back_to_the_pre_import_local_value() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = ['local']\nfrom .plugins import *\n",
        )
        .file(
            "/proj/myproject/plugins.py",
            "if ENABLED:\n    INSTALLED_APPS = ['imported']\n",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let cases = settings["installed_apps"]["cases"].as_array().unwrap();
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| known["apps"][0]["value"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(known, ["imported", "local"].into_iter().collect());
    assert!(!cases.iter().any(|case| case == "unset"));
}

#[test]
fn two_file_settings_cycle_is_bounded_and_retains_local_values() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            "from .base import *\nINSTALLED_APPS = ['local']\n",
        )
        .file(
            "/proj/myproject/base.py",
            "from .settings import *\nTEMPLATES = []\n",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    let sources = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log.take();

    assert_eq!(
        settings["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "local"
    );
    assert!(has_case(&settings["templates"], "known"));
    assert_eq!(sources.count(), 2);
    let evaluations = execution_count(&db, &events, "evaluate_python_module");
    assert!((2..=24).contains(&evaluations));
    assert_eq!(execution_count(&db, &events, "python_module_values"), 1);
    assert_eq!(
        execution_count(&db, &events, "python_module_dependencies"),
        1
    );
}

struct ToggleReadFileSystem {
    inner: InMemoryFileSystem,
    toggled_path: Utf8PathBuf,
    readable: Arc<AtomicBool>,
}

impl FileSystem for ToggleReadFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        if path == self.toggled_path && !self.readable.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "test file is unreadable",
            ));
        }
        self.inner.read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.inner.exists(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.inner.is_dir(path)
    }

    fn case_sensitivity(&self) -> djls_source::CaseSensitivity {
        self.inner.case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.inner.path_exists_case_sensitive(path, prefix)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        self.inner.walk_root(root, options)
    }
}

#[salsa::db]
#[derive(Clone)]
struct EventTestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<dyn FileSystem>,
    files: SourceFiles,
    project: Option<Project>,
}

#[salsa::db]
impl salsa::Database for EventTestDatabase {}

#[salsa::db]
impl djls_source::Db for EventTestDatabase {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
}

#[salsa::db]
impl djls_project::Db for EventTestDatabase {
    fn project(&self) -> Option<Project> {
        self.project
    }
}

#[test]
fn readable_unreadable_rescans_recompute_ancestors_once_and_retain_dependency() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let readable = Arc::new(AtomicBool::new(true));
    let leaf_path = Utf8PathBuf::from("/proj/myproject/leaf.py");
    let mut inner = InMemoryFileSystem::new();
    inner.add_file(
        Utf8PathBuf::from("/proj/myproject/__init__.py"),
        String::new(),
    );
    inner.add_file(
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        "from .leaf import *\nINSTALLED_APPS = ['local']\n".to_string(),
    );
    inner.add_file(leaf_path.clone(), "TEMPLATES = []\n".to_string());
    let mut db = EventTestDatabase {
        storage: salsa::Storage::new(Some(Box::new({
            let events = events.clone();
            move |event| events.lock().unwrap().push(event)
        }))),
        fs: Arc::new(ToggleReadFileSystem {
            inner,
            toggled_path: leaf_path,
            readable: readable.clone(),
        }),
        files: SourceFiles::default(),
        project: None,
    };
    let root = Utf8PathBuf::from("/proj");
    let interpreter = Interpreter::Auto;
    let pythonpath = Vec::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        root.as_path(),
        &interpreter,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = Project::new(
        &db,
        root,
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("myproject.settings").unwrap()),
        pythonpath,
        Vec::new(),
        djls_conf::Settings::default().tagspecs().clone(),
    );
    db.project = Some(project);

    let _ = django_settings(&db, project);
    assert_eq!(
        ProjectFactsPhase::SettingsSources.run(&db, project).count(),
        2
    );
    events.lock().unwrap().clear();

    for next_readable in [false, true] {
        readable.store(next_readable, Ordering::SeqCst);
        SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);
        let _ = django_settings(&db, project);
        assert_eq!(
            ProjectFactsPhase::SettingsSources.run(&db, project).count(),
            2
        );
        let transition_events = std::mem::take(&mut *events.lock().unwrap());

        assert_eq!(
            execution_count(&db, &transition_events, "evaluate_python_module"),
            2
        );
        assert_eq!(
            execution_count(&db, &transition_events, "python_module_values"),
            1
        );
        assert_eq!(
            execution_count(&db, &transition_events, "python_module_dependencies"),
            1
        );
        assert_eq!(
            execution_count(&db, &transition_events, "django_settings"),
            1
        );
        assert_eq!(
            execution_count(&db, &transition_events, "settings_sources"),
            1
        );
    }
}

enum FileSystemFailure {
    Read(Utf8PathBuf),
    Walk(Utf8PathBuf),
    PartialWalk(Utf8PathBuf),
    PathToFile(Utf8PathBuf),
}

struct FailingFileSystem {
    inner: InMemoryFileSystem,
    failure: FileSystemFailure,
}

impl FileSystem for FailingFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        if let FileSystemFailure::Read(unreadable) = &self.failure
            && path == unreadable
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "test file is unreadable",
            ));
        }

        self.inner.read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.inner.exists(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        if let FileSystemFailure::PathToFile(unindexable) = &self.failure
            && path == unindexable
        {
            return false;
        }

        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.inner.is_dir(path)
    }

    fn case_sensitivity(&self) -> djls_source::CaseSensitivity {
        self.inner.case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.inner.path_exists_case_sensitive(path, prefix)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        match &self.failure {
            FileSystemFailure::Walk(failing_root) if root == failing_root => {
                RootWalk::Inaccessible(io::ErrorKind::PermissionDenied)
            }
            FileSystemFailure::PartialWalk(failing_root) if root == failing_root => {
                match self.inner.walk_root(root, options) {
                    RootWalk::Directory { entries, .. } => RootWalk::Directory {
                        entries,
                        issues: vec![io::ErrorKind::PermissionDenied],
                    },
                    other => other,
                }
            }
            _ => self.inner.walk_root(root, options),
        }
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

fn project_with_file_system_failure(
    files: &[(&str, &str)],
    failure: FileSystemFailure,
) -> (OsTestDatabase, Project) {
    let mut fs = InMemoryFileSystem::new();
    for (path, source) in files {
        fs.add_file(Utf8PathBuf::from(*path), (*source).to_string());
    }

    let mut db =
        OsTestDatabase::with_file_system(Arc::new(FailingFileSystem { inner: fs, failure }));
    let root = Utf8PathBuf::from("/proj");
    let interpreter = Interpreter::Auto;
    let pythonpath = Vec::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        root.as_path(),
        &interpreter,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = Project::new(
        &db,
        root,
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("myproject.settings").unwrap()),
        pythonpath,
        Vec::new(),
        djls_conf::Settings::default().tagspecs().clone(),
    );
    db.set_project(project);

    (db, project)
}

fn complete_template_dirs(db: &dyn djls_project::Db, project: Project) -> Vec<Utf8PathBuf> {
    let directories = template_directories(db, project);
    assert!(!directories.configuration_may_omit_roots());
    directories
        .known_roots()
        .map(Utf8Path::to_path_buf)
        .collect()
}

fn apply_project_discovery(db: &mut TestDatabase) {
    let _facts = run_django_discovery(db).expect("project should be configured");
}

fn project_requiring_environment_application(db: &mut TestDatabase) -> Project {
    db.add_file(
        "/proj/settings.py",
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'extras.tags'}}}]\n",
    );
    db.add_file("/vendor/extras/__init__.py", "");
    db.add_file(
        "/vendor/extras/tags.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom(): pass\n",
    );

    let project = Project::new(
        db,
        Utf8PathBuf::from("/proj"),
        SearchPaths::default(),
        Interpreter::Auto,
        Some(PythonModuleName::parse("settings").unwrap()),
        vec![Utf8PathBuf::from("/vendor")],
        Vec::new(),
        djls_conf::Settings::default().tagspecs().clone(),
    );
    db.set_project(project);
    project
}

#[test]
fn django_discovery_run_matches_explicit_phase_sequence() {
    let library_path = Utf8Path::new("/vendor/extras/tags.py");
    let original_source = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom(): pass\n";
    let updated_source = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom(value): pass\n";

    let mut sequenced = TestDatabase::new();
    let sequenced_project = project_requiring_environment_application(&mut sequenced);
    let sequenced_library = sequenced.file(library_path);
    assert_eq!(
        sequenced_library.try_source(&sequenced).unwrap().as_str(),
        original_source
    );
    let sequenced_revision_before = sequenced_library.revision(&sequenced);
    sequenced.add_file(library_path.as_str(), updated_source);

    let environment = compute_django_environment(&sequenced, sequenced_project);
    apply_django_environment(&mut sequenced, environment);
    let expected = compute_project_facts(&sequenced, sequenced_project);
    apply_project_facts(&mut sequenced, &expected);

    assert_eq!(
        sequenced_library.try_source(&sequenced).unwrap().as_str(),
        updated_source
    );
    assert_eq!(
        sequenced_library.revision(&sequenced),
        sequenced_revision_before + 1
    );

    let mut synchronous = TestDatabase::new();
    let synchronous_project = project_requiring_environment_application(&mut synchronous);
    let synchronous_library = synchronous.file(library_path);
    assert_eq!(
        synchronous_library
            .try_source(&synchronous)
            .unwrap()
            .as_str(),
        original_source
    );
    let synchronous_revision_before = synchronous_library.revision(&synchronous);
    synchronous.add_file(library_path.as_str(), updated_source);

    let actual = run_django_discovery(&mut synchronous).expect("project should be configured");

    assert_eq!(
        synchronous_library
            .try_source(&synchronous)
            .unwrap()
            .as_str(),
        updated_source
    );
    assert_eq!(
        synchronous_library.revision(&synchronous),
        synchronous_revision_before + 1
    );
    assert_eq!(actual, expected);
    assert_eq!(actual.file_paths(), expected.file_paths());
    assert_eq!(
        synchronous_project.search_paths(&synchronous),
        sequenced_project.search_paths(&sequenced)
    );
    for path in actual.file_paths() {
        assert_eq!(
            synchronous.file(path).try_source(&synchronous),
            sequenced.file(path).try_source(&sequenced),
            "synchronized source outcome differs for {path}"
        );
    }
}

#[test]
fn django_discovery_run_applies_environment_before_computing_facts() {
    let mut db = TestDatabase::new();
    let project = project_requiring_environment_application(&mut db);

    assert!(project.search_paths(&db).iter().next().is_none());

    let facts = run_django_discovery(&mut db).expect("project should be configured");

    assert_eq!(
        project
            .search_paths(&db)
            .iter()
            .map(djls_project::SearchPath::path)
            .collect::<Vec<_>>(),
        [Utf8Path::new("/proj"), Utf8Path::new("/vendor")]
    );
    assert!(
        facts
            .file_paths()
            .contains(&Utf8PathBuf::from("/proj/settings.py"))
    );
    assert!(
        facts
            .file_paths()
            .contains(&Utf8PathBuf::from("/vendor/extras/tags.py"))
    );
}

#[test]
fn django_discovery_run_without_project_returns_none_without_mutating_sources() {
    let mut db = TestDatabase::new();
    let path = Utf8Path::new("/proj/preexisting.py");
    db.add_file(path.as_str(), "before\n");
    let file = db.file(path);
    let source_before = file.try_source(&db);
    let revision_before = file.revision(&db);

    db.add_file(path.as_str(), "after\n");

    assert_eq!(run_django_discovery(&mut db), None);
    assert_eq!(file.revision(&db), revision_before);
    assert_eq!(file.try_source(&db), source_before);
}

#[test]
fn django_discovery_enumerates_settings_star_import_chain() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import *\nfrom .feature import *\n",
            ),
            (
                "/proj/myproject/base.py",
                "from .common import *\nINSTALLED_APPS = []\n",
            ),
            ("/proj/myproject/feature.py", "from .common import *\n"),
            (
                "/proj/myproject/common.py",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False}]\n",
            ),
        ],
    );

    let discovery = compute_project_facts(&db, project);

    let expected = [
        Utf8PathBuf::from("/proj/myproject/base.py"),
        Utf8PathBuf::from("/proj/myproject/common.py"),
        Utf8PathBuf::from("/proj/myproject/feature.py"),
        Utf8PathBuf::from("/proj/myproject/settings.py"),
    ];
    assert_eq!(discovery.file_paths(), expected.as_slice());
}

#[test]
fn settings_sources_includes_semantically_reached_imports_and_excludes_unreachable_imports() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .flags import DEBUG\nif DEBUG:\n    from .unreachable import *\nelse:\n    from .base import *\n",
            ),
            ("/proj/myproject/flags.py", "DEBUG = False\n"),
            ("/proj/myproject/base.py", "INSTALLED_APPS = []\n"),
            ("/proj/myproject/unreachable.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_project_facts(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/base.py"),
            Utf8PathBuf::from("/proj/myproject/flags.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_excludes_import_guarded_by_imported_false_flag() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .flags import DEBUG\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/flags.py", "DEBUG = False\n"),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_project_facts(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/flags.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_includes_import_after_unsupported_guard_touch() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "DEBUG = False\nmaybe_enable(DEBUG)\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_project_facts(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/broken.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_includes_import_after_loop_guard_change() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "DEBUG = False\nfor plugin in PLUGINS:\n    DEBUG = True\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_project_facts(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/broken.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_plain_import_alias_makes_guarded_import_reachable() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "DEBUG = False\nimport flags as DEBUG\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_project_facts(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/broken.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_dedupes_duplicate_import_edges() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import INSTALLED_APPS as FIRST\nfrom .base import INSTALLED_APPS as SECOND\nINSTALLED_APPS = FIRST + SECOND\n",
            ),
            ("/proj/myproject/base.py", "INSTALLED_APPS = ['base']\n"),
        ],
    );

    let discovery = compute_project_facts(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/base.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn unreadable_root_settings_are_dynamic_never_unset() {
    let settings_path = Utf8PathBuf::from("/proj/myproject/settings.py");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(settings_path.clone(), "INSTALLED_APPS = []\n".to_string());

    let mut db = OsTestDatabase::with_file_system(Arc::new(FailingFileSystem {
        inner: fs,
        failure: FileSystemFailure::Read(settings_path),
    }));
    let root = Utf8PathBuf::from("/proj");
    let interpreter = Interpreter::Auto;
    let pythonpath = Vec::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        root.as_path(),
        &interpreter,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = Project::new(
        &db,
        root,
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("myproject.settings").unwrap()),
        pythonpath,
        Vec::new(),
        djls_conf::Settings::default().tagspecs().clone(),
    );
    db.set_project(project);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    assert_eq!(
        settings["installed_apps"]["cases"][0]["dynamic"]["apps"]["evidence"][0]["issue"]["kind"],
        "unreadable"
    );
    assert_eq!(
        settings["templates"]["cases"][0]["dynamic"]["templates"]["evidence"][0]["issue"]["kind"],
        "unreadable"
    );
}

#[test]
fn django_discovery_includes_deduped_unreadable_settings_source() {
    let unreadable = Utf8PathBuf::from("/proj/myproject/unreadable.py");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        "from .base import *\nfrom .unreadable import *\nfrom .base import *\n".to_string(),
    );
    fs.add_file(
        Utf8PathBuf::from("/proj/myproject/base.py"),
        "INSTALLED_APPS = []\n".to_string(),
    );
    fs.add_file(unreadable.clone(), "TEMPLATES = []\n".to_string());

    let mut db = OsTestDatabase::with_file_system(Arc::new(FailingFileSystem {
        inner: fs,
        failure: FileSystemFailure::Read(unreadable.clone()),
    }));
    let root = Utf8PathBuf::from("/proj");
    let interpreter = Interpreter::Auto;
    let pythonpath = Vec::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        root.as_path(),
        &interpreter,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let tag_specs = djls_conf::Settings::default().tagspecs().clone();
    let project = Project::new(
        &db,
        root,
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("myproject.settings").unwrap()),
        pythonpath,
        Vec::new(),
        tag_specs,
    );
    db.set_project(project);

    let settings_sources = ProjectFactsPhase::SettingsSources.run(&db, project);
    assert_eq!(settings_sources.count(), 3);

    let discovery = compute_project_facts(&db, project);

    let expected = [
        Utf8PathBuf::from("/proj/myproject/base.py"),
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        unreadable,
    ];
    assert_eq!(discovery.file_paths(), expected.as_slice());
}

#[test]
fn template_dirs_resolve_settings_module_file() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\n",
        )],
    );

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/templates")]);
}

#[test]
fn template_dirs_follow_supported_nested_insert_and_remove_mutations() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/first', '/proj/removed'], 'APP_DIRS': False}]
TEMPLATES[0]['DIRS'].insert(1, '/proj/inserted')
TEMPLATES[0]['DIRS'].remove('/proj/removed')",
        )],
    );

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(
        dirs,
        [
            Utf8PathBuf::from("/proj/first"),
            Utf8PathBuf::from("/proj/inserted"),
        ]
    );
}

#[test]
fn template_dirs_merge_equivalent_explicit_backend_branches() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/templates/index.html", "index"),
            (
                "/proj/myproject/settings.py",
                "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'OPTIONS': {'context_processors': ['project.context_processors.site']}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'OPTIONS': {'context_processors': ['project.context_processors.site']}}]\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    assert_eq!(settings["templates"]["cases"].as_array().unwrap().len(), 1);
    assert_eq!(
        settings["templates"]["cases"][0]["known"]["backends"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let origins: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.path_buf(&db).clone())
        .collect();
    assert_eq!(origins, [Utf8PathBuf::from("/proj/templates/index.html")]);
}

#[test]
fn template_resolution_earlier_walk_failure_weakens_later_candidate() {
    let settings = "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/first', '/proj/second'], 'APP_DIRS': False}]\n";
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", settings),
            ("/proj/first/other.html", "other"),
            ("/proj/second/base.html", "base"),
        ],
        FileSystemFailure::Walk(Utf8PathBuf::from("/proj/first")),
    );
    let name = TemplateName::new(&db, "base.html".to_string());

    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected the earlier walk failure to make resolution inconclusive");
    };

    assert_eq!(search.name, name);
    assert_eq!(search.possible_origins.len(), 1);
    assert_eq!(
        search.possible_origins[0].path_buf(&db),
        Utf8Path::new("/proj/second/base.html")
    );
}

#[test]
fn template_resolution_retains_candidate_from_partial_walk_as_possible_origin() {
    let settings = "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\n";
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", settings),
            ("/proj/templates/base.html", "base"),
        ],
        FileSystemFailure::PartialWalk(Utf8PathBuf::from("/proj/templates")),
    );
    let name = TemplateName::new(&db, "base.html".to_string());

    let resolution = template_resolution(&db, project);
    assert_eq!(resolution.origins(&db).count(), 1);
    let FindTemplateResult::Inconclusive(search) = resolution.resolve(&db, name) else {
        panic!("expected the partial walk to make its retained candidate uncertain");
    };

    assert_eq!(search.possible_origins.len(), 1);
    assert_eq!(
        search.possible_origins[0].path_buf(&db),
        Utf8Path::new("/proj/templates/base.html")
    );
}

#[test]
fn template_resolution_definite_earlier_candidate_wins_before_later_walk_failure() {
    let settings = "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/first', '/proj/second'], 'APP_DIRS': False}]\n";
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", settings),
            ("/proj/first/base.html", "base"),
            ("/proj/second/other.html", "other"),
        ],
        FileSystemFailure::Walk(Utf8PathBuf::from("/proj/second")),
    );
    let name = TemplateName::new(&db, "base.html".to_string());

    let FindTemplateResult::Found(origin) = template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected the definite earlier candidate to win");
    };

    assert_eq!(origin.path_buf(&db), Utf8Path::new("/proj/first/base.html"));
}

#[test]
fn template_resolution_no_candidate_with_walk_failure_is_inconclusive() {
    let settings = "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\n";
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", settings),
            ("/proj/templates/other.html", "other"),
        ],
        FileSystemFailure::Walk(Utf8PathBuf::from("/proj/templates")),
    );
    let name = TemplateName::new(&db, "missing.html".to_string());

    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected the failed walk to make a missing result inconclusive");
    };

    assert_eq!(search.name, name);
    assert!(search.possible_origins.is_empty());
}

#[test]
fn template_resolution_target_path_conversion_failure_is_inconclusive() {
    let settings = "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\n";
    let target = Utf8PathBuf::from("/proj/templates/base.html");
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", settings),
            ("/proj/templates/base.html", "base"),
        ],
        FileSystemFailure::PathToFile(target),
    );
    let name = TemplateName::new(&db, "base.html".to_string());

    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected the target indexing failure to make resolution inconclusive");
    };

    assert_eq!(search.name, name);
    assert!(search.possible_origins.is_empty());
}

#[test]
fn template_resolution_app_dirs_candidate_walk_failure_is_inconclusive() {
    let settings = "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n";
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", settings),
            ("/proj/blog/__init__.py", ""),
        ],
        FileSystemFailure::Walk(Utf8PathBuf::from("/proj/blog/templates")),
    );
    let name = TemplateName::new(&db, "missing.html".to_string());

    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected the APP_DIRS metadata failure to make resolution inconclusive");
    };

    assert!(search.possible_origins.is_empty());
    assert_eq!(
        template_directories(&db, project)
            .known_roots()
            .collect::<Vec<_>>(),
        [Utf8Path::new("/proj/blog/templates")]
    );
}

#[test]
fn template_dirs_keep_different_explicit_backend_alternatives() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/a/index.html", "a"),
            ("/proj/b/index.html", "b"),
            (
                "/proj/myproject/settings.py",
                "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/a']}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/b']}]\n",
            ),
        ],
    );

    let directories = template_directories(&db, project);
    assert!(directories.configuration_may_omit_roots());

    let origins: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.path_buf(&db).clone())
        .collect();
    assert_eq!(
        origins
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            Utf8PathBuf::from("/proj/a/index.html"),
            Utf8PathBuf::from("/proj/b/index.html"),
        ]
        .into_iter()
        .collect()
    );

    let name = TemplateName::new(&db, "index.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected alternative backend ordering to precede known roots");
    };
    assert_eq!(search.possible_origins.len(), 2);
}

#[test]
fn unknown_backend_before_known_backend_weakens_known_candidate() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/templates/index.html", "index"),
            (
                "/proj/myproject/settings.py",
                "TEMPLATES = [UNKNOWN, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\n",
            ),
        ],
    );
    let name = TemplateName::new(&db, "index.html".to_string());

    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected unknown backend ordering to precede the known root");
    };
    assert_eq!(search.possible_origins.len(), 1);
}

#[test]
fn uncertain_backend_dictionary_before_known_backend_keeps_library_identity_aligned() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/templates/index.html", "{% load custom %}"),
            (
                "/proj/custom_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': UNKNOWN}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
            ),
        ],
    );
    let file = db.file(Utf8Path::new("/proj/templates/index.html"));

    assert!(
        matches!(
            template_environment(&db, project, file).loadable_library_str("custom"),
            LoadableLibraryLookup::Inconclusive(candidates)
                if candidates.iter().any(|library| library.module_name_str() == "custom_tags")
        ),
        "the known second backend should retain its library slot"
    );
}

#[test]
fn missing_template_backend_excludes_directory_and_library_consumers() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/templates/index.html", "index"),
            (
                "/proj/custom_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'DIRS': ['/proj/templates'], 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    assert!(has_case(&settings["templates"], "malformed"));

    let directories = template_directories(&db, project);
    assert!(directories.configuration_may_omit_roots());
    assert_eq!(template_resolution(&db, project).origins(&db).count(), 0);
    assert!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .loadable_library_str("custom")
            .found()
            .is_none()
    );
}

#[test]
fn dynamic_template_backend_excludes_directory_and_library_consumers() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/templates/index.html", "index"),
            (
                "/proj/custom_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nBACKEND = object()\nTEMPLATES = [{'BACKEND': BACKEND, 'DIRS': ['/proj/templates'], 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    assert!(has_case(&settings["templates"], "dynamic"));

    let directories = template_directories(&db, project);
    assert!(directories.configuration_may_omit_roots());
    assert_eq!(template_resolution(&db, project).origins(&db).count(), 0);
    assert!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .loadable_library_str("custom")
            .found()
            .is_none()
    );
}

#[test]
fn template_dirs_treat_unset_templates_as_exact_absence() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[("/proj/myproject/settings.py", "INSTALLED_APPS = []\n")],
    );

    let directories = template_directories(&db, project);

    assert!(!directories.configuration_may_omit_roots());
}

#[test]
fn template_dirs_treat_unresolved_configured_settings_as_unknown() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(&mut db, "myproject.settings", &[]);

    let directories = template_directories(&db, project);

    assert!(directories.configuration_may_omit_roots());
}

#[test]
fn template_dirs_resolve_relative_star_imports() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.prod",
        &[
            ("/proj/django/contrib/auth/__init__.py", ""),
            (
                "/proj/django/contrib/auth/templates/auth/index.html",
                "auth",
            ),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/base.py",
                "INSTALLED_APPS = ['django.contrib.auth']\n",
            ),
            (
                "/proj/myproject/prod.py",
                "from .base import *\nINSTALLED_APPS += ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(
        dirs,
        vec![
            Utf8PathBuf::from("/proj/django/contrib/auth/templates"),
            Utf8PathBuf::from("/proj/blog/templates"),
        ]
    );
}

#[test]
fn template_dirs_resolve_relative_star_imports_from_package_module() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/contrib/auth/__init__.py", ""),
            (
                "/proj/django/contrib/auth/templates/auth/index.html",
                "auth",
            ),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/settings/base.py",
                "INSTALLED_APPS = ['django.contrib.auth']\n",
            ),
            (
                "/proj/myproject/settings/__init__.py",
                "from .base import *\nINSTALLED_APPS += ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(
        dirs,
        vec![
            Utf8PathBuf::from("/proj/django/contrib/auth/templates"),
            Utf8PathBuf::from("/proj/blog/templates"),
        ]
    );
}

#[test]
fn template_dirs_recover_from_star_import_cycle() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "from .settings import *\nINSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/blog/templates")]);
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

    let dirs = complete_template_dirs(&db, project);

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

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/blog/templates")]);
}

#[test]
fn template_dirs_resolve_app_config_class_from_init_module() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/something/__init__.py", ""),
            ("/proj/something/templates/something/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['something.WeirdConfig']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/something/templates")]);
}

#[test]
fn template_dirs_demote_broken_app_config_entry_to_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templates/myapp/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp.apps.MyConfig']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let directories = template_directories(&db, project);

    assert!(directories.configuration_may_omit_roots());
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
        &[Utf8PathBuf::from("/site")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db);

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(dirs, vec![Utf8PathBuf::from("/site/pkg/templates")]);
}

#[test]
fn template_dirs_resolve_bare_namespace_app_entry() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/nsapp/templates/nsapp/index.html", "index"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/nsapp/templates")]);
}

#[test]
fn template_dirs_resolve_namespace_app_portions_in_root_order() {
    let mut db = TestDatabase::new();
    db.add_file("/proj/nsapp/templates/project.html", "project");
    db.add_file("/vendor/nsapp/templates/vendor.html", "vendor");
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
    );
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &Interpreter::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db);

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(
        dirs,
        vec![
            Utf8PathBuf::from("/proj/nsapp/templates"),
            Utf8PathBuf::from("/vendor/nsapp/templates"),
        ]
    );
}

#[test]
fn template_dirs_demote_file_module_app_to_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/app.py", ""),
            ("/proj/app/templates/app/index.html", "index"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['app']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let directories = template_directories(&db, project);

    assert!(directories.configuration_may_omit_roots());
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

    let directories = template_directories(&db, project);

    assert!(directories.configuration_may_omit_roots());
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
            ("/proj/blog/__init__.py", ""),
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

    let custom = TemplateEnvironment::from_project_inventory(libraries)
        .loadable_library_str("custom")
        .found()
        .expect("custom library should be discovered");
    assert_eq!(custom.module_name_str(), "blog.templatetags.custom");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "hello")
    );
    assert_eq!(
        active_builtin_modules(libraries),
        vec![
            "django.template.defaulttags",
            "django.template.defaultfilters",
            "django.template.loader_tags",
        ]
    );
}

#[test]
fn template_libraries_cross_product_divergent_installed_apps_with_templates() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/first/templatetags/__init__.py", ""),
            (
                "/proj/first/templatetags/shared.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/second/templatetags/__init__.py", ""),
            (
                "/proj/second/templatetags/shared.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "if APP_FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nif TEMPLATE_FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/other'], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let LoadableLibraryLookup::Ambiguous(candidates) =
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .loadable_library_str("shared")
    else {
        panic!("divergent app alternatives must retain both library outcomes");
    };
    let modules = candidates
        .iter()
        .map(|library| library.module_name_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        modules,
        ["first.templatetags.shared", "second.templatetags.shared",]
            .into_iter()
            .collect()
    );
}

#[test]
fn unset_templates_is_closed_absence_for_app_libraries() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templatetags/__init__.py", ""),
            (
                "/proj/blog/templatetags/custom.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/myproject/settings.py", "INSTALLED_APPS = ['blog']\n"),
        ],
    );

    assert_eq!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .loadable_library_str("custom"),
        LoadableLibraryLookup::Absent
    );
}

#[test]
fn dynamic_installed_apps_keep_guidance_open_without_template_backends() {
    for templates in ["TEMPLATES = []\n", ""] {
        let mut db = TestDatabase::new();
        let settings = format!("INSTALLED_APPS = [UNKNOWN]\n{templates}");
        let project = project_with_settings(
            &mut db,
            "myproject.settings",
            &[
                ("/proj/crispy/__init__.py", ""),
                ("/proj/crispy/templatetags/__init__.py", ""),
                (
                    "/proj/crispy/templatetags/crispy.py",
                    "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag(): pass\n@register.filter\ndef crispy_filter(value): return value\n",
                ),
                ("/proj/myproject/settings.py", settings.as_str()),
            ],
        );

        let libraries = template_libraries(&db, project);
        assert_eq!(
            TemplateEnvironment::from_project_inventory(libraries)
                .available_app_symbol("crispy_tag", TemplateSymbolKind::Tag),
            TemplateSymbolLookup::Inconclusive,
            "dynamic apps with {templates:?} must not produce definitive tag guidance"
        );
        assert_eq!(
            TemplateEnvironment::from_project_inventory(libraries)
                .available_app_symbol("crispy_filter", TemplateSymbolKind::Filter),
            TemplateSymbolLookup::Inconclusive,
            "dynamic apps with {templates:?} must not produce definitive filter guidance"
        );
        assert_eq!(
            TemplateEnvironment::from_project_inventory(libraries)
                .missing_library(&library_name("crispy")),
            MissingLibraryLookup::Inconclusive,
            "dynamic apps with {templates:?} must not produce definitive library guidance"
        );
    }
}

#[test]
fn template_symbol_lookup_uses_later_definite_available_candidate() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/alpha/__init__.py", ""),
            ("/proj/alpha/templatetags/__init__.py", ""),
            (
                "/proj/alpha/templatetags/alpha.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef shared_tag(): pass\n",
            ),
            ("/proj/zeta/__init__.py", ""),
            ("/proj/zeta/templatetags/__init__.py", ""),
            (
                "/proj/zeta/templatetags/zeta.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef shared_tag(): pass\n",
            ),
            (
                "/proj/project_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'alpha': 'project_tags'}}}]\n",
            ),
        ],
    );

    assert_eq!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .available_app_symbol("shared_tag", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::FoundInApp {
            app: PythonModuleName::parse("zeta").unwrap(),
            load_name: library_name("zeta"),
        },
        "a shadowed earlier candidate must not hide a later definite candidate"
    );
}

#[test]
fn template_libraries_discover_package_templatetags() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/nsapp/__init__.py", ""),
            ("/proj/nsapp/templatetags/__init__.py", ""),
            (
                "/proj/nsapp/templatetags/custom.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let custom = TemplateEnvironment::from_project_inventory(libraries)
        .loadable_library_str("custom")
        .found()
        .expect("package templatetag should be discovered");
    assert_eq!(custom.module_name_str(), "nsapp.templatetags.custom");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "hello")
    );
}

#[test]
fn template_libraries_discover_namespace_package_templatetags() {
    let mut db = TestDatabase::new();
    db.add_file("/proj/nsapp/other.py", "");
    db.add_file("/vendor/nsapp/templatetags/__init__.py", "");
    db.add_file(
        "/vendor/nsapp/templatetags/custom.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n",
    );
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
    );
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &Interpreter::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db);

    let libraries = template_libraries(&db, project);

    let custom = TemplateEnvironment::from_project_inventory(libraries)
        .loadable_library_str("custom")
        .found()
        .expect("namespace package templatetag should be discovered");
    assert_eq!(custom.module_name_str(), "nsapp.templatetags.custom");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "hello")
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
            ("/proj/blog/__init__.py", ""),
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

    let empty = TemplateEnvironment::from_project_inventory(libraries)
        .loadable_library_str("empty")
        .found()
        .unwrap();
    assert_eq!(empty.module_name_str(), "blog.templatetags.empty");
    assert!(empty.symbols().is_empty());
}

#[test]
fn template_libraries_skip_discovered_helpers_without_demoting_inventory() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templatetags/__init__.py", ""),
            ("/proj/blog/templatetags/helpers.py", "VALUE = 1\n"),
            (
                "/proj/blog/templatetags/orphan.py",
                "@register.simple_tag\ndef orphan():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(
        TemplateEnvironment::from_project_inventory(libraries)
            .loadable_library_str("helpers")
            .found()
            .is_none()
    );
    let orphan = TemplateEnvironment::from_project_inventory(libraries)
        .loadable_library_str("orphan")
        .found()
        .expect("symbol-bearing modules are template libraries even without register assignment");
    assert!(
        orphan
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "orphan")
    );
}

#[test]
fn template_libraries_preserve_installed_app_discovery_order_across_failures() {
    let settings = "INSTALLED_APPS = ['first', 'second']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n";
    let files = [
        ("/proj/myproject/settings.py", settings),
        ("/proj/first/__init__.py", ""),
        ("/proj/first/templatetags/__init__.py", ""),
        (
            "/proj/first/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef from_first(): pass\n",
        ),
        ("/proj/second/__init__.py", ""),
        ("/proj/second/templatetags/__init__.py", ""),
        (
            "/proj/second/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef from_second(): pass\n",
        ),
    ];

    let (db, project) = project_with_file_system_failure(
        &files,
        FileSystemFailure::Walk(Utf8PathBuf::from("/proj/first/templatetags")),
    );
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project)).loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "second.templatetags.shared"
    ));

    let (db, project) = project_with_file_system_failure(
        &files,
        FileSystemFailure::Walk(Utf8PathBuf::from("/proj/second/templatetags")),
    );
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project)).loadable_library_str("shared"),
        LoadableLibraryLookup::Inconclusive(candidates)
            if candidates.iter().any(|library| {
                library.module_name_str() == "first.templatetags.shared"
            })
    ));
}

#[test]
fn template_libraries_preserve_installed_app_order_across_source_analysis_failures() {
    let valid_library = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known(): pass\n";
    let backend = "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n";

    let earlier_settings = format!("INSTALLED_APPS = ['first', 'second']\n{backend}");
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", &earlier_settings),
            ("/proj/first/__init__.py", ""),
            ("/proj/first/templatetags/__init__.py", ""),
            ("/proj/first/templatetags/shared.py", valid_library),
            ("/proj/second/__init__.py", ""),
            ("/proj/second/templatetags/__init__.py", ""),
            ("/proj/second/templatetags/shared.py", valid_library),
        ],
        FileSystemFailure::Read(Utf8PathBuf::from("/proj/second/templatetags/shared.py")),
    );
    let LoadableLibraryLookup::Inconclusive(candidates) =
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .loadable_library_str("shared")
    else {
        panic!("the unreadable later candidate should leave the earlier library feasible");
    };
    assert!(matches!(
        candidates.as_slice(),
        [library] if library.module_name_str() == "first.templatetags.shared"
    ));

    let later_settings = format!("INSTALLED_APPS = ['second', 'first']\n{backend}");
    let (db, project) = project_with_file_system_failure(
        &[
            ("/proj/myproject/settings.py", &later_settings),
            ("/proj/first/__init__.py", ""),
            ("/proj/first/templatetags/__init__.py", ""),
            ("/proj/first/templatetags/shared.py", valid_library),
            ("/proj/second/__init__.py", ""),
            ("/proj/second/templatetags/__init__.py", ""),
            ("/proj/second/templatetags/shared.py", valid_library),
        ],
        FileSystemFailure::Read(Utf8PathBuf::from("/proj/second/templatetags/shared.py")),
    );
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project)).loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "first.templatetags.shared"
    ));
}

#[test]
fn template_libraries_recovered_positive_candidate_remains_resolved() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/first/templatetags/__init__.py", ""),
            (
                "/proj/first/templatetags/shared.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/second/templatetags/__init__.py", ""),
            (
                "/proj/second/templatetags/shared.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef known(value):\n    return value\ndef broken(",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['first', 'second']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);
    let LoadableLibraryLookup::Found(library) =
        TemplateEnvironment::from_project_inventory(libraries).loadable_library_str("shared")
    else {
        panic!("the recovered later candidate should remain a known library");
    };
    assert_eq!(library.module_name_str(), "second.templatetags.shared");
    assert!(
        library
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "known")
    );
    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("possibly_hidden", TemplateSymbolKind::Filter),
        TemplateSymbolLookup::Inconclusive
    );
}

#[test]
fn template_libraries_retain_recovered_symbols_with_source_uncertainty() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/blog/templatetags/__init__.py", ""),
            (
                "/proj/blog/templatetags/broken.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef known(value):\n    return value\ndef broken(",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let LoadableLibraryLookup::Found(library) =
        TemplateEnvironment::from_project_inventory(libraries).loadable_library_str("broken")
    else {
        panic!("the recovered module still identifies the same loadable library");
    };
    assert!(
        library
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "known")
    );
    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("possibly_hidden", TemplateSymbolKind::Filter),
        TemplateSymbolLookup::Inconclusive
    );
}

#[test]
fn template_libraries_accept_supported_python_newer_than_ruff_default_target() {
    let mut db = TestDatabase::new();
    let path = Utf8Path::new("/proj/blog/templatetags/modern.py");
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/blog/templatetags/__init__.py", ""),
            (
                path.as_str(),
                "type FilterValue = str\nfrom django import template\nregister = template.Library()\n@register.filter\ndef known(value):\n    return value\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);
    let errors = python_syntax_errors(&db, db.file(path)).expect("file should be Python");

    assert!(
        TemplateEnvironment::from_project_inventory(libraries)
            .loadable_library_str("modern")
            .found()
            .is_some()
    );
    assert!(
        errors
            .iter()
            .any(|error| error.class == PythonSyntaxErrorClass::Unsupported)
    );
    assert!(
        errors
            .iter()
            .all(|error| error.class != PythonSyntaxErrorClass::Ordinary)
    );
}

#[test]
fn invalid_available_identifier_makes_missing_library_inconclusive() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            ("/proj/crispy/templatetags/bad-name.py", "VALUE = 1\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = []\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("missing")),
        MissingLibraryLookup::Inconclusive
    ));
}

#[test]
fn failed_available_candidate_walk_makes_missing_library_inconclusive() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(Utf8PathBuf::from("/proj/django/__init__.py"), String::new());
    fs.add_file(
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        "INSTALLED_APPS = []\nTEMPLATES = []\n".to_string(),
    );
    let mut db = OsTestDatabase::with_file_system(Arc::new(FailingFileSystem {
        inner: fs,
        failure: FileSystemFailure::Walk(Utf8PathBuf::from("/proj")),
    }));
    let root = Utf8PathBuf::from("/proj");
    let interpreter = Interpreter::Auto;
    let pythonpath = Vec::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        root.as_path(),
        &interpreter,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let tag_specs = djls_conf::Settings::default().tagspecs().clone();
    let project = Project::new(
        &db,
        root,
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("myproject.settings").unwrap()),
        pythonpath,
        Vec::new(),
        tag_specs,
    );
    db.set_project(project);

    let libraries = template_libraries(&db, project);
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("missing")),
        MissingLibraryLookup::Inconclusive
    ));
}

#[test]
fn template_libraries_collect_available_uninstalled_templatetags() {
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
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templatetags/__init__.py", ""),
            (
                "/proj/myapp/templatetags/myapp_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef active_tag():\n    pass\n",
            ),
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            (
                "/proj/crispy/templatetags/crispy.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n@register.filter\ndef crispy_filter(value):\n    return value\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let MissingLibraryLookup::FoundInApps(apps) =
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("crispy"))
    else {
        panic!("crispy should be reported as an available-app library candidate");
    };
    assert_eq!(apps.primary().as_str(), "crispy");
    assert_eq!(
        apps.as_slice()
            .iter()
            .map(PythonModuleName::as_str)
            .collect::<Vec<_>>(),
        vec!["crispy"]
    );

    let TemplateSymbolLookup::FoundInApp { app, load_name } =
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("crispy_tag", TemplateSymbolKind::Tag)
    else {
        panic!("crispy_tag should be reported as an available-app tag candidate");
    };
    assert_eq!(app.as_str(), "crispy");
    assert_eq!(load_name.as_str(), "crispy");

    let TemplateSymbolLookup::FoundInApp { app, load_name } =
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("crispy_filter", TemplateSymbolKind::Filter)
    else {
        panic!("crispy_filter should be reported as an available-app filter candidate");
    };
    assert_eq!(app.as_str(), "crispy");
    assert_eq!(load_name.as_str(), "crispy");

    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("myapp_tags")),
        MissingLibraryLookup::Inconclusive,
        "installed app libraries must be subtracted from available candidates"
    );
}

#[test]
fn template_libraries_available_candidates_rerun_after_search_root_revision_bump() {
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
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templatetags/__init__.py", ""),
            (
                "/proj/myapp/templatetags/myapp_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    assert_eq!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .missing_library(&library_name("crispy")),
        MissingLibraryLookup::Absent
    );

    db.add_file("/proj/crispy/__init__.py", "");
    db.add_file("/proj/crispy/templatetags/__init__.py", "");
    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n",
    );
    let root = db
        .files()
        .expect_root(&db, Utf8Path::new("/proj/crispy/templatetags/crispy.py"));
    db.bump_file_root_revision(root);

    let libraries = template_libraries(&db, project);

    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("crispy")),
        MissingLibraryLookup::FoundInApps(_)
    ));
}

#[test]
fn django_discovery_updates_available_template_library_symbols() {
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
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templatetags/__init__.py", ""),
            (
                "/proj/myapp/templatetags/myapp_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            (
                "/proj/crispy/templatetags/crispy.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef old_tag():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    assert_eq!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .available_app_symbol("new_tag", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::Absent
    );

    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n",
    );
    apply_project_discovery(&mut db);

    assert!(matches!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .available_app_symbol("new_tag", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::FoundInApp { .. }
    ));
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
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("missing")),
        MissingLibraryLookup::Inconclusive
    ));
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

    let custom = TemplateEnvironment::from_project_inventory(libraries)
        .loadable_library_str("custom")
        .found()
        .unwrap();
    assert_eq!(custom.module_name_str(), "custom_tags");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "configured")
    );
    let environment = TemplateEnvironment::from_project_inventory(libraries);
    assert!(
        environment
            .contextual_symbol_candidates("configured_filter", TemplateSymbolKind::Filter)
            .iter()
            .any(|candidate| {
                matches!(
                    &candidate.availability,
                    TemplateSymbolAvailability::Builtin { module }
                        if module.as_str() == "custom_builtin"
                )
            })
    );
}

#[test]
fn partial_django_backend_keeps_alias_definitive_until_open_backend_selection() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/custom_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef configured():\n    pass\n",
            ),
            ("/proj/templates/page.html", "{% load custom %}"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}, unknown_key: 'maybe'}]\n",
            ),
        ],
    );
    let file = db.file(Utf8Path::new("/proj/templates/page.html"));

    assert!(matches!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project)).loadable_library_str("custom"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "custom_tags"
    ));
    assert!(matches!(
        template_environment(&db, project, file).loadable_library_str("custom"),
        LoadableLibraryLookup::Inconclusive(candidates)
            if candidates.iter().any(|library| library.module_name_str() == "custom_tags")
    ));
}

#[test]
fn partial_non_django_backend_contributes_open_library_alternative() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'project.backends.CustomTemplates', unknown_key: 'maybe'}]\n",
        )],
    );

    assert_eq!(
        TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
            .loadable_library_str("missing"),
        LoadableLibraryLookup::Inconclusive(Vec::new())
    );
}

#[test]
fn template_libraries_keep_candidate_with_later_backend_uncertainty() {
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
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'project_tags'}}}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': UNKNOWN}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let LoadableLibraryLookup::Inconclusive(candidates) =
        TemplateEnvironment::from_project_inventory(libraries).loadable_library_str("custom")
    else {
        panic!("the open backend alternative should keep lookup inconclusive");
    };
    let custom = candidates
        .into_iter()
        .find(|library| library.module_name_str() == "project_tags")
        .expect("the concrete configured candidate should be retained");
    assert_eq!(custom.module_name_str(), "project_tags");
    assert!(
        custom
            .symbols()
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
            ("/proj/blog/__init__.py", ""),
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

    let custom = TemplateEnvironment::from_project_inventory(libraries)
        .loadable_library_str("custom")
        .found()
        .unwrap();
    assert_eq!(custom.module_name_str(), "project_tags");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "new_tag")
    );
    assert!(
        !custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "old_tag")
    );
    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("old_tag", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::Absent,
        "a configured alias can shadow an installed app library without making that app available"
    );
}

#[test]
fn failed_configured_library_is_inconclusive() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/broken_tags.py", "def broken(:\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'broken': 'broken_tags'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(
        TemplateEnvironment::from_project_inventory(libraries)
            .loadable_library_str("broken")
            .found()
            .is_none()
    );
}

#[test]
fn unknown_configured_alias_keys_suppress_available_app_guidance() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            (
                "/proj/crispy/templatetags/shared.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n@register.filter\ndef crispy_filter(value):\n    return value\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {**UNKNOWN}}}]\n",
            ),
        ],
    );
    let libraries = template_libraries(&db, project);

    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("crispy_tag", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("crispy_filter", TemplateSymbolKind::Filter),
        TemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("shared")),
        MissingLibraryLookup::Inconclusive
    );
}

#[test]
fn exact_alias_after_unknown_keys_remains_authoritative() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            (
                "/proj/crispy/templatetags/shared.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n@register.filter\ndef crispy_filter(value):\n    return value\n",
            ),
            (
                "/proj/project_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {**UNKNOWN, 'shared': 'project_tags'}}}]\n",
            ),
        ],
    );
    let libraries = template_libraries(&db, project);

    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("crispy_tag", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::Absent
    );
    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .available_app_symbol("crispy_filter", TemplateSymbolKind::Filter),
        TemplateSymbolLookup::Absent
    );
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries).loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library) if library.module_name_str() == "project_tags"
    ));
}

#[test]
fn unresolved_configured_alias_shadows_available_app_guidance() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            (
                "/proj/crispy/templatetags/shared.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'missing_tags'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert_eq!(
        TemplateEnvironment::from_project_inventory(libraries)
            .missing_library(&library_name("shared")),
        MissingLibraryLookup::Inconclusive
    );
}

#[test]
fn template_libraries_omit_invalid_configured_alias_and_demote_knowledge() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'broken': 'bad-module'}}}]\n",
        )],
    );

    let libraries = template_libraries(&db, project);

    assert!(
        TemplateEnvironment::from_project_inventory(libraries)
            .loadable_library_str("broken")
            .found()
            .is_none()
    );
}

#[test]
fn template_libraries_retain_missing_configured_alias_without_source() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'missing': 'missing_tags'}}}]\n",
        )],
    );

    let libraries = template_libraries(&db, project);

    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries).loadable_library_str("missing"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "missing_tags" && library.source_file().is_none()
    ));
}

#[test]
fn template_libraries_omit_configured_non_library_module_and_demote_knowledge() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/not_a_library.py", "VALUE = 1\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'not_a_library'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(
        TemplateEnvironment::from_project_inventory(libraries)
            .loadable_library_str("custom")
            .found()
            .is_none()
    );
}

#[test]
fn template_libraries_include_resolved_and_configured_only_libraries() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/good_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef good():\n    pass\n",
            ),
            ("/proj/not_a_library.py", "VALUE = 1\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'good': 'good_tags', 'missing': 'missing_tags', 'invalid': 'bad-module'}, 'builtins': ['not_a_library']}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);
    let active_modules: Vec<_> = TemplateEnvironment::from_project_inventory(libraries)
        .resolved_libraries()
        .into_iter()
        .map(|library| library.module_name_str().to_string())
        .collect();

    assert_eq!(
        active_modules,
        vec![
            "good_tags",
            "missing_tags",
            "django.template.defaulttags",
            "django.template.defaultfilters",
            "django.template.loader_tags",
        ]
    );
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries).loadable_library_str("good"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "good_tags"
    ));
    assert!(matches!(
        TemplateEnvironment::from_project_inventory(libraries).loadable_library_str("missing"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "missing_tags" && library.source_file().is_none()
    ));
    assert!(
        TemplateEnvironment::from_project_inventory(libraries)
            .loadable_library_str("invalid")
            .found()
            .is_none()
    );
}

fn django_facts_golden_fixture() -> (
    OsTestDatabase,
    Project,
    Utf8PathBuf,
    Utf8PathBuf,
    DjangoFactsGolden,
) {
    let corpus = Corpus::require();
    let django_source_root = corpus.root().join("repos/django-5.2");
    assert!(
        django_source_root.join("django/__init__.py").is_file(),
        "pinned Django 5.2 corpus source is missing"
    );

    let workspace = Utf8PathBuf::from_path_buf(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap(),
    )
    .unwrap();
    let project_root = workspace.join("tests/project");
    let golden_path = workspace.join("tests/fixtures/django-facts/django-5.2.json");
    let golden =
        serde_json::from_str(&std::fs::read_to_string(golden_path.as_std_path()).unwrap()).unwrap();

    let mut db = OsTestDatabase::new();
    let interpreter = Interpreter::VenvPath(corpus.root().join("hermetic-no-venv"));
    let pythonpath = vec![django_source_root.clone()];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        project_root.as_path(),
        &interpreter,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = Project::new(
        &db,
        project_root.clone(),
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("djls_test.settings").unwrap()),
        pythonpath,
        Vec::new(),
        djls_conf::Settings::default().tagspecs().clone(),
    );
    db.set_project(project);

    (db, project, project_root, django_source_root, golden)
}

#[test]
fn django_facts_golden_template_dirs_match() {
    let (db, project, project_root, django_source_root, golden) = django_facts_golden_fixture();
    let expected: Vec<_> = golden
        .template_dirs
        .into_iter()
        .map(|path| {
            path.replace("${PROJECT}", project_root.as_str())
                .replace("${SITE_PACKAGES}", django_source_root.as_str())
        })
        .collect();
    let actual: Vec<_> = complete_template_dirs(&db, project)
        .into_iter()
        // APP_DIRS now retains missing candidate roots so detailed walking can distinguish
        // absence from metadata failure; the golden records concrete directories only.
        .filter(|path| db.path_is_dir(path))
        .map(|path| path.to_string())
        .collect();

    assert_eq!(actual, expected);
}

#[test]
fn django_facts_golden_template_libraries_match() {
    let (db, project, _, _, golden) = django_facts_golden_fixture();
    let libraries = template_libraries(&db, project);
    let actual_builtins = active_builtin_modules(libraries);
    assert_eq!(actual_builtins, golden.template_libraries.builtins);

    let environment = TemplateEnvironment::from_project_inventory(libraries);
    let actual_libraries: BTreeMap<_, _> = environment
        .completion_library_names()
        .into_iter()
        .filter_map(|name| {
            let library = environment.loadable_library(&name).found()?;
            Some((
                name.as_str().to_string(),
                library.module_name_str().to_string(),
            ))
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

    for library in TemplateEnvironment::from_project_inventory(libraries).resolved_libraries() {
        let load_name = library.load_name().map(|name| name.as_str().to_string());

        for symbol in library.symbols() {
            symbols.push(GoldenTemplateSymbol {
                kind: symbol.kind,
                name: symbol.name().to_string(),
                load_name: load_name.clone(),
                library_module: library.module_name_str().to_string(),
                module: library.module_name_str().to_string(),
            });
        }
    }

    symbols
}
