use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::PythonEnvironment;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::compute_django_environment;
use djls_project::testing::compute_project_facts;
use djls_project::testing::django_settings;
use djls_project::testing::python_module_evaluation;
use djls_project::testing::python_settings_evaluation;
use djls_project::testing::python_syntax_errors;
use djls_project::*;
use djls_source::CaseSensitivity;
use djls_source::ChangeEvent;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::RootWalk;
use djls_source::SourceChanges;
use djls_source::WalkOptions;
use djls_testing::DjangoFactsGolden;
use djls_testing::GoldenTemplateSymbol;
use djls_testing::OsTestDatabase;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use djls_testing::django_facts_project;
use djls_testing::will_execute_count;
use serde_json::Value;
use serde_json::to_value;

fn library_name(name: &str) -> Result<LibraryName, Box<dyn std::error::Error>> {
    Ok(LibraryName::parse(name)?)
}

fn active_builtin_modules(libraries: &TemplateLibraryCatalog) -> Vec<String> {
    ScopedTemplateLibraries::from_project_inventory(libraries)
        .resolved_libraries()
        .into_iter()
        .filter(|&library| library.load_name().is_none())
        .map(|library| library.module_name().as_str().to_string())
        .collect()
}

fn has_case(value: &Value, kind: &str) -> bool {
    value["cases"].as_array().is_some_and(|cases| {
        cases
            .iter()
            .any(|case| case.as_str() == Some(kind) || case.get(kind).is_some())
    })
}

fn update_project_file(
    db: &mut TestDatabase,
    path: &str,
    source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    db.add_file(path, source)?;
    SourceChanges::new([ChangeEvent::ContentChanged(path.into())]).apply(db);
    Ok(())
}

fn update_settings_file(
    db: &mut TestDatabase,
    source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    update_project_file(db, "/proj/myproject/settings.py", source)
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings = serde_json::to_value(django_settings(&db, project))
        .expect("test value should serialize to JSON");
    let app_cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings = serde_json::to_value(django_settings(&db, project))
        .expect("test value should serialize to JSON");
    let app_cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
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

    let template_cases = settings["templates"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let app_cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
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

    let template_cases = settings["templates"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");

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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "base");
}

#[test]
fn later_named_import_dominates_namespace_wide_syntax_exclusion() {
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
        .file(
            "/proj/myproject/settings.py",
            "from .base import INSTALLED_APPS\n",
        )
        .install(&mut db).expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "base");
}

#[test]
fn star_import_preserves_namespace_wide_syntax_uncertainty() {
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
        .file("/proj/myproject/settings.py", "from .base import *\n")
        .install(&mut db).expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert!(
        has_case(&settings["installed_apps"], "known"),
        "{settings:#}"
    );
    assert!(
        has_case(&settings["installed_apps"], "dynamic"),
        "{settings:#}"
    );
    assert!(
        has_case(&settings["installed_apps"], "unset"),
        "{settings:#}"
    );
    assert!(cases.iter().any(|case| {
        case.pointer("/known/apps/0/value").and_then(Value::as_str) == Some("base")
    }));
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings = serde_json::to_value(django_settings(&db, project))
        .expect("expected JSON value should be a string");
    let errors = python_syntax_errors(&db, db.file(path).expect("settings test file should exist"))
        .expect("file should be Python");

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
        .install(&mut db)
        .expect("settings project fixture should install");

    let _ = django_settings(&db, project);
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        1
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 1);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 1);
}

#[test]
fn repeated_module_members_keep_values_and_invalidate_after_helper_edit() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("settings")
        .file(
            "/proj/settings.py",
            "import base\nINSTALLED_APPS = [base.FIRST, base.SECOND, base.FIRST]\nTEMPLATES = []\n",
        )
        .file("/proj/base.py", "FIRST = 'alpha'\nSECOND = 'beta'\n")
        .install(&mut db)
        .expect("member fixture should install");

    for (first, expected_executions) in [("alpha", 2), ("gamma", 2)] {
        if first == "gamma" {
            update_project_file(
                &mut db,
                "/proj/base.py",
                "FIRST = 'gamma'\nSECOND = 'beta'\n",
            )
            .expect("helper should update");
        }
        let settings = to_value(django_settings(&db, project)).expect("settings should serialize");
        let apps = settings["installed_apps"]["cases"][0]["known"]["apps"]
            .as_array()
            .expect("apps should be known");
        assert_eq!(
            apps.iter()
                .map(|app| app["value"].as_str().expect("app name"))
                .collect::<Vec<_>>(),
            [first, "beta", first],
        );
        assert_eq!(
            will_execute_count(
                &db,
                &event_log.take().expect("events"),
                "evaluate_python_module"
            ),
            expected_executions,
        );
        assert_eq!(
            to_value(django_settings(&db, project)).expect("settings should serialize"),
            settings
        );
        assert_eq!(
            will_execute_count(
                &db,
                &event_log.take().expect("events"),
                "evaluate_python_module"
            ),
            0,
        );
    }
}

#[test]
fn import_chain_cache_tracks_membership_but_not_source_content() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("settings")
        .file("/proj/pkg/__init__.py", "")
        .file(
            "/proj/settings.py",
            "import pkg.child as first\nimport pkg.child as second\nINSTALLED_APPS = [first.APP, second.APP]\n",
        )
        .install(&mut db)
        .expect("import cache fixture should install");
    let missing = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert!(has_case(&missing["installed_apps"], "dynamic"));
    assert_eq!(
        will_execute_count(
            &db,
            &events.take().expect("events"),
            "resolve_chain_from_name"
        ),
        2
    );

    for (app, lookups) in [("alpha", 1), ("beta", 0)] {
        db.add_file("/proj/pkg/child.py", &format!("APP = '{app}'\n"))
            .expect("child should be writable");
        File::sync_path(&mut db, Utf8Path::new("/proj/pkg/child.py"));
        let settings = to_value(django_settings(&db, project)).expect("settings should serialize");
        let apps = settings["installed_apps"]["cases"][0]["known"]["apps"]
            .as_array()
            .expect("known apps");
        assert_eq!(
            apps.iter()
                .map(|value| value["value"].as_str().expect("app name"))
                .collect::<Vec<_>>(),
            [app, app]
        );
        assert_eq!(
            will_execute_count(
                &db,
                &events.take().expect("events"),
                "resolve_chain_from_name"
            ),
            lookups
        );
    }
    db.remove_file("/proj/pkg/child.py")
        .expect("child should be removable");
    File::sync_path(&mut db, Utf8Path::new("/proj/pkg/child.py"));
    let missing = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert!(has_case(&missing["installed_apps"], "dynamic"));
    assert_eq!(
        will_execute_count(
            &db,
            &events.take().expect("events"),
            "resolve_chain_from_name"
        ),
        1
    );
}

#[test]
fn settings_slice_caches_facts_and_import_trace() {
    for demanded in [false, true] {
        let event_log = SalsaEventLog::default();
        let mut db = TestDatabase::with_event_log(event_log.clone());
        let source = format!(
            r"import constants
MESSAGE_TAGS = {{constants.INFO: 'info'}}
INSTALLED_APPS = ['blog']
TEMPLATES = {}
",
            if demanded { "[MESSAGE_TAGS]" } else { "[]" }
        );
        let project = ProjectFixture::new("/proj")
            .django_settings_module("settings")
            .file("/proj/settings.py", &source)
            .file("/proj/constants.py", "INFO = 'level'\n")
            .install(&mut db)
            .expect("settings fixture should install");
        let file = db
            .file(Utf8Path::new("/proj/settings.py"))
            .expect("settings file");
        let first =
            python_settings_evaluation(&db, project, file).expect("settings should evaluate");
        assert_eq!(first.binding("MESSAGE_TAGS").is_some(), demanded);
        assert_eq!(first.dependency_files.len(), 2);
        let events = event_log.take().expect("events should be readable");
        assert_eq!(
            will_execute_count(&db, &events, "evaluate_python_module"),
            2
        );
        assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
        assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);

        for _ in 0..2 {
            assert_eq!(
                python_settings_evaluation(&db, project, file)
                    .expect("cached settings should evaluate"),
                first
            );
        }
        let events = event_log.take().expect("events should be readable");
        for query in [
            "evaluate_python_module",
            "python_module_facts",
            "python_import_trace",
        ] {
            assert_eq!(will_execute_count(&db, &events, query), 0, "{query}");
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "keep multiline Python fixtures inline"
)]
fn settings_slice_preserves_dependencies_and_effect_barriers() {
    for source in [
        r"APPS = ['early']
if FLAG:
    APPS = ['late']
INSTALLED_APPS = APPS
APPS = ['too_late']
TEMPLATES = []
",
        r"if FLAG:
    APPS = ['a']
    BACKENDS = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['one']}]
else:
    APPS = ['b', 'c']
    BACKENDS = []
INSTALLED_APPS = APPS
TEMPLATES = BACKENDS
",
        r"APPS = ['a']
INSTALLED_APPS = APPS
APPS += ['b']
TEMPLATES = []
",
        r"INSTALLED_APPS = ['a']
ALIAS = INSTALLED_APPS
ALIAS.append('b')
TEMPLATES = []
",
        r"INSTALLED_APPS = ['a']
ALIAS = INSTALLED_APPS
UNUSED = opaque(ALIAS)
TEMPLATES = []
",
        r"INSTALLED_APPS = ['a']
LEFT = RIGHT = INSTALLED_APPS
TEMPLATES = []
",
        r"from .base import INSTALLED_APPS
TEMPLATES = []
",
        "from .base import *\n",
        r"from .dynamic import *
if FLAG:
    UNUSED = ['a']
else:
    UNUSED = ['b']
",
        r"from .base import *
from . import poison as UNUSED_IMPORT
from pathlib import Path
TEMPLATES = [{'DIRS': [Path('templates')]}]
",
        r"import myproject.child
from myproject import INSTALLED_APPS
TEMPLATES = myproject.child.TEMPLATES
",
        r"from .cycle import *
INSTALLED_APPS = ['local']
",
        r"from .settings import *
INSTALLED_APPS = ['local']
TEMPLATES = []
",
        r"INSTALLED_APPS = ['a']
TEMPLATES = []
def broken(
",
        r"INSTALLED_APPS = ['a']
if FLAG:
    del INSTALLED_APPS
TEMPLATES = []
",
        r"INSTALLED_APPS = ['a']
try:
    INSTALLED_APPS += ['b']
except Exception:
    INSTALLED_APPS = ['c']
TEMPLATES = []
",
        // An unread aggregate can bridge alias degradation back into a demanded value.
        r"INSTALLED_APPS = ['a']
A = ['other']
UNUSED = [A, INSTALLED_APPS]
for item in unknown:
    A.append('changed')
TEMPLATES = []
",
        r"import pathlib
UNUSED = [pathlib, pathlib.Path]
unknown().attr = 1
TEMPLATES = [{'DIRS': [pathlib.Path('templates')]}]
INSTALLED_APPS = []
",
        r"import pathlib
UNUSED = {'result': opaque(pathlib)}
TEMPLATES = [{'DIRS': [pathlib.Path('templates')]}]
INSTALLED_APPS = []
",
        r"import pathlib as UNUSED
UNUSED = {'key': unknown.attr}
unknown().attr = 1
INSTALLED_APPS = []
TEMPLATES = []
",
        r"INSTALLED_APPS = ['a']
UNUSED = [INSTALLED_APPS]
UNUSED = {'key': unknown.attr}
for item in unknown:
    INSTALLED_APPS.append('b')
TEMPLATES = []
",
        r"UNUSED = {'key': unknown.attr}
for item in unknown:
    UNUSED = ['nested']
INSTALLED_APPS = []
TEMPLATES = []
",
        r"UNUSED = {'key': unknown.attr}
if FLAG:
    from .dynamic import *
INSTALLED_APPS = []
TEMPLATES = []
",
        r"INSTALLED_APPS = ['a']
UNUSED = {'key': [*INSTALLED_APPS]}
opaque(INSTALLED_APPS)
TEMPLATES = []
",
    ] {
        let mut db = TestDatabase::new();
        let project = ProjectFixture::new("/proj")
            .django_settings_module("myproject.settings")
            .file(
                "/proj/myproject/__init__.py",
                "INSTALLED_APPS = ['parent']\n",
            )
            .file("/proj/myproject/settings.py", source)
            .file(
                "/proj/myproject/base.py",
                r"__all__ = ['INSTALLED_APPS', 'TEMPLATES']
INSTALLED_APPS = ['base']
TEMPLATES = []
",
            )
            .file(
                "/proj/myproject/dynamic.py",
                r"__all__ = unknown()
INSTALLED_APPS = ['dynamic']
",
            )
            .file(
                "/proj/myproject/poison.py",
                r"import pathlib
pathlib.Path = unknown
",
            )
            .file("/proj/myproject/child.py", "TEMPLATES = []\n")
            .file(
                "/proj/myproject/cycle.py",
                r"from .settings import *
TEMPLATES = []
",
            )
            .install(&mut db)
            .expect("fixture should install");
        let file = db
            .file(Utf8Path::new("/proj/myproject/settings.py"))
            .expect("fixture file should exist");
        let sliced =
            python_settings_evaluation(&db, project, file).expect("settings should evaluate");
        let full = python_module_evaluation(&db, project, file).expect("module should evaluate");
        for name in ["INSTALLED_APPS", "TEMPLATES"] {
            assert_eq!(sliced.binding(name), full.binding(name), "{name}: {source}");
        }
        assert_eq!(
            sliced.namespace_unknowns, full.namespace_unknowns,
            "{source}"
        );
        assert_eq!(sliced.dependency_files, full.dependency_files, "{source}");
        assert_eq!(sliced.imports, full.imports, "{source}");
        assert_eq!(sliced.mutations, full.mutations, "{source}");
    }
}

#[test]
fn settings_slice_recomputes_discarded_aggregate_and_imported_leaves() {
    let mut db = TestDatabase::new();
    let source = r"from . import constants
UNUSED = [constants.APP]
INSTALLED_APPS = ['initial']
TEMPLATES = []
";
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", source)
        .file("/proj/myproject/constants.py", "APP = 'first'\n")
        .install(&mut db)
        .expect("fixture should install");
    let file = db
        .file(Utf8Path::new("/proj/myproject/settings.py"))
        .expect("fixture file");
    assert!(
        python_settings_evaluation(&db, project, file)
            .expect("settings should evaluate")
            .binding("UNUSED")
            .is_none()
    );
    let initial = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert_eq!(
        initial["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "initial"
    );
    let source_count = ProjectFactsPhase::SettingsSources.run(&db, project).count();
    assert_eq!(source_count, 3);
    update_settings_file(&mut db, &source.replace("['initial']", "UNUSED"))
        .expect("settings should update");
    let changed = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert_eq!(
        changed["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "first"
    );
    update_project_file(&mut db, "/proj/myproject/constants.py", "APP = 'second'\n")
        .expect("constant should update");
    let changed = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert_eq!(
        changed["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "second"
    );
    assert_eq!(
        ProjectFactsPhase::SettingsSources.run(&db, project).count(),
        source_count
    );
}

#[test]
fn settings_slice_recomputes_when_unused_code_becomes_a_dependency() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    let source = r"if FLAG:
    UNUSED = ['left']
else:
    UNUSED = ['right', 'extra']
INSTALLED_APPS = ['initial']
TEMPLATES = []
";
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", source)
        .install(&mut db)
        .expect("fixture should install");
    let file = db
        .file(Utf8Path::new("/proj/myproject/settings.py"))
        .expect("fixture file should exist");
    assert!(
        python_settings_evaluation(&db, project, file)
            .expect("settings should evaluate")
            .binding("UNUSED")
            .is_none()
    );
    let initial = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert_eq!(
        initial["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "initial"
    );
    assert_eq!(
        ProjectFactsPhase::SettingsSources.run(&db, project).count(),
        1
    );
    assert_eq!(
        will_execute_count(
            &db,
            &events.take().expect("events should be readable"),
            "evaluate_python_module"
        ),
        1
    );

    update_settings_file(&mut db, &source.replace("['initial']", "UNUSED"))
        .expect("settings should update");
    let changed = to_value(django_settings(&db, project)).expect("settings should serialize");
    let mut alternatives = changed["installed_apps"]["cases"]
        .as_array()
        .expect("app cases")
        .iter()
        .map(|case| {
            case["known"]["apps"]
                .as_array()
                .expect("known apps")
                .iter()
                .map(|app| app["value"].as_str().expect("app string"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    alternatives.sort();
    assert_eq!(alternatives, [vec!["left"], vec!["right", "extra"]]);
    assert!(
        python_settings_evaluation(&db, project, file)
            .expect("settings should evaluate")
            .binding("UNUSED")
            .is_some()
    );
    assert_eq!(
        ProjectFactsPhase::SettingsSources.run(&db, project).count(),
        1
    );
    assert_eq!(
        will_execute_count(
            &db,
            &events.take().expect("events should be readable"),
            "evaluate_python_module"
        ),
        1
    );
}

#[test]
fn settings_slice_keeps_full_cycle_and_recovered_syntax_results() {
    for source in [
        r"from .settings import *
INSTALLED_APPS = ['local']
UNUSED = ['retained']
",
        r"UNUSED = ['retained']
INSTALLED_APPS = ['local']
def broken(
",
    ] {
        let mut db = TestDatabase::new();
        let project = ProjectFixture::new("/proj")
            .django_settings_module("myproject.settings")
            .file("/proj/myproject/__init__.py", "")
            .file("/proj/myproject/settings.py", source)
            .install(&mut db)
            .expect("fixture should install");
        let file = db
            .file(Utf8Path::new("/proj/myproject/settings.py"))
            .expect("fixture file should exist");
        let sliced =
            python_settings_evaluation(&db, project, file).expect("settings should evaluate");
        assert!(sliced.binding("UNUSED").is_some());
        assert_eq!(
            sliced,
            python_module_evaluation(&db, project, file).expect("module should evaluate")
        );
        let settings = to_value(django_settings(&db, project)).expect("settings should serialize");
        assert_eq!(
            settings["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
            "local"
        );
    }
}

#[test]
fn settings_slice_discovers_imports_added_to_a_previously_skipped_region() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r"if True:
    UNUSED = ['ignored']
TEMPLATES = []
",
        )
        .file(
            "/proj/myproject/dependency.py",
            "INSTALLED_APPS = ['imported']\n",
        )
        .install(&mut db)
        .expect("fixture should install");
    assert_eq!(
        ProjectFactsPhase::SettingsSources.run(&db, project).count(),
        1
    );
    assert!(has_case(
        &to_value(django_settings(&db, project)).expect("settings should serialize")["installed_apps"],
        "unset"
    ));

    update_settings_file(
        &mut db,
        r"if True:
    from .dependency import INSTALLED_APPS
TEMPLATES = []
",
    )
    .expect("settings should update");
    assert_eq!(
        ProjectFactsPhase::SettingsSources.run(&db, project).count(),
        3
    );
    let imported = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert_eq!(
        imported["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "imported"
    );
    update_project_file(
        &mut db,
        "/proj/myproject/dependency.py",
        "INSTALLED_APPS = ['edited']\n",
    )
    .expect("dependency should update");
    let edited = to_value(django_settings(&db, project)).expect("settings should serialize");
    assert_eq!(
        edited["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "edited"
    );
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let _ = django_settings(&db, project);
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    update_project_file(
        &mut db,
        "/proj/myproject/leaf.py",
        "INSTALLED_APPS = ['a']\n# comment only\n",
    )
    .expect("settings project file should be updated");
    let _ = django_settings(&db, project);
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_eq!(will_execute_count(&db, &events, "parse_python_file"), 1);
    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        0
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 0);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 0);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 0);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 0);
}

#[test]
fn value_change_backdates_dependency_projection() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", "INSTALLED_APPS = ['a']")
        .install(&mut db)
        .expect("settings project fixture should install");

    let _ = django_settings(&db, project);
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    update_settings_file(&mut db, "INSTALLED_APPS = ['b']")
        .expect("Django settings file should be updated");
    let _ = django_settings(&db, project);
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        1
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 1);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 0);
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let before =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    update_settings_file(&mut db, "INSTALLED_APPS = ['a']\nfrom .extra import *")
        .expect("Django settings file should be updated");
    let sources = ProjectFactsPhase::SettingsSources.run(&db, project);
    let after =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_eq!(after, before);
    // The `from .extra import *` edit makes settings.py load its parent package
    // `myproject/__init__.py` (a distinct file) plus `myproject.extra`, so three
    // modules evaluate and all three files are dependency sources.
    assert_eq!(sources.count(), 3);
    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        3
    );
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 1);
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 0);
}

#[test]
fn origin_shift_changes_values_but_backdates_dependency_projection() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", "INSTALLED_APPS = ['a']\n")
        .install(&mut db)
        .expect("settings project fixture should install");

    let before =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    update_settings_file(&mut db, "\nINSTALLED_APPS = ['a']\n")
        .expect("Django settings file should be updated");
    let after =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_ne!(after, before);
    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        1
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 1);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 0);
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
        .install(&mut db)
        .expect("settings project fixture should install");
    let _unreachable = db
        .file(Utf8Path::new("/proj/myproject/unreachable.py"))
        .expect("settings test file should exist");

    let _ = django_settings(&db, project);
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    update_project_file(&mut db, "/proj/myproject/unreachable.py", "VALUE = 'new'\n")
        .expect("settings project file should be updated");
    let _ = django_settings(&db, project);
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        0
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 0);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 0);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 0);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 0);
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let sources = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_eq!(
        settings["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "local"
    );
    // The self-cycle settings module also loads its distinct parent package
    // `myproject/__init__.py`, which becomes a second dependency source and is
    // projected once.
    assert_eq!(sources.count(), 2);
    let evaluations = will_execute_count(&db, &events, "evaluate_python_module");
    assert!((1..=12).contains(&evaluations));
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
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
        .install(&mut db).expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| {
            known["apps"][0]["value"]
                .as_str()
                .expect("expected JSON field should be an array")
        })
        .collect::<BTreeSet<_>>();

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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| {
            known["apps"][0]["value"]
                .as_str()
                .expect("expected JSON field should be an array")
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(known, ["imported", "local"].into_iter().collect());
    assert!(!cases.iter().any(|case| case == "unset"));
}

#[test]
fn exact_all_conditional_setting_preserves_local_setting_alternative() {
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
            "if ENABLED:\n    INSTALLED_APPS = ['imported']\n__all__ = ['INSTALLED_APPS']\n",
        )
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let cases = settings["installed_apps"]["cases"]
        .as_array()
        .expect("expected JSON field should be an array");
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| {
            known["apps"][0]["value"]
                .as_str()
                .expect("expected JSON field should be an array")
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(known, ["imported", "local"].into_iter().collect());
    assert!(!has_case(&settings["installed_apps"], "dynamic"));
    assert!(!has_case(&settings["installed_apps"], "unset"));
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
        .install(&mut db)
        .expect("settings project fixture should install");

    let settings =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let sources = ProjectFactsPhase::SettingsSources.run(&db, project);
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    assert_eq!(
        settings["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "local"
    );
    assert!(has_case(&settings["templates"], "known"));
    // The two-file cycle also loads its distinct parent package
    // `myproject/__init__.py`, a third dependency source.
    assert_eq!(sources.count(), 3);
    let evaluations = will_execute_count(&db, &events, "evaluate_python_module");
    assert!((2..=24).contains(&evaluations));
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
}

#[test]
fn child_topology_change_backdates_values_projection() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/child.py", "X = 'v1'\n")
        .file("/proj/myproject/other.py", "Y = 'y'\n")
        .file(
            "/proj/myproject/settings.py",
            "import myproject.child\nINSTALLED_APPS = ['a']\n",
        )
        .install(&mut db)
        .expect("settings project fixture should install");

    let before =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    // Adding an import to the loaded child changes the recursive object/topology
    // effect (a new attached coordinate and a new source edge) without touching
    // any lexical settings the root module exposes.
    update_project_file(
        &mut db,
        "/proj/myproject/child.py",
        "X = 'v1'\nimport myproject.other\n",
    )
    .expect("settings project file should be updated");
    let after =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    // The root module's exposed settings never change, so `django_settings`
    // backdates even though the recursive core and its dependency projection
    // both recompute against the new child topology.
    assert_eq!(after, before);
    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        3
    );
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 1);
    // `python_module_facts` recomputes but produces an equal projection, so it
    // backdates and `django_settings` never re-runs.
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 0);
}

#[test]
fn parent_package_init_edit_invalidates_dotted_consumer() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/pkg/__init__.py", "APPS = ['blog']\n")
        .file("/proj/myproject/pkg/sub.py", "X = 1\n")
        .file(
            "/proj/myproject/settings.py",
            "import myproject.pkg.sub\nINSTALLED_APPS = myproject.pkg.APPS\n",
        )
        .install(&mut db)
        .expect("settings project fixture should install");

    let before =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    update_project_file(
        &mut db,
        "/proj/myproject/pkg/__init__.py",
        "APPS = ['news']\n",
    )
    .expect("settings project file should be updated");
    let after =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    // Editing the dotted parent package `myproject/pkg/__init__.py` invalidates
    // the settings consumer that reads `myproject.pkg.APPS`: the parent and the
    // consumer recompute and the changed lexical value flows to settings.
    assert_ne!(after, before);
    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        2
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 1);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 1);
}

#[test]
fn external_module_body_edit_never_reaches_the_consumer() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/.venv/lib/python3.12/site-packages/ext/__init__.py",
            "VALUE = 'old'\n",
        )
        .file(
            "/proj/myproject/settings.py",
            "import ext\nINSTALLED_APPS = ['a']\n",
        )
        .install(&mut db)
        .expect("settings project fixture should install");

    let before =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    update_project_file(
        &mut db,
        "/proj/.venv/lib/python3.12/site-packages/ext/__init__.py",
        "VALUE = 'new'\n",
    )
    .expect("settings project file should be updated");
    let after =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    // The external body is never parsed, evaluated, or recorded as a dependency,
    // so editing it leaves every projection cold and the settings unchanged.
    assert_eq!(after, before);
    assert_eq!(will_execute_count(&db, &events, "parse_python_file"), 0);
    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        0
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 0);
    assert_eq!(will_execute_count(&db, &events, "python_import_trace"), 0);
    assert_eq!(will_execute_count(&db, &events, "settings_sources"), 0);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 0);
}

#[test]
fn search_path_winner_change_recomputes_module_reads() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    // `/extra` is a separate project-code root registered alongside `/proj`; the
    // first-party `/proj` root outranks it, so a later `/proj/mod.py` becomes the
    // resolution winner and changes the imported module's object identity.
    db.add_file("/extra/keep.py", "")
        .expect("settings test file should be added");
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &PythonEnvironment::Auto,
        &[Utf8PathBuf::from("/extra")],
    );
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .file("/proj/myproject/__init__.py", "")
        .file("/extra/mod.py", "APPS = ['extra']\n")
        .file(
            "/proj/myproject/settings.py",
            "import mod\nINSTALLED_APPS = mod.APPS\n",
        )
        .install(&mut db)
        .expect("settings project fixture should install");

    let before =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    drop(ProjectFactsPhase::SettingsSources.run(&db, project));
    drop(
        event_log
            .take()
            .expect("settings event log should be readable"),
    );

    db.add_file("/proj/mod.py", "APPS = ['root']\n")
        .expect("settings test file should be added");
    SourceChanges::new([ChangeEvent::BecameVisible("/proj/mod.py".into())]).apply(&mut db);

    let after =
        to_value(django_settings(&db, project)).expect("test value should serialize to JSON");
    let events = event_log
        .take()
        .expect("settings event log should be readable");

    // The new first-party `/proj/mod.py` outranks `/extra/mod.py`, so the
    // imported module's object identity changes and the module-attribute read
    // recomputes from `extra` to `root`.
    assert_eq!(
        before["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "extra"
    );
    assert_eq!(
        after["installed_apps"]["cases"][0]["known"]["apps"][0]["value"],
        "root"
    );
    assert_eq!(
        will_execute_count(&db, &events, "evaluate_python_module"),
        2
    );
    assert_eq!(will_execute_count(&db, &events, "python_module_facts"), 1);
    assert_eq!(will_execute_count(&db, &events, "django_settings"), 1);
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

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.inner.case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.inner.path_exists_case_sensitive(path, prefix)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        self.inner.walk_root(root, options)
    }
}

#[test]
fn readable_unreadable_rescans_recompute_ancestors_once_and_retain_dependency() {
    let events = SalsaEventLog::default();
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
    let mut db = OsTestDatabase::with_file_system_and_event_log(
        Arc::new(ToggleReadFileSystem {
            inner,
            toggled_path: leaf_path,
            readable: Arc::clone(&readable),
        }),
        [Utf8PathBuf::from("/proj")],
        events.clone(),
    );
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .python_environment(PythonEnvironment::Auto)
        .install(&mut db)
        .expect("settings project fixture should build");

    let _ = django_settings(&db, project);
    // settings.py loads `.leaf` and its distinct parent package
    // `myproject/__init__.py`, so three files are dependency sources.
    assert_eq!(
        ProjectFactsPhase::SettingsSources.run(&db, project).count(),
        3
    );
    events
        .take()
        .expect("settings event log should be readable");

    for next_readable in [false, true] {
        readable.store(next_readable, Ordering::SeqCst);
        SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);
        let _ = django_settings(&db, project);
        assert_eq!(
            ProjectFactsPhase::SettingsSources.run(&db, project).count(),
            3
        );
        let transition_events = events
            .take()
            .expect("settings event log should be readable");

        assert_eq!(
            will_execute_count(&db, &transition_events, "evaluate_python_module"),
            2
        );
        assert_eq!(
            will_execute_count(&db, &transition_events, "python_module_facts"),
            1
        );
        assert_eq!(
            will_execute_count(&db, &transition_events, "python_import_trace"),
            1
        );
        assert_eq!(
            will_execute_count(&db, &transition_events, "django_settings"),
            1
        );
        assert_eq!(
            will_execute_count(&db, &transition_events, "settings_sources"),
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
                    other @ (RootWalk::Missing | RootWalk::File(_) | RootWalk::Inaccessible(_)) => {
                        other
                    }
                }
            }
            FileSystemFailure::Read(_)
            | FileSystemFailure::Walk(_)
            | FileSystemFailure::PartialWalk(_)
            | FileSystemFailure::PathToFile(_) => self.inner.walk_root(root, options),
        }
    }
}

fn project_with_settings(
    db: &mut TestDatabase,
    settings_module: &str,
    files: &[(&str, &str)],
) -> Result<Project, Box<dyn std::error::Error>> {
    let mut fixture = ProjectFixture::new("/proj").django_settings_module(settings_module);
    for (path, source) in files {
        fixture = fixture.file(*path, *source);
    }
    Ok(fixture.install(db)?)
}

fn project_with_file_system_failure(
    files: &[(&str, &str)],
    failure: FileSystemFailure,
) -> Result<(OsTestDatabase, Project), Box<dyn std::error::Error>> {
    let mut fs = InMemoryFileSystem::new();
    for (path, source) in files {
        fs.add_file(Utf8PathBuf::from(*path), (*source).to_string());
    }

    let mut db = OsTestDatabase::with_file_system(
        Arc::new(FailingFileSystem { inner: fs, failure }),
        [Utf8PathBuf::from("/proj")],
    );
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .python_environment(PythonEnvironment::Auto)
        .install(&mut db)?;

    Ok((db, project))
}

fn complete_template_dirs(db: &dyn ProjectDb, project: Project) -> Vec<Utf8PathBuf> {
    let directories = template_directories(db, project);
    assert!(!directories.settings_cases_may_omit_roots());
    directories
        .known_roots()
        .map(Utf8Path::to_path_buf)
        .collect()
}

fn apply_project_discovery(db: &mut TestDatabase) -> Result<(), io::Error> {
    run_django_discovery(db)
        .map_err(io::Error::other)?
        .map(drop)
        .ok_or_else(|| io::Error::other("project should be configured before discovery"))
}

fn project_requiring_environment_application(
    db: &mut TestDatabase,
) -> Result<Project, Box<dyn std::error::Error>> {
    Ok(ProjectFixture::new("/proj")
        .file(
            "/proj/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'extras.tags'}}}]\n",
        )
        .file("/vendor/extras/__init__.py", "")
        .file(
            "/vendor/extras/tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom(): pass\n",
        )
        .django_settings_module("settings")
        .pythonpath("/vendor")
        .python_environment(PythonEnvironment::Auto)
        .search_paths(SearchPaths::default())
        .register_roots(false)
        .install(db)?)
}

#[test]
fn django_discovery_run_matches_explicit_phase_sequence() {
    let library_path = Utf8Path::new("/vendor/extras/tags.py");
    let original_source = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom(): pass\n";
    let updated_source = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom(value): pass\n";

    let mut sequenced = TestDatabase::new();
    let sequenced_project = project_requiring_environment_application(&mut sequenced)
        .expect("environment-application project fixture should build");
    let sequenced_library = sequenced
        .file(library_path)
        .expect("sequenced settings test file should exist");
    assert_eq!(
        sequenced_library
            .try_source(&sequenced)
            .expect("test library should have readable source")
            .as_str(),
        original_source
    );
    let sequenced_revision_before = sequenced_library.revision(&sequenced);
    sequenced
        .add_file(library_path.as_str(), updated_source)
        .expect("sequenced template library should be updated");

    let environment = compute_django_environment(&sequenced, sequenced_project)
        .expect("all Django environment phases should assemble");
    apply_django_environment(&mut sequenced, environment);
    let expected = compute_project_facts(&sequenced, sequenced_project);
    apply_project_facts(&mut sequenced, &expected);

    assert_eq!(
        sequenced_library
            .try_source(&sequenced)
            .expect("expected JSON value should be a string")
            .as_str(),
        updated_source
    );
    assert_eq!(
        sequenced_library.revision(&sequenced),
        sequenced_revision_before + 1
    );

    let mut synchronous = TestDatabase::new();
    let synchronous_project = project_requiring_environment_application(&mut synchronous)
        .expect("environment-application project fixture should build");
    let synchronous_library = synchronous
        .file(library_path)
        .expect("synchronous settings test file should exist");
    assert_eq!(
        synchronous_library
            .try_source(&synchronous)
            .expect("expected JSON value should be a string")
            .as_str(),
        original_source
    );
    let synchronous_revision_before = synchronous_library.revision(&synchronous);
    synchronous
        .add_file(library_path.as_str(), updated_source)
        .expect("synchronous template library should be updated");

    let actual = run_django_discovery(&mut synchronous)
        .expect("all Django environment phases should assemble")
        .expect("project should be configured");

    assert_eq!(
        synchronous_library
            .try_source(&synchronous)
            .expect("expected JSON value should be a string")
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
            synchronous
                .file(path)
                .expect("synchronous settings test file should exist")
                .try_source(&synchronous),
            sequenced
                .file(path)
                .expect("sequenced settings test file should exist")
                .try_source(&sequenced),
            "synchronized source outcome differs for {path}"
        );
    }
}

#[test]
fn django_discovery_run_applies_environment_before_computing_facts() {
    let mut db = TestDatabase::new();
    let project = project_requiring_environment_application(&mut db)
        .expect("environment-application project fixture should build");

    assert!(project.search_paths(&db).iter().next().is_none());

    let facts = run_django_discovery(&mut db)
        .expect("all Django environment phases should assemble")
        .expect("project should be configured");

    assert_eq!(
        project
            .search_paths(&db)
            .iter()
            .map(SearchPath::path)
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
    db.add_file(path.as_str(), "before\n")
        .expect("settings test file should be added");
    let file = db.file(path).expect("settings test file should exist");
    let source_before = file.try_source(&db);
    let revision_before = file.revision(&db);

    db.add_file(path.as_str(), "after\n")
        .expect("settings test file should be added");

    assert_eq!(run_django_discovery(&mut db), Ok(None));
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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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

    let mut db = OsTestDatabase::with_file_system(
        Arc::new(FailingFileSystem {
            inner: fs,
            failure: FileSystemFailure::Read(settings_path),
        }),
        [Utf8PathBuf::from("/proj")],
    );
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .python_environment(PythonEnvironment::Auto)
        .install(&mut db)
        .expect("settings project fixture should build");

    let settings =
        to_value(django_settings(&db, project)).expect("test Python module name should be valid");
    assert_eq!(
        settings["installed_apps"]["cases"][0]["dynamic"]["evidence"][0]["issue"]["kind"],
        "unreadable"
    );
    assert_eq!(
        settings["templates"]["cases"][0]["dynamic"]["evidence"][0]["issue"]["kind"],
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

    let mut db = OsTestDatabase::with_file_system(
        Arc::new(FailingFileSystem {
            inner: fs,
            failure: FileSystemFailure::Read(unreadable.clone()),
        }),
        [Utf8PathBuf::from("/proj")],
    );
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .python_environment(PythonEnvironment::Auto)
        .install(&mut db)
        .expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
fn template_settings_projection_merges_branches_differing_only_in_context_processors() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/templates/index.html", "{% load shared %}"),
            (
                "/proj/custom_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'custom_tags'}, 'context_processors': ['project.context_processors.site']}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'custom_tags'}, 'context_processors': ['project.context_processors.admin']}}]\n",
            ),
        ],
    ).expect("settings project fixture should build");

    let settings = serde_json::to_value(django_settings(&db, project))
        .expect("test value should serialize to JSON");
    assert_eq!(
        settings["templates"]["cases"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2,
        "source settings must retain context processor alternatives"
    );

    assert_eq!(
        complete_template_dirs(&db, project),
        [Utf8PathBuf::from("/proj/templates")]
    );
    let origins: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.path_buf(&db).clone())
        .collect();
    assert_eq!(origins, [Utf8PathBuf::from("/proj/templates/index.html")]);

    let file = db
        .file(Utf8Path::new("/proj/templates/index.html"))
        .expect("template fixture should exist in the test database");
    let libraries = scoped_template_libraries(&db, project, file);
    assert_eq!(
        libraries.library_chains(&["shared"]).len(),
        1,
        "context processors must not duplicate the consumed library alternative"
    );
    assert_eq!(
        libraries
            .loadable_library_str("shared")
            .found()
            .expect("projected backend should provide the configured library")
            .module_name_str(),
        "custom_tags"
    );
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
    )
    .expect("file-system failure project fixture should build");
    let name = TemplateName::new(&db, "base.html".to_string());

    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the earlier walk failure should make resolution inconclusive");

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
    )
    .expect("file-system failure project fixture should build");
    let name = TemplateName::new(&db, "base.html".to_string());

    let resolution = template_resolution(&db, project);
    assert_eq!(resolution.origins(&db).count(), 1);
    let search = match resolution.resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the partial walk should make its retained candidate uncertain");

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
    )
    .expect("file-system failure project fixture should build");
    let name = TemplateName::new(&db, "base.html".to_string());

    let origin = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Found(origin) => Some(origin),
        TemplateResolutionResult::DoesNotExist(_) | TemplateResolutionResult::Inconclusive(_) => {
            None
        }
    }
    .expect("the definite earlier candidate should win");

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
    )
    .expect("file-system failure project fixture should build");
    let name = TemplateName::new(&db, "missing.html".to_string());

    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the failed walk should make a missing result inconclusive");

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
    )
    .expect("file-system failure project fixture should build");
    let name = TemplateName::new(&db, "base.html".to_string());

    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the target indexing failure should make resolution inconclusive");

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
    )
    .expect("file-system failure project fixture should build");
    let name = TemplateName::new(&db, "missing.html".to_string());

    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the APP_DIRS metadata failure should make resolution inconclusive");

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
    ).expect("settings project fixture should build");

    let directories = template_directories(&db, project);
    assert!(directories.settings_cases_may_omit_roots());

    let origins: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.path_buf(&db).clone())
        .collect();
    assert_eq!(
        origins.into_iter().collect::<BTreeSet<_>>(),
        [
            Utf8PathBuf::from("/proj/a/index.html"),
            Utf8PathBuf::from("/proj/b/index.html"),
        ]
        .into_iter()
        .collect()
    );

    let name = TemplateName::new(&db, "index.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("alternative backend ordering should precede known roots");
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
    ).expect("settings project fixture should build");
    let name = TemplateName::new(&db, "index.html".to_string());

    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("unknown backend ordering should precede the known root");
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
    ).expect("settings project fixture should build");
    let file = db
        .file(Utf8Path::new("/proj/templates/index.html"))
        .expect("settings test file should exist");

    assert!(
        matches!(
            scoped_template_libraries(&db, project, file).loadable_library_str("custom"),
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
    ).expect("settings project fixture should build");

    let settings = serde_json::to_value(django_settings(&db, project))
        .expect("test value should serialize to JSON");
    assert!(has_case(&settings["templates"], "malformed"));

    let directories = template_directories(&db, project);
    assert!(directories.settings_cases_may_omit_roots());
    assert_eq!(template_resolution(&db, project).origins(&db).count(), 0);
    assert!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
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
    ).expect("settings project fixture should build");

    let settings = serde_json::to_value(django_settings(&db, project))
        .expect("test value should serialize to JSON");
    assert!(has_case(&settings["templates"], "dynamic"));

    let directories = template_directories(&db, project);
    assert!(directories.settings_cases_may_omit_roots());
    assert_eq!(template_resolution(&db, project).origins(&db).count(), 0);
    assert!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
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
    )
    .expect("settings project fixture should build");

    let directories = template_directories(&db, project);

    assert!(!directories.settings_cases_may_omit_roots());
}

#[test]
fn template_dirs_treat_unresolved_configured_settings_as_unknown() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(&mut db, "myproject.settings", &[])
        .expect("settings project fixture should build");

    let directories = template_directories(&db, project);

    assert!(directories.settings_cases_may_omit_roots());
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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
                "from pathlib import Path\nBASE_DIR = Path(__file__).parent.parent\nINSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE_DIR / 'templates'], 'APP_DIRS': True}]\n",
            ),
        ],
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

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
    ).expect("settings project fixture should build");

    let directories = template_directories(&db, project);

    assert!(directories.settings_cases_may_omit_roots());
}

#[test]
fn template_dirs_resolve_apps_from_site_packages_search_path() {
    let mut db = TestDatabase::new();
    db.add_file("/site/pkg/__init__.py", "")
        .expect("settings test file should be added");
    db.add_file("/site/pkg/templates/pkg/index.html", "index")
        .expect("settings test file should be added");
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['pkg']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
    ).expect("settings test file should be added");
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &PythonEnvironment::Auto,
        &[Utf8PathBuf::from("/site")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db)
        .expect("settings project fixture should install");

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
    ).expect("settings project fixture should build");

    let dirs = complete_template_dirs(&db, project);

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/nsapp/templates")]);
}

#[test]
fn template_dirs_resolve_namespace_app_portions_in_root_order() {
    let mut db = TestDatabase::new();
    db.add_file("/proj/nsapp/templates/project.html", "project")
        .expect("settings test file should be added");
    db.add_file("/vendor/nsapp/templates/vendor.html", "vendor")
        .expect("settings test file should be added");
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
    ).expect("settings test file should be added");
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &PythonEnvironment::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db)
        .expect("settings project fixture should install");

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
    ).expect("settings project fixture should build");

    let directories = template_directories(&db, project);

    assert!(directories.settings_cases_may_omit_roots());
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
    ).expect("settings project fixture should build");

    let directories = template_directories(&db, project);

    assert!(directories.settings_cases_may_omit_roots());
}

#[test]
fn template_library_catalog_discover_app_templatetags_and_builtins() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let custom = ScopedTemplateLibraries::from_project_inventory(libraries)
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
fn template_library_catalog_cross_product_divergent_installed_apps_with_templates() {
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
    ).expect("settings project fixture should build");

    let candidates = match ScopedTemplateLibraries::from_project_inventory(
        template_library_catalog(&db, project),
    )
    .loadable_library_str("shared")
    {
        LoadableLibraryLookup::Ambiguous(candidates) => Some(candidates),
        LoadableLibraryLookup::Found(_)
        | LoadableLibraryLookup::Inconclusive(_)
        | LoadableLibraryLookup::Absent => None,
    }
    .expect("divergent app alternatives should retain both library outcomes");
    let modules = candidates
        .iter()
        .map(|library| library.module_name_str())
        .collect::<BTreeSet<_>>();
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
    )
    .expect("settings project fixture should build");

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
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
        ).expect("settings project fixture should build");

        let libraries = template_library_catalog(&db, project);
        assert_eq!(
            ScopedTemplateLibraries::from_project_inventory(libraries)
                .available_in_app_symbol("crispy_tag", TemplateSymbolKind::Tag),
            AppTemplateSymbolLookup::Inconclusive,
            "dynamic apps with {templates:?} must not produce definitive tag guidance"
        );
        assert_eq!(
            ScopedTemplateLibraries::from_project_inventory(libraries)
                .available_in_app_symbol("crispy_filter", TemplateSymbolKind::Filter),
            AppTemplateSymbolLookup::Inconclusive,
            "dynamic apps with {templates:?} must not produce definitive filter guidance"
        );
        assert_eq!(
            ScopedTemplateLibraries::from_project_inventory(libraries).missing_library(
                &library_name("crispy").expect("test library name should be valid")
            ),
            MissingTemplateLibraryLookup::Inconclusive,
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
    ).expect("settings project fixture should build");

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .available_in_app_symbol("shared_tag", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::FoundInApp {
            app: PythonModuleName::parse("zeta").expect("test Python module name should be valid"),
            load_name: library_name("zeta").expect("test library name should be valid"),
        },
        "a shadowed earlier candidate must not hide a later definite candidate"
    );
}

#[test]
fn template_library_catalog_discover_package_templatetags() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let custom = ScopedTemplateLibraries::from_project_inventory(libraries)
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
fn template_library_catalog_discover_namespace_package_templatetags() {
    let mut db = TestDatabase::new();
    db.add_file("/proj/nsapp/other.py", "")
        .expect("settings test file should be added");
    db.add_file("/vendor/nsapp/templatetags/__init__.py", "")
        .expect("settings test file should be added");
    db.add_file(
        "/vendor/nsapp/templatetags/custom.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n",
    ).expect("settings test file should be added");
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
    ).expect("settings test file should be added");
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &PythonEnvironment::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db)
        .expect("settings project fixture should install");

    let libraries = template_library_catalog(&db, project);

    let custom = ScopedTemplateLibraries::from_project_inventory(libraries)
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
fn template_library_catalog_include_empty_registered_modules() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let empty = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("empty")
        .found()
        .expect("settings fixture should have the expected shape");
    assert_eq!(empty.module_name_str(), "blog.templatetags.empty");
    assert!(empty.symbols().is_empty());
}

#[test]
fn template_library_catalog_skip_discovered_helpers_without_demoting_inventory() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    assert!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .loadable_library_str("helpers")
            .found()
            .is_none()
    );
    let orphan = ScopedTemplateLibraries::from_project_inventory(libraries)
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
fn template_library_catalog_preserve_installed_app_discovery_order_across_failures() {
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
    )
    .expect("file-system failure project fixture should build");
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project)).loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "second.templatetags.shared"
    ));

    let (db, project) = project_with_file_system_failure(
        &files,
        FileSystemFailure::Walk(Utf8PathBuf::from("/proj/second/templatetags")),
    )
    .expect("file-system failure project fixture should build");
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project)).loadable_library_str("shared"),
        LoadableLibraryLookup::Inconclusive(candidates)
            if candidates.iter().any(|library| {
                library.module_name_str() == "first.templatetags.shared"
            })
    ));
}

#[test]
fn template_library_catalog_preserve_installed_app_order_across_source_analysis_failures() {
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
    )
    .expect("file-system failure project fixture should build");
    let candidates = match ScopedTemplateLibraries::from_project_inventory(
        template_library_catalog(&db, project),
    )
    .loadable_library_str("shared")
    {
        LoadableLibraryLookup::Inconclusive(candidates) => Some(candidates),
        LoadableLibraryLookup::Found(_)
        | LoadableLibraryLookup::Ambiguous(_)
        | LoadableLibraryLookup::Absent => None,
    }
    .expect("the unreadable later candidate should leave the earlier library feasible");
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
    )
    .expect("file-system failure project fixture should build");
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project)).loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "first.templatetags.shared"
    ));
}

#[test]
fn template_library_catalog_recovered_positive_candidate_remains_resolved() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);
    let library = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("shared")
        .found()
        .expect("the recovered later candidate should remain a known library");
    assert_eq!(library.module_name_str(), "second.templatetags.shared");
    assert!(
        library
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "known")
    );
    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .available_in_app_symbol("possibly_hidden", TemplateSymbolKind::Filter),
        AppTemplateSymbolLookup::Inconclusive
    );
}

#[test]
fn template_library_catalog_retain_recovered_symbols_with_source_uncertainty() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let library = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("broken")
        .found()
        .expect("the recovered module should still identify the same loadable library");
    assert!(
        library
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "known")
    );
    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .available_in_app_symbol("possibly_hidden", TemplateSymbolKind::Filter),
        AppTemplateSymbolLookup::Inconclusive
    );
}

#[test]
fn template_library_catalog_accept_supported_python_newer_than_ruff_default_target() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);
    let errors = python_syntax_errors(&db, db.file(path).expect("settings test file should exist"))
        .expect("file should be Python");

    assert!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
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
    )
    .expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .missing_library(&library_name("missing").expect("test library name should be valid")),
        MissingTemplateLibraryLookup::Inconclusive
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
    let mut db = OsTestDatabase::with_file_system(
        Arc::new(FailingFileSystem {
            inner: fs,
            failure: FileSystemFailure::Walk(Utf8PathBuf::from("/proj")),
        }),
        [Utf8PathBuf::from("/proj")],
    );
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .python_environment(PythonEnvironment::Auto)
        .install(&mut db)
        .expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .missing_library(&library_name("missing").expect("test library name should be valid")),
        MissingTemplateLibraryLookup::Inconclusive
    ));
}

#[test]
fn template_library_catalog_collects_templatetags_available_outside_installed_apps() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let apps = match ScopedTemplateLibraries::from_project_inventory(libraries)
        .missing_library(&library_name("crispy").expect("test library name should be valid"))
    {
        MissingTemplateLibraryLookup::FoundInApps(apps) => Some(apps),
        MissingTemplateLibraryLookup::Absent | MissingTemplateLibraryLookup::Inconclusive => None,
    }
    .expect("crispy should be reported as an available-in-app library candidate");
    assert_eq!(apps.primary().as_str(), "crispy");
    assert_eq!(
        apps.as_slice()
            .iter()
            .map(PythonModuleName::as_str)
            .collect::<Vec<_>>(),
        vec!["crispy"]
    );

    let (app, load_name) = match ScopedTemplateLibraries::from_project_inventory(libraries)
        .available_in_app_symbol("crispy_tag", TemplateSymbolKind::Tag)
    {
        AppTemplateSymbolLookup::FoundInApp { app, load_name } => Some((app, load_name)),
        AppTemplateSymbolLookup::Absent | AppTemplateSymbolLookup::Inconclusive => None,
    }
    .expect("crispy_tag should be reported as an available-in-app tag candidate");
    assert_eq!(app.as_str(), "crispy");
    assert_eq!(load_name.as_str(), "crispy");

    let (app, load_name) = match ScopedTemplateLibraries::from_project_inventory(libraries)
        .available_in_app_symbol("crispy_filter", TemplateSymbolKind::Filter)
    {
        AppTemplateSymbolLookup::FoundInApp { app, load_name } => Some((app, load_name)),
        AppTemplateSymbolLookup::Absent | AppTemplateSymbolLookup::Inconclusive => None,
    }
    .expect("crispy_filter should be reported as an available-in-app filter candidate");
    assert_eq!(app.as_str(), "crispy");
    assert_eq!(load_name.as_str(), "crispy");

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries).missing_library(
            &library_name("myapp_tags").expect("test library name should be valid")
        ),
        MissingTemplateLibraryLookup::Inconclusive,
        "installed app libraries must be subtracted from available candidates"
    );
}

#[test]
fn template_library_catalog_available_candidates_rerun_after_search_root_revision_bump() {
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
    ).expect("settings project fixture should build");

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .missing_library(&library_name("crispy").expect("test library name should be valid")),
        MissingTemplateLibraryLookup::Absent
    );

    db.add_file("/proj/crispy/__init__.py", "")
        .expect("settings test file should be added");
    db.add_file("/proj/crispy/templatetags/__init__.py", "")
        .expect("settings test file should be added");
    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n",
    ).expect("settings test file should be added");
    let root = db
        .files()
        .expect_root(&db, Utf8Path::new("/proj/crispy/templatetags/crispy.py"));
    db.bump_file_root_revision(root);

    let libraries = template_library_catalog(&db, project);

    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .missing_library(&library_name("crispy").expect("test library name should be valid")),
        MissingTemplateLibraryLookup::FoundInApps(_)
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
    ).expect("settings project fixture should build");

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .available_in_app_symbol("new_tag", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::Absent
    );

    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n",
    ).expect("settings test file should be added");
    apply_project_discovery(&mut db).expect("configured project discovery should succeed");

    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .available_in_app_symbol("new_tag", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::FoundInApp { .. }
    ));
}

#[test]
fn template_library_catalog_demote_unresolved_app_to_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = ['missing']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )],
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .missing_library(&library_name("missing").expect("test library name should be valid")),
        MissingTemplateLibraryLookup::Inconclusive
    ));
}

#[test]
fn template_library_catalog_include_options_libraries_and_builtins() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let custom = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("custom")
        .found()
        .expect("settings fixture should have the expected shape");
    assert_eq!(custom.module_name_str(), "custom_tags");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "configured")
    );
    let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(libraries);
    assert!(
        scoped_libraries
            .scoped_symbol_candidates("configured_filter", TemplateSymbolKind::Filter)
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
    ).expect("settings project fixture should build");
    let file = db
        .file(Utf8Path::new("/proj/templates/page.html"))
        .expect("settings test file should exist");

    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project)).loadable_library_str("custom"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "custom_tags"
    ));
    assert!(matches!(
        scoped_template_libraries(&db, project, file).loadable_library_str("custom"),
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
    ).expect("settings project fixture should build");

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .loadable_library_str("missing"),
        LoadableLibraryLookup::Inconclusive(Vec::new())
    );
}

#[test]
fn template_library_catalog_keep_candidate_with_later_backend_uncertainty() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let candidates = match ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("custom")
    {
        LoadableLibraryLookup::Inconclusive(candidates) => Some(candidates),
        LoadableLibraryLookup::Found(_)
        | LoadableLibraryLookup::Ambiguous(_)
        | LoadableLibraryLookup::Absent => None,
    }
    .expect("the open backend alternative should keep lookup inconclusive");
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
fn template_library_catalog_options_override_app_library_load_name() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    let custom = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("custom")
        .found()
        .expect("settings fixture should have the expected shape");
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
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .available_in_app_symbol("old_tag", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::Absent,
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    assert!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .loadable_library_str("broken")
            .found()
            .is_none()
    );
}

#[test]
fn unknown_configured_alias_keys_suppress_available_in_app_guidance() {
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
    ).expect("settings project fixture should build");
    let libraries = template_library_catalog(&db, project);

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .available_in_app_symbol("crispy_tag", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .available_in_app_symbol("crispy_filter", TemplateSymbolKind::Filter),
        AppTemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .missing_library(&library_name("shared").expect("test library name should be valid")),
        MissingTemplateLibraryLookup::Inconclusive
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
    ).expect("settings project fixture should build");
    let libraries = template_library_catalog(&db, project);

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .available_in_app_symbol("crispy_tag", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::Absent
    );
    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .available_in_app_symbol("crispy_filter", TemplateSymbolKind::Filter),
        AppTemplateSymbolLookup::Absent
    );
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(libraries).loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library) if library.module_name_str() == "project_tags"
    ));
}

#[test]
fn unresolved_configured_alias_shadows_available_in_app_guidance() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    assert_eq!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .missing_library(&library_name("shared").expect("test library name should be valid")),
        MissingTemplateLibraryLookup::Inconclusive
    );
}

#[test]
fn template_library_catalog_omit_invalid_configured_alias_and_demote_knowledge() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'broken': 'bad-module'}}}]\n",
        )],
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    assert!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .loadable_library_str("broken")
            .found()
            .is_none()
    );
}

#[test]
fn template_library_catalog_retain_missing_configured_alias_without_source() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'missing': 'missing_tags'}}}]\n",
        )],
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(libraries).loadable_library_str("missing"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "missing_tags" && library.source_file().is_none()
    ));
}

#[test]
fn template_library_catalog_omit_configured_non_library_module_and_demote_knowledge() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);

    assert!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .loadable_library_str("custom")
            .found()
            .is_none()
    );
}

#[test]
fn template_library_catalog_include_resolved_and_configured_only_libraries() {
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
    ).expect("settings project fixture should build");

    let libraries = template_library_catalog(&db, project);
    let active_modules: Vec<_> = ScopedTemplateLibraries::from_project_inventory(libraries)
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
        ScopedTemplateLibraries::from_project_inventory(libraries).loadable_library_str("good"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "good_tags"
    ));
    assert!(matches!(
        ScopedTemplateLibraries::from_project_inventory(libraries).loadable_library_str("missing"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "missing_tags" && library.source_file().is_none()
    ));
    assert!(
        ScopedTemplateLibraries::from_project_inventory(libraries)
            .loadable_library_str("invalid")
            .found()
            .is_none()
    );
}

#[test]
fn django_facts_golden_template_dirs_match() {
    let (db, project, project_root, django_source_root) =
        django_facts_project("tests/project", "djls_test.settings")
            .expect("Django facts project should build");
    let golden: DjangoFactsGolden = serde_json::from_str(include_str!(
        "../../../tests/fixtures/django-facts/django-5.2.json"
    ))
    .expect("Django facts golden should parse");
    assert!(
        project
            .search_paths(&db)
            .iter()
            .any(|path| path == &SearchPath::SitePackages(django_source_root.clone()))
    );
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
fn django_facts_golden_template_library_catalog_matches() {
    let (db, project, _, _) = django_facts_project("tests/project", "djls_test.settings")
        .expect("Django facts project should build");
    let golden: DjangoFactsGolden = serde_json::from_str(include_str!(
        "../../../tests/fixtures/django-facts/django-5.2.json"
    ))
    .expect("Django facts golden should parse");
    let libraries = template_library_catalog(&db, project);
    let actual_builtins = active_builtin_modules(libraries);
    assert_eq!(actual_builtins, golden.template_library_catalog.builtins);

    let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(libraries);
    let actual_libraries: BTreeMap<_, _> = scoped_libraries
        .completion_library_names()
        .into_iter()
        .filter_map(|name| {
            let library = scoped_libraries.loadable_library(&name).found()?;
            Some((
                name.as_str().to_string(),
                library.module_name_str().to_string(),
            ))
        })
        .collect();
    assert_eq!(actual_libraries, golden.template_library_catalog.libraries);

    let mut actual_symbols = comparable_symbols(libraries);
    let mut expected_symbols = golden.template_library_catalog.symbols;
    actual_symbols.sort();
    expected_symbols.sort();
    assert_eq!(actual_symbols, expected_symbols);
}

fn comparable_symbols(libraries: &TemplateLibraryCatalog) -> Vec<GoldenTemplateSymbol> {
    let mut symbols = Vec::new();

    for library in ScopedTemplateLibraries::from_project_inventory(libraries).resolved_libraries() {
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

#[test]
fn bundled_django_selection_uses_project_metadata_before_oldest_lts() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
    fs.add_file(root.join("pyproject.toml"), "[project]\ndependencies = ['Django>=6.0,<7', 'django-stubs==5.2.0']\n[project.optional-dependencies]\ntest = ['Django==5.2.1']\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nversion = '0.1.0'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '6.1.1'\nsource = { registry = 'https://pypi.org/simple' }\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    assert_eq!(
        bundled_django_version(&fs, root, Some(DjangoVersion::Django52)),
        Some(DjangoVersion::Django52)
    );
    // A stale lock must not override the current project requirement.
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '5.2.3'\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.remove_file(&root.join("uv.lock"));
    fs.add_file(
        root.join("poetry.lock"),
        "[[package]]\nname = 'Django'\nversion = '6.1.0'\ngroups = ['main']\noptional = false\n"
            .into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
}

#[test]
fn bundled_django_reads_pylock_and_checks_declared_constraints() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("pylock.toml"),
        "lock-version = '1.0'\ncreated-by = 'pip'\n\
         [[packages]]\nname = 'django-stubs'\nversion = '5.2.0'\n\
         [[packages]]\nname = 'django'\nversion = '6.1.1'\n"
            .into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.add_file(root.join("requirements.txt"), "Django>=6.0,<6.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60),
        "a stale pylock must not override requirements.txt"
    );
    fs.remove_file(&root.join("requirements.txt"));
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '5.2.17'\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52),
        "tool-native lockfiles take precedence over pylock exports"
    );
    fs.remove_file(&root.join("uv.lock"));
    fs.add_file(
        root.join("pylock.toml"),
        "lock-version = '1.0'\ncreated-by = 'pip'\n\
         [[packages]]\nname = 'django'\nversion = '7.0.0'\n"
            .into(),
    );
    assert_eq!(bundled_django_version(&fs, root, None), None);
}

#[test]
fn bundled_django_reads_requirements_includes_constraints_and_exact_patches() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("requirements.txt"),
        "-r requirements/base.txt\n-c constraints.txt\n".into(),
    );
    fs.add_file(
        root.join("requirements/base.txt"),
        "--requirement=../requirements.txt\nDjango[argon2]>=6.0 # project dependency\n".into(),
    );
    fs.add_file(
        root.join("constraints.txt"),
        "Django==6.1.0 \\\n    --hash=sha256:example\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.add_file(root.join("constraints.txt"), "Django==5.1.9\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        None,
        "known incompatible requirements must not silently select an LTS"
    );
}

#[test]
fn bundled_django_reads_pdm_and_pipenv_runtime_locks() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    for (name, content) in [
        (
            "pdm.lock",
            "[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['default']\n[[package]]\nname = 'django'\nversion = '5.2.17'\ngroups = ['test']\n",
        ),
        (
            "Pipfile.lock",
            r#"{"default":{"django":{"version":"==6.1.1"}},"develop":{"django":{"version":"==5.2.17"}}}"#,
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join(name), content.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django61),
            "{name}"
        );
        fs.add_file(root.join("requirements.txt"), "Django>=6.0,<6.1\n".into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django60),
            "stale {name}"
        );
    }
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("Pipfile.lock"),
        r#"{"develop":{"django":{"version":"==6.1.1"}}}"#.into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
    fs.add_file(root.join("Pipfile.lock"), "not JSON".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
    fs.add_file(
        root.join("pdm.lock"),
        "[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['default']\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
}

#[test]
fn bundled_django_reads_legacy_poetry_constraints() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    for (declaration, expected) in [
        ("'^6.0'", Some(DjangoVersion::Django60)),
        ("'~6.1'", Some(DjangoVersion::Django61)),
        ("'6.1.*'", Some(DjangoVersion::Django61)),
        ("'6.0.3'", Some(DjangoVersion::Django60)),
        ("'^5.1'", Some(DjangoVersion::Django52)),
        ("'~5.1'", None),
        ("'^5.1 || ~6.1'", Some(DjangoVersion::Django52)),
        ("'>=6.0 <6.1'", Some(DjangoVersion::Django60)),
        ("'>= 6.0 < 6.1 || >= 7'", Some(DjangoVersion::Django60)),
        (
            "{version = '^6.1', python = '>=3.12 <3.13'}",
            Some(DjangoVersion::Django61),
        ),
        (
            "{version = '^6.1', extras = ['argon2']}",
            Some(DjangoVersion::Django61),
        ),
        (
            "{version = '^6.1', optional = true}",
            Some(DjangoVersion::Django52),
        ),
        (
            "{version = '^6.1', platform = 'linux', markers = \"sys_platform == 'win32'\"}",
            Some(DjangoVersion::Django52),
        ),
        (
            "[{version = '~6.0', python = '<3.12'}, {version = '^6.1', python = '>=3.12'}]",
            Some(DjangoVersion::Django60),
        ),
        (
            "[{version = '~6.0', markers = \"sys_platform == 'win32'\"}, {version = '^6.1', markers = \"sys_platform != 'win32'\"}]",
            Some(DjangoVersion::Django60),
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("pyproject.toml"), format!("[tool.poetry.dependencies]\nDjango = {declaration}\n[tool.poetry.group.test.dependencies]\nDjango = '5.2.*'\n"));
        assert_eq!(
            bundled_django_version(&fs, root, None),
            expected,
            "{declaration}"
        );
    }
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("pyproject.toml"), "[project]\ndependencies = ['Django>=6.0']\n[tool.poetry.dependencies]\ndjango = '^5.2 || ~6.1'\n".into());
    fs.add_file(
        root.join("poetry.lock"),
        "[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['main']\noptional = false\n"
            .into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61),
        "intersect declarations without collapsing Poetry alternatives"
    );
}

#[test]
fn bundled_django_reads_setup_cfg_runtime_requirements_and_markers() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("setup.cfg"), "[metadata]\nname = example\n[options]\ninstall_requires =\n    django-stubs==5.2.0\n    Django>=6.0,<6.1; python_version < '3.12'\n    Django>=6.1; python_version >= '3.12' # newer Python\n[options.extras_require]\ntest = Django==5.2.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.add_file(root.join("requirements.txt"), "Django>=6.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.remove_file(&root.join("requirements.txt"));
    fs.add_file(
        root.join("setup.cfg"),
        "[options]\ninstall_requires = Django==6.1.0\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
}

#[test]
fn bundled_django_handles_universal_locks_and_unusable_metadata() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("uv.lock"), "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django', version = '6.1.1', marker = \"sys_platform == 'win32'\" }, { name = 'django', version = '6.0.8', marker = \"sys_platform != 'win32'\" }]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n[[package]]\nname = 'django'\nversion = '6.0.8'\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '5.1.9'\n".into(),
    );
    assert_eq!(bundled_django_version(&fs, root, None), None);
    fs.add_file(root.join("uv.lock"), "invalid toml [".into());
    fs.add_file(
        root.join("requirements.in"),
        "Django @ https://example.invalid/django.whl\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
}

#[test]
fn bundled_django_constraints_do_not_create_runtime_requirements() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("requirements.txt"),
        "Django>=6.0; sys_platform == 'win32'\n-c constraints.txt\n".into(),
    );
    fs.add_file(root.join("constraints.txt"), "Django<6.0\n".into());
    assert_eq!(bundled_django_version(&fs, root, None), None);

    fs.add_file(root.join("requirements.txt"), "-c constraints.txt\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52),
        "a constraints-only file must not establish a Django requirement"
    );
    fs.add_file(root.join("constraints.txt"), "Django>=6.0,<6.1\n".into());
    fs.add_file(root.join("uv.lock"), "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{name = 'django'}]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60),
        "runtime lock evidence activates constraints even when the pinned version is stale"
    );
    fs.add_file(root.join("constraints.txt"), "Django>=7\n".into());
    assert_eq!(bundled_django_version(&fs, root, None), None);
}

#[test]
fn bundled_django_uv_uses_only_marker_feasible_runtime_reachability() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("requirements.txt"),
        "Django>=6.0; sys_platform == 'win32'\nDjango>=6.1; sys_platform != 'win32'\n".into(),
    );
    fs.add_file(root.join("uv.lock"), "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'middle' }]\n[package.dev-dependencies]\ndev = [{ name = 'django', version = '5.2.1' }]\n[package.optional-dependencies]\nfeature = [{ name = 'django', version = '5.2.1' }]\n[[package]]\nname = 'middle'\nversion = '1.0'\ndependencies = [{ name = 'project' }, { name = 'django', version = '6.1.1', marker = \"sys_platform != 'win32'\" }, { name = 'django', version = '6.0.8', marker = \"sys_platform == 'win32'\" }]\n[[package]]\nname = 'django'\nversion = '5.2.1'\n[[package]]\nname = 'django'\nversion = '6.0.8'\n[[package]]\nname = 'django'\nversion = '6.1.1'\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60),
        "transitive runtime edges are followed, cycles terminate, and dev/optional edges stay disabled"
    );
}

#[test]
fn bundled_django_ignores_non_runtime_poetry_lock_entries() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("poetry.lock"), "[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['dev']\noptional = false\n[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['main']\noptional = true\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
}

#[test]
fn bundled_django_does_not_invent_runtime_lock_applicability() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    for (filename, source) in [
        (
            "Pipfile.lock",
            r#"{"default":{"django":{"version":"==6.1.1","markers":"sys_platform == 'linux' and sys_platform == 'win32'"}}}"#,
        ),
        (
            "poetry.lock",
            "[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['main']\nmarkers = \"extra == 'web'\"\n",
        ),
        (
            "pylock.toml",
            "[[packages]]\nname = 'django'\nversion = '6.1.1'\nmarker = \"'web' in extras\"\n",
        ),
        (
            "pdm.lock",
            "[[package]]\nname = 'django'\nversion = '6.1.1'\n[metadata]\ngroups = ['default', 'dev']\n",
        ),
        (
            "uv.lock",
            "[[package]]\nname = 'project'\nsource = { virtual = '.' }\n[package.dev-dependencies]\ndev = [{name = 'helper'}]\n[[package]]\nname = 'helper'\nsource = { editable = 'helper' }\ndependencies = [{name = 'django'}]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n",
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join(filename), source.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django52),
            "{filename}: {source}"
        );
    }
}

#[test]
fn bundled_django_constraint_include_modes_follow_each_directive() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("requirements.txt"), "-c constraints.txt\n".into());
    fs.add_file(root.join("constraints.txt"), "-r runtime.txt\n".into());
    fs.add_file(root.join("runtime.txt"), "Django>=6.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.add_file(
        root.join("requirements.txt"),
        "-r runtime.txt\n-c runtime.txt\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
}

#[test]
fn bundled_django_lock_markers_must_match_the_declaration_environment() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    for (filename, source) in [
        (
            "poetry.lock",
            "[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['main','dev']\nmarkers = {main = \"sys_platform == 'linux'\", dev = \"sys_platform == 'win32'\"}\n[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['main']\n",
        ),
        (
            "uv.lock",
            "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{name = 'django', version = '6.0.8', marker = \"sys_platform == 'linux'\"}, {name = 'django', version = '6.1.1'}]\n[[package]]\nname = 'django'\nversion = '6.0.8'\n[[package]]\nname = 'django'\nversion = '6.1.1'\n",
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            root.join("requirements.txt"),
            "Django>=6.0; sys_platform == 'win32'\nDjango>=6.1; sys_platform != 'win32'\n".into(),
        );
        fs.add_file(root.join(filename), source.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django61),
            "{filename}"
        );
    }
}

#[test]
fn bundled_django_uv_only_activates_explicit_dependency_extras() {
    use djls_conf::DjangoVersion;
    use djls_project::bundled_django_version;
    use djls_source::InMemoryFileSystem;

    let root = Utf8Path::new("/project");
    for (extra, expected) in [
        ("", DjangoVersion::Django52),
        (", extra = ['web']", DjangoVersion::Django61),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("uv.lock"), format!("[[package]]\nname = 'project'\nsource = {{ virtual = '.' }}\ndependencies = [{{name = 'helper'{extra}}}]\n[[package]]\nname = 'helper'\nversion = '1.0'\n[package.optional-dependencies]\nweb = [{{name = 'django'}}]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n"));
        assert_eq!(bundled_django_version(&fs, root, None), Some(expected));
    }
}

#[test]
fn bundled_django_url_requirements_activate_constraints() {
    use djls_conf::DjangoVersion;
    let root = Utf8Path::new("/project");
    for (constraint, expected) in [
        ("", Some(DjangoVersion::Django52)),
        ("Django>=6.1,<6.2", Some(DjangoVersion::Django61)),
        ("Django>=7", None),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            root.join("requirements.txt"),
            "Django @ https://example.org/django.whl\n-c constraints.txt\n".into(),
        );
        fs.add_file(root.join("constraints.txt"), constraint.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            expected,
            "{constraint}"
        );
    }
}

#[test]
fn bundled_django_declaration_extras_are_not_selected() {
    use djls_conf::DjangoVersion;
    let root = Utf8Path::new("/project");
    for (marker, expected) in [
        ("extra == 'web'", DjangoVersion::Django52),
        ("extra != 'web'", DjangoVersion::Django61),
        (
            "extra == 'web' and sys_platform == 'win32'",
            DjangoVersion::Django52,
        ),
        (
            "extra == 'web' or sys_platform == 'win32'",
            DjangoVersion::Django61,
        ),
    ] {
        for poetry in [false, true] {
            let mut fs = InMemoryFileSystem::new();
            if poetry {
                fs.add_file(root.join("pyproject.toml"), format!("[tool.poetry.dependencies]\nDjango = {{version = '>=6.1', markers = \"{marker}\"}}\n"));
            } else {
                fs.add_file(
                    root.join("requirements.txt"),
                    format!("Django>=6.1; {marker}\n"),
                );
            }
            assert_eq!(
                bundled_django_version(&fs, root, None),
                Some(expected),
                "{marker}, poetry={poetry}"
            );
        }
    }
}

#[test]
fn bundled_django_respects_project_python_bounds() {
    use djls_conf::DjangoVersion;
    let root = Utf8Path::new("/project");
    for (bound, expected) in [
        (">=3.12", DjangoVersion::Django61),
        (">=3.10", DjangoVersion::Django52),
    ] {
        for style in ["pep621", "poetry", "setup"] {
            let mut fs = InMemoryFileSystem::new();
            match style {
                "pep621" => fs.add_file(root.join("pyproject.toml"), format!("[project]\nrequires-python = '{bound}'\ndependencies = [\"Django>=5.2,<6; python_version < '3.12'\", \"Django>=6.1; python_version >= '3.12'\"]\n")),
                "poetry" => fs.add_file(root.join("pyproject.toml"), format!("[tool.poetry.dependencies]\npython = '{bound}'\nDjango = [{{version = '>=5.2,<6', python = '<3.12'}}, {{version = '>=6.1', python = '>=3.12'}}]\n")),
                _ => fs.add_file(root.join("setup.cfg"), format!("[options]\npython_requires = {bound}\ninstall_requires =\n    Django>=5.2,<6; python_version < '3.12'\n    Django>=6.1; python_version >= '3.12'\n")),
            }
            assert_eq!(
                bundled_django_version(&fs, root, None),
                Some(expected),
                "{style}, {bound}"
            );
            fs.add_file(root.join("pylock.toml"), "[[packages]]\nname = 'django'\nversion = '5.2.17'\nmarker = \"python_version < '3.12'\"\n".into());
            assert_eq!(
                bundled_django_version(&fs, root, None),
                Some(expected),
                "lock: {style}, {bound}"
            );
        }
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            root.join("pyproject.toml"),
            format!("[project]\nrequires-python = '{bound}'\n"),
        );
        fs.add_file(root.join("pylock.toml"), "[[packages]]\nname = 'django'\nversion = '5.2.17'\nmarker = \"python_version < '3.12'\"\n[[packages]]\nname = 'django'\nversion = '6.1.1'\nmarker = \"python_version >= '3.12'\"\n".into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(expected),
            "lock-only: {bound}"
        );
    }
}
