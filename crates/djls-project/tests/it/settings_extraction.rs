use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io;
use std::iter;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_project::Project;
use djls_project::PythonEnvironment;
use djls_project::PythonModuleName;
use djls_project::SearchPaths;
use djls_project::TemplateName;
use djls_project::TemplateResolutionResult;
use djls_project::template_resolution;
use djls_project::testing::PythonBindingAlternativeView;
use djls_project::testing::PythonBoundValueView;
use djls_project::testing::PythonModuleEvaluationError;
use djls_project::testing::PythonValueKindView;
use djls_project::testing::PythonValueView;
use djls_project::testing::compute_project_facts;
use djls_project::testing::django_settings;
use djls_project::testing::python_module_evaluation;
use djls_project::testing::settings_module_file;
use djls_source::ChangeEvent;
use djls_source::Origin;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_testing::ProjectFixture;
use djls_testing::ProjectFixtureDatabase;
use djls_testing::TestDatabase;
use serde_json::Value;
use serde_json::json;
use serde_json::to_value;

fn extract_project(
    source: &str,
    modules: &[(&str, &str)],
) -> Result<(TestDatabase, Project, Value), Box<dyn std::error::Error>> {
    let mut fixture = ProjectFixture::new("/project/settings")
        .without_bundled_django()
        .django_settings_module("config.settings")
        .file("/project/settings/config/__init__.py", "")
        .file("/project/settings/config/settings.py", source);
    for (module, source) in modules {
        fixture = fixture.file(
            format!("/project/settings/{}.py", module.replace('.', "/")),
            *source,
        );
    }
    let mut db = TestDatabase::new();
    let project = fixture.install(&mut db)?;
    let settings = to_value(django_settings(&db, project))?;
    Ok((db, project, settings))
}

fn extract(source: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(extract_project(source, &[])?.2)
}

fn cases<'a>(settings: &'a Value, pointer: &str) -> Result<&'a [Value], io::Error> {
    settings
        .pointer(pointer)
        .ok_or_else(|| io::Error::other(format!("settings JSON has no pointer `{pointer}`")))?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| io::Error::other(format!("settings JSON at `{pointer}` is not an array")))
}

fn installed_app_alternatives(setting_cases: &[Value]) -> Option<BTreeSet<String>> {
    setting_cases
        .iter()
        .map(|case| {
            Some(
                case.get("known")?
                    .get("apps")?
                    .as_array()?
                    .iter()
                    .map(|app| app.get("value")?.as_str())
                    .collect::<Option<Vec<_>>>()?
                    .join(","),
            )
        })
        .collect()
}
fn python_project(db: &impl ProjectFixtureDatabase) -> Project {
    python_project_with_paths(db, &[])
}

fn python_project_with_paths(
    db: &impl ProjectFixtureDatabase,
    pythonpath: &[Utf8PathBuf],
) -> Project {
    let root = Utf8Path::new("/project");
    let python_environment = PythonEnvironment::Auto;
    let search_paths =
        SearchPaths::from_project_settings(db.file_system(), root, &python_environment, pythonpath);
    ProjectFixture::new(root)
        .python_environment(python_environment)
        .search_paths(search_paths)
        .tag_specs(TagSpecDef::default())
        .build(db)
        .unwrap_or_else(|error| panic!("Python project fixture should build: {error:#}"))
}

fn expected_span(source: &str, needle: &str) -> Option<Span> {
    source
        .find(needle)
        .map(|start| Span::saturating_from_parts_usize(start, needle.len()))
}

fn only<T>(values: &[T]) -> Option<&T> {
    let [value] = values else {
        return None;
    };
    Some(value)
}

fn equal_top_level_list_alternatives(
    count: usize,
    reverse_module_installation: bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut source = String::new();
    let mut modules = Vec::new();
    for index in 0..count {
        if index == 0 {
            source.push_str("if FLAG_0:\n");
        } else if index + 1 == count {
            source.push_str("else:\n");
        } else {
            writeln!(source, "elif FLAG_{index}:")?;
        }
        writeln!(
            source,
            "    from variant_{index:02} import INSTALLED_APPS, TEMPLATES"
        )?;
        modules.push((
            format!("variant_{index:02}"),
            "INSTALLED_APPS = ['shared']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates']}]\n",
        ));
    }
    if reverse_module_installation {
        modules.reverse();
    }
    let module_refs = modules
        .iter()
        .map(|(name, body)| (name.as_str(), *body))
        .collect::<Vec<_>>();
    Ok(extract_project(&source, &module_refs)?.2)
}

#[test]
fn python_module_evaluation_rejects_file_outside_project_search_paths() {
    let db = TestDatabase::new();
    db.add_file("/outside/settings.py", "VALUE = 'outside'\n")
        .expect("unresolved Python test file should be added");
    let project = python_project(&db);
    let file = db
        .file(Utf8Path::new("/outside/settings.py"))
        .expect("unresolved Python test file should exist");

    assert_eq!(
        python_module_evaluation(&db, project, file),
        Err(PythonModuleEvaluationError::UnresolvedFile {
            path: Utf8PathBuf::from("/outside/settings.py"),
        })
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn env_reads_keep_file_candidates_and_runtime_uncertainty() {
    let db = TestDatabase::new();
    let source = "import os\nfrom os import environ as imported_environ, getenv as imported_getenv\nENVIRON = os.environ\nENV_GET = os.environ.get\nGETENV = os.getenv\nPRESENT = os.environ.get('APP')\nALIASED = ENV_GET('APP')\nIMPORTED_GET = imported_environ.get('APP')\nIMPORTED_GETENV = imported_getenv('APP')\nDEFAULTED = GETENV('MISSING', 'fallback')\nGET_DEFAULTED = os.environ.get('MISSING', 'fallback')\nABSENT = os.environ.get('ABSENT')\nGETENV_ABSENT = os.getenv('ABSENT')\nDYNAMIC_KEY = os.getenv(KEY)\nDYNAMIC_DEFAULT = os.getenv('MISSING', fallback())\nKEYWORD_DEFAULT = os.getenv('MISSING', default='fallback')\nSUBSCRIPT = os.environ['APP']\nos.getenv('APP')\nAFTER_DISCARDED = os.getenv('APP')\nINSTALLED_APPS = [os.getenv('APP', 'fallback')]\n";
    for (path, content) in [
        ("/project/.env", "APP=stale.app\nAPP=env.app\n"),
        ("/project/manage.py", ""),
        ("/project/config/__init__.py", ""),
        ("/project/config/settings.py", source),
    ] {
        db.add_file(path, content)
            .expect("env settings fixture file should be added");
    }
    let settings_config: Settings = toml::from_str("django_settings_module = 'config.settings'\n")
        .expect("env settings fixture config should parse");
    let project = Project::bootstrap(&db, Utf8Path::new("/project"), &settings_config);
    assert_eq!(
        project
            .django_settings_module(&db)
            .as_ref()
            .map(PythonModuleName::as_str),
        Some("config.settings"),
    );
    assert_eq!(
        project.env_vars(&db),
        &[
            ("APP".to_string(), "stale.app".to_string()),
            ("APP".to_string(), "env.app".to_string()),
        ]
    );
    let settings =
        to_value(django_settings(&db, project)).expect("Django settings facts should serialize");
    let app_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(app_cases.len(), 2, "{settings:#}");
    assert!(app_cases.iter().any(|case| {
        case.pointer("/known/apps/0/value").and_then(Value::as_str) == Some("env.app")
    }));
    assert!(
        app_cases.iter().any(|case| {
            case.pointer("/dynamic/evidence/0/issue/kind")
                .and_then(Value::as_str)
                == Some("unknown_element")
        }),
        "{settings:#}"
    );
}

#[test]
fn env_read_default_reaches_settings_with_runtime_uncertainty() {
    let db = TestDatabase::new();
    for (path, content) in [
        ("/project/config/__init__.py", ""),
        (
            "/project/config/settings.py",
            "import os\nINSTALLED_APPS = [os.getenv('APP', 'fallback.app')]\n",
        ),
    ] {
        db.add_file(path, content)
            .expect("env settings fixture file should be added");
    }
    let settings_config: Settings = toml::from_str("django_settings_module = 'config.settings'\n")
        .expect("env settings fixture config should parse");
    let project = Project::bootstrap(&db, Utf8Path::new("/project"), &settings_config);
    assert!(project.env_vars(&db).is_empty());

    let settings =
        to_value(django_settings(&db, project)).expect("Django settings facts should serialize");
    let app_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(app_cases.len(), 2, "{settings:#}");
    assert!(app_cases.iter().any(|case| {
        case.pointer("/known/apps/0/value").and_then(Value::as_str) == Some("fallback.app")
    }));
    assert!(app_cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn env_function_intrinsics_are_always_truthy() {
    let settings = extract(
        "import os\nGET = os.environ.get\nINSTALLED_APPS = []\nif os.getenv:\n    INSTALLED_APPS.append('getenv')\nelse:\n    INSTALLED_APPS.append('impossible')\nif GET:\n    INSTALLED_APPS.append('get')\n",
    )
    .expect("Django settings extraction should succeed");
    let app_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(app_cases),
        Some(BTreeSet::from(["getenv,get".to_string()]))
    );
}

#[test]
fn closed_unsupported_literals_are_malformed_but_unknown_unpacks_stay_dynamic() {
    for literal in ["None", "1", "-1", "b'x'", "...", "{1}"] {
        let settings = extract(&format!("INSTALLED_APPS = {literal}"))
            .expect("Django settings extraction should succeed");
        let case = &cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0];
        assert!(case.get("malformed").is_some(), "{literal}: {settings:#}");
        assert!(case.get("dynamic").is_none(), "{literal}: {settings:#}");
    }

    let member = extract("INSTALLED_APPS = ['known', None]")
        .expect("Django settings extraction should succeed");
    let case = &cases(&member, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0];
    assert!(case.get("malformed").is_some(), "{member:#}");
    assert!(case.get("dynamic").is_none(), "{member:#}");

    let unpack = extract("INSTALLED_APPS = ['known', *{1}]")
        .expect("Django settings extraction should succeed");
    let case = &cases(&unpack, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0];
    assert!(case.get("dynamic").is_some(), "{unpack:#}");
    assert!(case.get("malformed").is_none(), "{unpack:#}");

    let non_string_key = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {None: 'app.templatetags.invalid'}}}]",
    ).expect("Django settings extraction should succeed");
    let case = &cases(&non_string_key, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0];
    assert!(case.get("malformed").is_some(), "{non_string_key:#}");
    assert!(case.get("dynamic").is_none(), "{non_string_key:#}");
}

#[test]
fn missing_resolved_named_import_member_is_typed_dynamic_and_replaces_stale_binding() {
    let source = "INSTALLED_APPS = ['stale']\nfrom plugin import MISSING as INSTALLED_APPS\n";
    let (_, _, settings) = extract_project(source, &[("plugin", "PRESENT = 'known'\n")])
        .expect("settings extraction project should build");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(setting_cases.len(), 1, "{settings:#}");
    assert!(setting_cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(setting_cases.iter().all(|case| case != "unset"));
    assert_eq!(
        setting_cases[0]["dynamic"]["evidence"][0]["issue"]["kind"],
        "dynamic_expression",
    );
}

#[test]
fn named_import_of_absent_open_name_preserves_member_and_namespace_uncertainty() {
    let source = "from plugin import INSTALLED_APPS\n";
    let (_, _, settings) = extract_project(
        source,
        &[("plugin", "if ENABLED:\n    from missing import *\n")],
    )
    .expect("settings extraction project should build");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert!(
        setting_cases.iter().all(|case| case != "unset"),
        "{settings:#}"
    );
    assert!(
        setting_cases
            .iter()
            .any(|case| case.get("dynamic").is_some()),
        "{settings:#}",
    );
}

#[test]
fn named_import_of_conditional_binding_preserves_known_member_and_namespace_outcomes() {
    let source = "from plugin import INSTALLED_APPS\n";
    let (_, _, settings) = extract_project(
        source,
        &[(
            "plugin",
            "if ENABLED:\n    INSTALLED_APPS = ['imported']\n    from missing import *\n",
        )],
    )
    .expect("settings extraction project should build");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert!(
        setting_cases.iter().all(|case| case != "unset"),
        "{settings:#}"
    );
    assert!(
        setting_cases
            .iter()
            .any(|case| case.get("dynamic").is_some())
    );
    assert!(
        setting_cases
            .iter()
            .any(|case| { case.pointer("/known/apps/0/value") == Some(&json!("imported")) })
    );
}

#[test]
fn deterministic_false_while_executes_only_else_body() {
    let source = "while False:\n    from missing_body import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['else']\n";
    let (_, _, settings) =
        extract_project(source, &[]).expect("settings extraction project should build");
    assert_eq!(
        cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]["known"]["apps"][0]["value"],
        "else"
    );
}

#[test]
fn ambiguous_while_degrades_writes_and_retains_branch_effects() {
    let source = "INSTALLED_APPS = []\nwhile FLAG:\n    INSTALLED_APPS.append('loop')\nelse:\n    from plugin import VALUE\n";
    let (_, _, settings) = extract_project(source, &[("plugin", "VALUE = '/static/'")])
        .expect("settings extraction project should build");
    let app_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(app_cases.len(), 2, "{settings:#}");
    assert_eq!(
        app_cases[0]["known"]["apps"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        0,
        "{settings:#}"
    );
    assert_eq!(
        app_cases[1]["dynamic"]["evidence"][0]["issue"]["kind"], "dynamic_expression",
        "{settings:#}"
    );
}

// Minimized from OpenStack horizon's settings, where evaluation never finished
// because every `if` doubled the open-namespace causes.
#[test]
fn open_namespace_causes_stay_bounded_across_sequential_ifs() {
    let mut source = String::from(
        "from missing import *\nfor snippet in SNIPPETS:\n    try:\n        exec(snippet)\n    except Exception:\n        pass\n",
    );
    for index in 0..24 {
        writeln!(
            source,
            "if URL_{index} is None:\n    URL_{index} = '/{index}/'"
        )
        .expect("writing to a String cannot fail");
    }
    source.push_str("INSTALLED_APPS = ['app']\n");

    let settings = extract(&source).expect("settings extraction fixture should build");

    let app_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(
        installed_app_alternatives(app_cases),
        Some(["app".to_string()].into_iter().collect()),
        "{settings:#}"
    );
}

#[test]
fn canonical_unknown_origins_merge_dynamic_namespace_import_edges() {
    let source = "from a import *\nfrom b import *\n";
    let (db, project, settings) = extract_project(
        source,
        &[("a", "from b import *\n"), ("b", "from a import *\n")],
    )
    .expect("settings extraction project should build");
    let settings_file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let expected_origins = ["from a import *", "from b import *"].map(|statement| {
        Origin::new(
            settings_file,
            expected_span(source, statement).expect("test source should contain expected text"),
        )
    });

    let dynamic_namespace_issues = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .find_map(|case| case.get("dynamic"))
        .expect("the open namespace should produce a dynamic settings case")["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    let evidence = only(dynamic_namespace_issues)
        .expect("plural namespace provenance should project through one issue");
    assert_eq!(evidence["issue"]["kind"], "dynamic_namespace");
    assert_eq!(
        evidence["issue"]["spans"],
        to_value(expected_origins.map(|origin| origin.span))
            .expect("expected JSON field should be an array")
    );
}

#[test]
fn canonical_unknown_origins_import_rebase_is_exactly_local() {
    let source = "from a import *\n";
    let (db, project, settings) = extract_project(
        source,
        &[
            ("a", "from b import INSTALLED_APPS\n"),
            ("b", "from a import INSTALLED_APPS\n"),
        ],
    )
    .expect("settings extraction project should build");
    let settings_file =
        settings_module_file(&db, project).expect("test value should serialize to JSON");
    let import_origin = Origin::new(
        settings_file,
        expected_span(source, "from a import *").expect("test source should contain expected text"),
    );

    let evidence = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .find_map(|case| case.get("dynamic"))
        .expect("the cycle should produce dynamic setting evidence")["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(evidence.len(), 1, "{settings:#}");
    assert_eq!(evidence[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(
        evidence[0]["issue"]["spans"],
        to_value([import_origin.span]).expect("expected JSON field should be an array")
    );
}

#[test]
fn python_module_cycle_side_dependency_change_invalidates_the_product() {
    let mut db = TestDatabase::new();
    db.add_file("/project/side.py", "SIDE = 'before'\n")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/a.py",
        "from side import SIDE\nfrom b import B\nA = B\n",
    )
    .expect("settings-extraction test file should be added");
    db.add_file("/project/b.py", "from a import A\nB = A\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let a = db
        .file(Utf8Path::new("/project/a.py"))
        .expect("settings-extraction test file should exist");

    let before =
        python_module_evaluation(&db, project, a).expect("Python file should map to a module");
    assert!(matches!(
        before.binding("SIDE").expect("expected test binding should exist").alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "before"
    ));

    db.add_file("/project/side.py", "SIDE = 'after'\n")
        .expect("settings-extraction test file should be added");
    SourceChanges::new([ChangeEvent::ContentChanged("/project/side.py".into())]).apply(&mut db);

    let after =
        python_module_evaluation(&db, project, a).expect("Python file should map to a module");
    assert!(matches!(
        after.binding("SIDE").expect("expected test binding should exist").alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "after"
    ));
}

#[test]
fn multiline_setting_syntax_errors_are_structurally_associated() {
    for (source, pointer) in [
        (
            "INSTALLED_APPS = [\n    'blog',\n    @\n]\n",
            "/installed_apps/cases",
        ),
        (
            "TEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates'},\n    @\n]\n",
            "/templates/cases",
        ),
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        assert!(
            cases(&settings, pointer)
                .expect("settings JSON pointer should identify an array")
                .iter()
                .any(|case| case.get("dynamic").is_some()),
            "{source}"
        );
    }
}

#[test]
fn syntax_errors_in_enclosing_control_flow_widen_touched_settings() {
    for source in [
        "if FLAG:\n    INSTALLED_APPS = ['blog']\n    broken(\n",
        "try:\n    INSTALLED_APPS = ['blog']\n    broken(\nexcept Exception:\n    pass\n",
        "for item in ITEMS:\n    INSTALLED_APPS = ['blog']\n    broken(\n",
        "match VALUE:\n    case _:\n        INSTALLED_APPS = ['blog']\n        broken(\n",
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        assert!(
            cases(&settings, "/installed_apps/cases")
                .expect("settings JSON pointer should identify an array")
                .iter()
                .any(|case| case.get("dynamic").is_some()),
            "{source}"
        );
    }
}

#[test]
fn later_unconditional_exact_assignment_dominates_local_syntax_impact() {
    let settings =
        extract("INSTALLED_APPS = [\n    'stale',\n    @\n]\nINSTALLED_APPS = ['local']\n")
            .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "local");
}

#[test]
fn only_later_binding_targets_can_dominate_local_syntax_impact() {
    for later in [
        "INSTALLED_APPS[0] = 'replacement'",
        "INSTALLED_APPS.value = ['replacement']",
    ] {
        let source = format!("INSTALLED_APPS = [\n    'stale',\n    @\n]\n{later}\n");
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert!(
            setting_cases
                .iter()
                .any(|case| case.to_string().contains("syntax_error")),
            "{later}: {settings:#}"
        );
    }

    let settings = extract(
        "INSTALLED_APPS = [\n    'stale',\n    @\n]\n(INSTALLED_APPS, OTHER) = (['clean'], None)\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(setting_cases.len(), 1, "{settings:#}");
    assert!(
        setting_cases
            .iter()
            .all(|case| !case.to_string().contains("syntax_error")),
        "{settings:#}"
    );
}

#[test]
fn later_assignments_reading_the_impacted_name_do_not_dominate_syntax_impact() {
    for later in [
        "INSTALLED_APPS = INSTALLED_APPS",
        "INSTALLED_APPS += ['later']",
        "INSTALLED_APPS = INSTALLED_APPS + ['later']",
    ] {
        let source = format!("INSTALLED_APPS = [\n    'stale',\n    @\n]\n{later}\n");
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert!(
            setting_cases
                .iter()
                .any(|case| case.to_string().contains("syntax_error")),
            "{later}: {settings:#}"
        );
    }
}

#[test]
fn syntax_impact_taint_propagates_through_multi_hop_aliases() {
    for aliases in [
        "FIRST = INSTALLED_APPS\nSECOND = FIRST\nINSTALLED_APPS = SECOND",
        "if True:\n    FIRST = INSTALLED_APPS\nSECOND = FIRST\nINSTALLED_APPS = SECOND",
    ] {
        let source = format!("INSTALLED_APPS = [\n    'stale',\n    @\n]\n{aliases}\n");
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert!(
            setting_cases
                .iter()
                .any(|case| case.to_string().contains("syntax_error")),
            "{aliases}: {settings:#}"
        );
    }
}

#[test]
fn syntax_impact_taint_propagates_through_comprehensions_and_formatted_strings() {
    for expression in [
        "[item for item in INSTALLED_APPS]",
        "[item for item in ITEMS if INSTALLED_APPS]",
        "{item for item in INSTALLED_APPS}",
        "{key: value for key, value in INSTALLED_APPS}",
        "list(item for item in INSTALLED_APPS)",
        "[f'{INSTALLED_APPS}']",
        "[f'{VALUE:{INSTALLED_APPS}}']",
        "(lambda value=INSTALLED_APPS: value)()",
    ] {
        let source =
            format!("INSTALLED_APPS = [\n    'stale',\n    @\n]\nINSTALLED_APPS = {expression}\n");
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert!(
            setting_cases
                .iter()
                .any(|case| case.to_string().contains("syntax_error")),
            "{expression}: {settings:#}"
        );
    }
}

#[test]
fn independent_write_clears_alias_taint_before_setting_assignment() {
    let settings = extract(
        "INSTALLED_APPS = [\n    'stale',\n    @\n]\nCOPY = INSTALLED_APPS\nCOPY = ['clean']\nINSTALLED_APPS = COPY\n",
    ).expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(setting_cases.len(), 1, "{settings:#}");
    assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], "clean");
}

#[test]
fn later_conditional_assignment_does_not_dominate_local_syntax_impact() {
    let settings = extract(
        "INSTALLED_APPS = [\n    'stale',\n    @\n]\nif FLAG:\n    INSTALLED_APPS = ['conditional']\n",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert!(cases.iter().any(|case| case.get("known").is_some()));
    assert!(
        cases
            .iter()
            .any(|case| case.to_string().contains("syntax_error"))
    );
}

#[test]
fn later_exact_assignment_dominates_namespace_wide_syntax_impact() {
    let settings = extract_project(
        "if FLAG:\n    from clean import *\n    broken(]\nINSTALLED_APPS = ['local']\n",
        &[("clean", "")],
    )
    .expect("settings extraction project should build")
    .2;
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "local");
}

#[test]
fn assignment_reading_a_name_does_not_dominate_namespace_wide_syntax_impact() {
    let settings = extract_project(
        "BASE_APPS = ['base']\nif FLAG:\n    from clean import *\n    broken(]\nINSTALLED_APPS = BASE_APPS\n",
        &[("clean", "")],
    ).expect("settings extraction project should build")
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert!(
        setting_cases
            .iter()
            .any(|case| case.to_string().contains("syntax_error")),
        "{settings:#}"
    );
}

#[test]
fn namespace_wide_syntax_taint_propagates_through_multi_hop_aliases() {
    let settings = extract_project(
        "BASE_APPS = ['base']\nif FLAG:\n    from clean import *\n    broken(]\nFIRST = BASE_APPS\nSECOND = FIRST\nINSTALLED_APPS = SECOND\n",
        &[("clean", "")],
    ).expect("settings extraction project should build")
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert!(
        setting_cases
            .iter()
            .any(|case| case.to_string().contains("syntax_error")),
        "{settings:#}"
    );
}

#[test]
fn named_and_aliased_imports_dominate_namespace_wide_syntax_impact() {
    for import in [
        "from apps import INSTALLED_APPS",
        "from apps import APPS as INSTALLED_APPS",
    ] {
        let source = format!("if FLAG:\n    from clean import *\n    broken(]\n{import}\n");
        let settings = extract_project(
            &source,
            &[
                ("clean", ""),
                (
                    "apps",
                    "INSTALLED_APPS = ['imported']\nAPPS = ['imported']\n",
                ),
            ],
        )
        .expect("settings extraction project should build")
        .2;
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert_eq!(setting_cases.len(), 1, "{import}");
        assert_eq!(
            setting_cases[0]["known"]["apps"][0]["value"], "imported",
            "{import}"
        );
    }
}

#[test]
fn only_deterministic_if_assignments_dominate_namespace_wide_syntax_impact() {
    for (condition, expected) in [("True", true), ("False", false), ("FLAG", false)] {
        let source = format!(
            "if FLAG:\n    from clean import *\n    broken(]\nif {condition}:\n    INSTALLED_APPS = ['selected']\nelse:\n    INSTALLED_APPS = ['fallback']\n"
        );
        let settings = extract_project(&source, &[("clean", "")])
            .expect("settings extraction project should build")
            .2;
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        if condition == "FLAG" {
            assert!(
                setting_cases
                    .iter()
                    .any(|case| case.to_string().contains("syntax_error")),
                "{condition}"
            );
        } else {
            assert_eq!(setting_cases.len(), 1, "{condition}");
            let value = if expected { "selected" } else { "fallback" };
            assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], value);
        }
    }

    let settings = extract_project(
        "if FLAG:\n    from clean import *\n    broken(]\nSELECT = True\nif not SELECT:\n    INSTALLED_APPS = ['wrong']\nelse:\n    INSTALLED_APPS = ['selected']\n",
        &[("clean", "")],
    ).expect("settings extraction project should build")
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(setting_cases.len(), 1);
    assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], "selected");
}

#[test]
fn boolean_short_circuit_preserves_decisive_bound_values() {
    for (assignment, expected) in [
        ("SELECT = False\nSELECT = SELECT and UNKNOWN", "fallback"),
        ("SELECT = True\nSELECT = SELECT or UNKNOWN", "selected"),
    ] {
        let source = format!(
            "{assignment}\nif SELECT:\n    INSTALLED_APPS = ['selected']\nelse:\n    INSTALLED_APPS = ['fallback']\n"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert_eq!(setting_cases.len(), 1, "{assignment}");
        assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], expected);
    }
}

#[test]
fn unreachable_boolean_calls_do_not_contaminate_path_values() {
    for statement in [
        "IGNORED = False and mutate(pathlib.Path)",
        "False and mutate(pathlib.Path)",
        "if False and mutate(pathlib.Path):\n    pass",
    ] {
        let source = format!(
            "import pathlib\n{statement}\nBASE = pathlib.Path(__file__).parent\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE / 'templates']}}]\n"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/templates/cases")
            .expect("settings JSON pointer should identify an array");

        assert_eq!(setting_cases.len(), 1, "{statement}");
        assert_eq!(
            setting_cases[0]["known"]["backends"][0]["dirs"][0]["value"],
            "/project/settings/config/templates",
            "{statement}"
        );
    }
}

#[test]
fn reachable_boolean_calls_persist_intrinsic_contamination() {
    for statement in [
        "True and mutate(pathlib.Path)",
        "if mutate(pathlib.Path) and False:\n    pass",
    ] {
        let source = format!(
            "import pathlib\n{statement}\nimport pathlib as fresh_pathlib\nBASE = fresh_pathlib.Path(__file__).parent\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE / 'templates']}}]\n"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/templates/cases")
            .expect("settings JSON pointer should identify an array");

        assert_eq!(setting_cases.len(), 1, "{statement}");
        assert!(setting_cases[0].get("dynamic").is_some(), "{statement}");
    }
}

#[test]
fn short_circuit_assignments_clear_unreachable_syntax_taint() {
    let settings = extract_project(
        "if FLAG:\n    from clean import *\n    broken(]\nSELECT = False\nSELECT = SELECT and UNKNOWN\nif SELECT:\n    INSTALLED_APPS = ['wrong']\nelse:\n    INSTALLED_APPS = ['selected']\n",
        &[("clean", "")],
    )
    .expect("settings extraction project should build")
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(setting_cases.len(), 1);
    assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], "selected");
}

#[test]
fn boolean_control_flow_honors_decisive_operands() {
    for (condition, expected) in [("FLAG and False", "fallback"), ("FLAG or True", "selected")] {
        let source = format!(
            "if {condition}:\n    INSTALLED_APPS = ['selected']\nelse:\n    INSTALLED_APPS = ['fallback']\n"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let setting_cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert_eq!(setting_cases.len(), 1, "{condition}");
        assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], expected);
    }
}

#[test]
fn repeated_predicates_reuse_one_truth_decision() {
    let settings = extract(
        "FLAG = get_flag()\nINSTALLED_APPS = []\nif FLAG:\n    INSTALLED_APPS.append('first')\nif FLAG:\n    INSTALLED_APPS.append('second')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([String::new(), "first,second".to_string(),]))
    );
}

#[test]
fn boolean_alias_preserves_the_source_predicate() {
    let settings = extract(
        "DEBUG = get_debug()\nINSTALLED_APPS = []\nif DEBUG:\n    INSTALLED_APPS.append('django_autotyping')\nTRACKER = True and DEBUG\nif TRACKER:\n    INSTALLED_APPS.append('requests_tracker')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            String::new(),
            "django_autotyping,requests_tracker".to_string(),
        ]))
    );
}

#[test]
fn derived_and_truth_implies_its_left_operand() {
    let settings = extract(
        "DEBUG = get_debug()\nSILK = DEBUG and get_silk()\nINSTALLED_APPS = []\nif SILK:\n    INSTALLED_APPS.append('silk')\nif DEBUG:\n    INSTALLED_APPS.append('sslserver')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            String::new(),
            "silk,sslserver".to_string(),
            "sslserver".to_string(),
        ]))
    );
}

#[test]
fn derived_or_truth_does_not_imply_its_left_operand() {
    let settings = extract(
        "LEFT = get_left()\nRIGHT = get_right()\nENABLED = LEFT or RIGHT\nINSTALLED_APPS = []\nif ENABLED:\n    INSTALLED_APPS.append('enabled')\nif LEFT:\n    INSTALLED_APPS.append('left')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            String::new(),
            "enabled".to_string(),
            "enabled,left".to_string(),
        ]))
    );
}

#[test]
fn independent_predicates_keep_their_cross_product() {
    let settings = extract(
        "FIRST = get_first()\nSECOND = get_second()\nINSTALLED_APPS = []\nif FIRST:\n    INSTALLED_APPS.append('first')\nif SECOND:\n    INSTALLED_APPS.append('second')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            String::new(),
            "first".to_string(),
            "first,second".to_string(),
            "second".to_string(),
        ]))
    );
}

#[test]
fn unknowns_from_one_destructuring_target_stay_independent() {
    let settings = extract(
        "LEFT, RIGHT = get_pair()\nINSTALLED_APPS = []\nif LEFT:\n    INSTALLED_APPS.append('left')\nif RIGHT:\n    INSTALLED_APPS.append('right')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            String::new(),
            "left".to_string(),
            "left,right".to_string(),
            "right".to_string(),
        ]))
    );
}

#[test]
fn aliases_preserve_a_predicate_identity() {
    let settings = extract(
        "SOURCE = get_flag()\nALIAS = SOURCE\nINSTALLED_APPS = []\nif SOURCE:\n    INSTALLED_APPS.append('source')\nif ALIAS:\n    INSTALLED_APPS.append('alias')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([String::new(), "source,alias".to_string(),]))
    );
}

#[test]
fn chained_assignment_targets_share_one_predicate_identity() {
    let settings = extract(
        "LEFT = RIGHT = get_flag()\nINSTALLED_APPS = []\nif LEFT:\n    INSTALLED_APPS.append('left')\nif RIGHT:\n    INSTALLED_APPS.append('right')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([String::new(), "left,right".to_string(),]))
    );
}

#[test]
fn repeated_unsupported_literal_tests_share_one_predicate_identity() {
    let settings = extract(
        "FLAG = 1\nINSTALLED_APPS = []\nif FLAG:\n    INSTALLED_APPS.append('first')\nif FLAG:\n    INSTALLED_APPS.append('second')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([String::new(), "first,second".to_string(),]))
    );
}

#[test]
fn imported_predicates_keep_source_module_correlations() {
    let settings = extract_project(
        "from flags import FLAG, APPS\nif FLAG:\n    INSTALLED_APPS = APPS + ['checked']\nelse:\n    INSTALLED_APPS = APPS\n",
        &[(
            "flags",
            "FLAG = get_flag()\nif FLAG:\n    APPS = ['enabled']\nelse:\n    APPS = []\n",
        )],
    )
    .expect("settings extraction project should build")
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            String::new(),
            "enabled,checked".to_string(),
        ]))
    );
}

#[test]
fn guarded_mutations_ignore_infeasible_receiver_and_argument_cases() {
    let settings = extract(
        "FLAG = get_flag()\nif FLAG:\n    VALUES = []\n    ITEM = 'a'\nelse:\n    VALUES = True\n    ITEM = 'b'\nif FLAG:\n    VALUES.append(ITEM)\nINSTALLED_APPS = VALUES\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert!(
        setting_cases.iter().any(|case| {
            case.pointer("/known/apps/0/value").and_then(Value::as_str) == Some("a")
        })
    );
}

#[test]
fn mixed_known_and_unknown_binding_cases_keep_stable_reads() {
    let settings = extract(
        "if CHOOSE:\n    FLAG = False\nelse:\n    FLAG = get_flag()\nINSTALLED_APPS = []\nif FLAG:\n    INSTALLED_APPS.append('first')\nif FLAG:\n    INSTALLED_APPS.append('second')\n",
    )
    .expect("Django settings extraction should succeed");
    let known = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .filter_map(|case| case.pointer("/known/apps").and_then(Value::as_array))
        .map(|apps| {
            apps.iter()
                .filter_map(|app| app.get("value").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(
        known,
        BTreeSet::from([String::new(), "first,second".to_string()])
    );
}

#[test]
fn guarded_mutations_ignore_aliases_from_infeasible_cases() {
    let settings = extract(
        "FLAG = get_flag()\nINSTALLED_APPS = ['base']\nif FLAG:\n    TARGET = [[]]\nelse:\n    TARGET = [INSTALLED_APPS]\nif FLAG:\n    TARGET[0].append('x')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(setting_cases.len(), 1, "{settings:#}");
    assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], "base");
}

#[test]
fn rebinding_replaces_a_predicate_identity() {
    let settings = extract(
        "FLAG = get_first()\nINSTALLED_APPS = []\nif FLAG:\n    INSTALLED_APPS.append('first')\nFLAG = get_second()\nif FLAG:\n    INSTALLED_APPS.append('second')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            String::new(),
            "first".to_string(),
            "first,second".to_string(),
            "second".to_string(),
        ]))
    );
}

#[test]
fn negated_and_positive_reads_are_mutually_exclusive() {
    let settings = extract(
        "FLAG = get_flag()\nINSTALLED_APPS = []\nif not FLAG:\n    INSTALLED_APPS.append('negative')\nif FLAG:\n    INSTALLED_APPS.append('positive')\n",
    )
    .expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(
        installed_app_alternatives(setting_cases),
        Some(BTreeSet::from([
            "negative".to_string(),
            "positive".to_string(),
        ]))
    );
}

#[test]
fn unrelated_later_syntax_error_preserves_all_exact_settings() {
    let settings = extract("INSTALLED_APPS = ['blog']\nTEMPLATES = []\ndef broken(\n")
        .expect("Django settings extraction should succeed");

    for pointer in ["/installed_apps/cases", "/templates/cases"] {
        let setting_cases =
            cases(&settings, pointer).expect("settings JSON pointer should identify an array");
        assert_eq!(setting_cases.len(), 1, "{pointer}");
        assert!(setting_cases[0].get("known").is_some(), "{pointer}");
    }
}

#[test]
fn explicit_empty_and_unset_are_distinct() {
    let unset = extract("").expect("Django settings extraction should succeed");
    let empty = extract("INSTALLED_APPS = []").expect("Django settings extraction should succeed");

    assert_eq!(
        cases(&unset, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array"),
        [json!("unset")]
    );
    assert_eq!(
        cases(&empty, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]["known"]["apps"],
        json!([])
    );
}

#[test]
fn installed_apps_preserve_exact_branch_alternatives() {
    let settings =
        extract("if FLAG:\n    INSTALLED_APPS = ['a']\nelse:\n    INSTALLED_APPS = ['b']")
            .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "a");
    assert_eq!(cases[1]["known"]["apps"][0]["value"], "b");
}

#[test]
fn installed_apps_dynamic_member_retains_ordered_known_fragment() {
    let settings = extract("INSTALLED_APPS = ['a', env('APP'), 'b']")
        .expect("Django settings extraction should succeed");
    let dynamic = &cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"];

    let evidence = &dynamic["evidence"];
    assert_eq!(evidence[0]["known"]["value"], "a");
    assert_eq!(evidence[1]["issue"]["kind"], "unknown_element");
    assert_eq!(evidence[2]["known"]["value"], "b");
}

#[test]
fn installed_apps_exact_list_mutations_preserve_known_order() {
    let settings = extract(
        "INSTALLED_APPS = ['middle']\nINSTALLED_APPS.append('last')\nINSTALLED_APPS.extend(['later', 'removed'])\nINSTALLED_APPS.insert(100, 'bounded-last')\nINSTALLED_APPS.insert(-100, 'first')\nINSTALLED_APPS.remove('removed')",
    ).expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array")
        .iter()
        .map(|app| {
            app["value"]
                .as_str()
                .expect("expected JSON field should be an array")
        })
        .collect::<Vec<_>>();

    assert_eq!(apps, ["first", "middle", "last", "later", "bounded-last"]);
}

#[test]
fn ambiguous_installed_apps_insert_and_remove_are_dynamic() {
    for list in ["[UNKNOWN, 'known']", "[*UNKNOWN, 'known']"] {
        for mutation in [
            "INSTALLED_APPS.insert(1, 'inserted')",
            "INSTALLED_APPS.remove('known')",
        ] {
            let source = format!("INSTALLED_APPS = {list}\n{mutation}");
            let cases = cases(
                &extract(&source).expect("Django settings extraction should succeed"),
                "/installed_apps/cases",
            )
            .expect("settings JSON pointer should identify an array")
            .to_vec();

            assert_eq!(cases.len(), 1, "{source}");
            assert!(cases[0].get("dynamic").is_some(), "{source}");
            assert!(!cases[0].to_string().contains("known"), "{source}");
        }
    }
}

#[test]
fn try_match_and_loop_preserve_settings_alternatives() {
    let try_settings = extract(
        "try:\n    INSTALLED_APPS = ['try']\nexcept Exception:\n    INSTALLED_APPS = ['except']",
    )
    .expect("Django settings extraction should succeed");
    let try_apps = cases(&try_settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .map(|case| {
            case["known"]["apps"][0]["value"]
                .as_str()
                .expect("expected JSON value should be a string")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(try_apps, ["except", "try"].into_iter().collect());

    let match_settings = extract(
        "match VALUE:\n    case 1:\n        INSTALLED_APPS = ['one']\n    case _:\n        INSTALLED_APPS = ['other']",
    ).expect("Django settings extraction should succeed");
    let match_apps = cases(&match_settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .map(|case| {
            case["known"]["apps"][0]["value"]
                .as_str()
                .expect("expected JSON value should be a string")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(match_apps, ["one", "other"].into_iter().collect());

    let loop_settings =
        extract("INSTALLED_APPS = ['before']\nfor app in APPS:\n    INSTALLED_APPS = [app]")
            .expect("Django settings extraction should succeed");
    let loop_cases = cases(&loop_settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert!(loop_cases.iter().any(|case| case.get("known").is_some()));
    assert!(loop_cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn wrong_installed_apps_shape_is_malformed() {
    let settings = extract("INSTALLED_APPS = 'not-a-list'")
        .expect("Django settings extraction should succeed");
    assert!(
        cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]
            .get("malformed")
            .is_some()
    );
}

#[test]
fn templates_keep_simultaneous_backends_correlated() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/b']}]",
    ).expect("Django settings extraction should succeed");
    let known = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"];

    assert_eq!(
        known["backends"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2
    );
    assert_eq!(known["backends"][0]["dirs"][0]["value"], "/a");
    assert_eq!(known["backends"][1]["dirs"][0]["value"], "/b");
}

#[test]
fn templates_keep_mutually_exclusive_settings_cases_separate() {
    let settings = extract(
        "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/b']}]",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 2);
    let roots: BTreeSet<_> = cases
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"][0]["value"]
                .as_str()
                .expect("expected JSON value should be a string")
        })
        .collect();
    assert_eq!(roots, ["/a", "/b"].into_iter().collect());
}

#[test]
fn template_field_uncertainty_does_not_erase_siblings() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'OPTIONS': {'libraries': {'good': 'app.templatetags.good'}, 'context_processors': ['app.context.good', unknown]}}]",
    ).expect("Django settings extraction should succeed");
    let template_case = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0];
    assert!(template_case.get("dynamic").is_some(), "{settings:#}");
    assert!(template_case.get("malformed").is_none(), "{settings:#}");
    let backend = &template_case["dynamic"]["evidence"][0]["backend"];

    assert_eq!(
        backend["dirs"]["evidence"][0]["known"]["value"],
        "/templates"
    );
    assert_eq!(backend["libraries"]["known"][0][0], "good");
    assert_eq!(
        backend["dirs"]["evidence"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
    assert!(
        backend["libraries"]["issues"]
            .as_array()
            .expect("expected JSON field should be an array")
            .is_empty()
    );
    assert_eq!(
        backend["context_processors"]["known"][0]["value"],
        "app.context.good"
    );
    assert_eq!(
        backend["context_processors"]["issues"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
    assert_eq!(
        backend["context_processors"]["issues"][0]["kind"],
        "unknown_element"
    );
}

#[test]
fn invalid_context_processor_module_path_is_malformed() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': ['invalid path']}}]",
    ).expect("Django settings extraction should succeed");
    let template_case = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0];

    assert!(template_case.get("malformed").is_some(), "{settings:#}");
    assert!(template_case.get("dynamic").is_none(), "{settings:#}");
    let issues =
        template_case["malformed"]["evidence"][0]["backend"]["context_processors"]["issues"]
            .as_array()
            .expect("expected JSON field should be an array");
    assert_eq!(issues.len(), 1, "{settings:#}");
    assert_eq!(issues[0]["kind"], "invalid_module_name");
}

#[test]
fn missing_template_backend_is_malformed() {
    let settings = extract("TEMPLATES = [{'DIRS': ['/templates']}]")
        .expect("Django settings extraction should succeed");
    assert!(
        cases(&settings, "/templates/cases")
            .expect("settings JSON pointer should identify an array")[0]
            .get("malformed")
            .is_some()
    );
}

#[test]
fn explicit_app_dirs_retains_origins_and_absence_stays_distinct() {
    let complete = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'APP_DIRS': True}]",
    ).expect("Django settings extraction should succeed");
    let app_dirs = &cases(&complete, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["backends"][0]["app_dirs"];
    assert_eq!(app_dirs["value"], true);
    assert_eq!(
        app_dirs["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );

    let absent =
        extract("TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates'}]")
            .expect("Django settings extraction should succeed");
    assert!(
        cases(&absent, "/templates/cases").expect("settings JSON pointer should identify an array")
            [0]["known"]["backends"][0]["app_dirs"]
            .is_null()
    );

    let partial = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN], 'APP_DIRS': False}]",
    ).expect("Django settings extraction should succeed");
    let app_dirs = &cases(&partial, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"]
        ["app_dirs"]["known"];
    assert_eq!(app_dirs["value"], false);
    assert_eq!(
        app_dirs["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
}

#[test]
fn exact_wrong_template_member_shapes_are_malformed() {
    for member in [
        "'BACKEND': False",
        "'DIRS': 'templates'",
        "'APP_DIRS': 'yes'",
        "'OPTIONS': []",
        "'OPTIONS': {'libraries': []}",
        "'OPTIONS': {'builtins': 'module'}",
        "'OPTIONS': {'context_processors': 'processor'}",
    ] {
        let source = format!(
            "TEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', {member}}}]"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        assert!(
            cases(&settings, "/templates/cases")
                .expect("settings JSON pointer should identify an array")[0]
                .get("malformed")
                .is_some(),
            "expected malformed for {member}"
        );
    }
}

#[test]
fn dynamic_template_members_are_not_malformed() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN]}]",
    ).expect("Django settings extraction should succeed");
    let case = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0];
    assert!(case.get("dynamic").is_some());
    assert!(case.get("malformed").is_none());
}

#[test]
fn dynamic_templates_preserve_backend_order_and_complete_siblings() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/first']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN]}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/third']}]",
    ).expect("Django settings extraction should succeed");
    let evidence = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    let backends = evidence
        .iter()
        .map(|evidence| &evidence["backend"])
        .collect::<Vec<_>>();

    assert_eq!(backends.len(), 3);
    assert_eq!(
        backends[0]["dirs"]["evidence"][0]["known"]["value"],
        "/first"
    );
    assert_eq!(
        backends[2]["dirs"]["evidence"][0]["known"]["value"],
        "/third"
    );
}

#[test]
fn malformed_templates_preserve_backend_order_and_complete_siblings() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/first']}, {'DIRS': ['/broken']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/third']}]",
    ).expect("Django settings extraction should succeed");
    let evidence = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["malformed"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    let backends = evidence
        .iter()
        .map(|evidence| &evidence["backend"])
        .collect::<Vec<_>>();

    assert_eq!(backends.len(), 3);
    assert_eq!(
        backends[0]["dirs"]["evidence"][0]["known"]["value"],
        "/first"
    );
    assert_eq!(
        backends[2]["dirs"]["evidence"][0]["known"]["value"],
        "/third"
    );
}

#[test]
fn template_library_aliases_use_last_exact_value() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'same': 'first.tags', 'same': 'last.tags'}}}]",
    ).expect("Django settings extraction should succeed");
    let libraries = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["backends"][0]["libraries"];
    assert_eq!(
        libraries
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
    assert_eq!(libraries[0][0], "same");
    assert_eq!(libraries[0][1]["value"], "last.tags");
}

#[test]
fn overwritten_invalid_library_value_contributes_no_issue() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'same': False, 'same': 'last.tags'}}}]",
    ).expect("Django settings extraction should succeed");
    let backend = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["backends"][0];

    assert_eq!(
        backend["libraries"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
    assert_eq!(backend["libraries"][0][1]["value"], "last.tags");
}

#[test]
fn duplicate_mapping_keys_use_last_exact_value() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': unknown, 'DIRS': ['/last']}]",
    ).expect("Django settings extraction should succeed");
    let backend = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["backends"][0];
    assert_eq!(backend["dirs"][0]["value"], "/last");
}

#[test]
fn equivalent_template_cases_merge_all_value_origins() {
    let settings = extract(
        "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': True, 'OPTIONS': {'context_processors': ['app.context.processor']}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': True, 'OPTIONS': {'context_processors': ['app.context.processor']}}]",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1);
    assert_eq!(
        cases[0]["known"]["backends"][0]["backend"]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2
    );
    assert_eq!(
        cases[0]["known"]["backends"][0]["dirs"][0]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2
    );
    assert_eq!(
        cases[0]["known"]["backends"][0]["app_dirs"]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2
    );
    assert_eq!(
        cases[0]["known"]["backends"][0]["context_processors"][0]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2
    );
}

#[test]
fn branch_merged_unknown_appended_to_installed_apps_retains_all_origins() {
    let source = "if FLAG:\n    APP = first_dynamic()\nelse:\n    APP = second_dynamic()\nINSTALLED_APPS = []\nINSTALLED_APPS.append(APP)";
    let settings = extract(source).expect("Django settings extraction should succeed");
    let evidence = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(evidence.len(), 1, "{settings:#}");
    assert_eq!(evidence[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(
        evidence[0]["issue"]["spans"],
        to_value([
            expected_span(source, "first_dynamic()")
                .expect("test source should contain expected text"),
            expected_span(source, "second_dynamic()")
                .expect("test source should contain expected text"),
        ])
        .expect("expected JSON field should be an array")
    );
}

#[test]
fn typed_unknowns_reach_module_name_and_path_extractors_with_all_origins() {
    let source = "if FLAG:\n    VALUE = []\n    VALUE.clear()\nelse:\n    VALUE = []\n    VALUE.clear()\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': VALUE, 'OPTIONS': {'builtins': VALUE}},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': []},\n]\nTEMPLATES[1]['DIRS'].append(VALUE)";
    let settings = extract(source).expect("Django settings extraction should succeed");
    let expected_spans = to_value([
        expected_span(source, "VALUE.clear()").expect("test source should contain expected text"),
        Span::saturating_from_parts_usize(
            source
                .rfind("VALUE.clear()")
                .expect("test value should serialize to JSON"),
            "VALUE.clear()".len(),
        ),
    ])
    .expect("test value should serialize to JSON");

    let backends = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    let direct_dirs_issue = &backends[0]["backend"]["dirs"]["evidence"][0]["issue"];
    let builtins_issue = &backends[0]["backend"]["builtins"]["issues"][0];
    let appended_dirs_issue = &backends[1]["backend"]["dirs"]["evidence"][0]["issue"];

    for issue in [direct_dirs_issue, builtins_issue, appended_dirs_issue] {
        assert_eq!(issue["kind"], "unsupported_mutation", "{settings:#}");
        assert_eq!(issue["spans"], expected_spans, "{settings:#}");
    }
}

#[test]
fn path_collections_keep_known_paths_and_uncertainty_in_source_order() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/first', *UNKNOWN, '/later']}]",
    ).expect("Django settings extraction should succeed");
    let template_evidence = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"]
        ["dirs"]["evidence"];
    assert_eq!(template_evidence[0]["known"]["value"], "/first");
    assert_eq!(template_evidence[1]["issue"]["kind"], "unknown_unpack");
    assert_eq!(template_evidence[2]["known"]["value"], "/later");
}

#[test]
fn exact_path_lists_preserve_duplicate_entries() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/same', '/same']} ]",
    ).expect("Django settings extraction should succeed");

    let template_dirs = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["backends"][0]["dirs"];
    assert_eq!(
        template_dirs
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2
    );
    assert_eq!(template_dirs[0]["value"], "/same");
    assert_eq!(template_dirs[1]["value"], "/same");
}

#[test]
fn uncertain_star_import_preserves_known_setting_alternatives() {
    let settings = extract(
        "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nfrom missing import *",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| {
            known["apps"][0]["value"]
                .as_str()
                .expect("expected JSON value should be a string")
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(known, ["first", "second"].into_iter().collect());
    assert!(cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn conditional_uncertain_star_import_preserves_known_setting_alternatives() {
    let settings = extract(
        "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nif PLUGINS:\n    from missing import *",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| {
            known["apps"][0]["value"]
                .as_str()
                .expect("expected JSON value should be a string")
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(known, ["first", "second"].into_iter().collect());
    assert!(cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn exact_assignment_after_uncertain_star_import_restores_certainty() {
    let settings =
        extract("INSTALLED_APPS = ['stale']\nfrom missing import *\nINSTALLED_APPS = ['local']")
            .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "local");
}

#[test]
fn straight_line_unsupported_mutation_discards_stale_installed_apps() {
    let source = "INSTALLED_APPS = ['stale']\nINSTALLED_APPS.clear()";
    let settings = extract(source).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{source}");
    assert!(cases[0].get("known").is_none(), "{source}");
    assert_eq!(
        cases[0]["dynamic"]["evidence"][0]["issue"]["kind"], "unsupported_mutation",
        "{source}"
    );
    assert!(!cases[0].to_string().contains("stale"), "{source}");
}

#[test]
fn branch_local_unsupported_mutation_preserves_unaffected_known_alternative() {
    let settings = extract("INSTALLED_APPS = ['kept']\nif FLAG:\n    INSTALLED_APPS.clear()")
        .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 2);
    assert!(cases.iter().any(|case| {
        case["known"]["apps"][0]["value"]
            .as_str()
            .is_some_and(|app| app == "kept")
    }));
    assert!(cases.iter().any(|case| {
        case["dynamic"]["evidence"][0]["issue"]["kind"]
            .as_str()
            .is_some_and(|kind| kind == "unsupported_mutation")
    }));
}

#[test]
fn installed_apps_augmented_assignment_remains_exact() {
    let settings = extract("INSTALLED_APPS = ['first']\nINSTALLED_APPS += ['second']")
        .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array")
        .iter()
        .map(|app| {
            app["value"]
                .as_str()
                .expect("expected JSON field should be an array")
        })
        .collect::<Vec<_>>();

    assert_eq!(apps, ["first", "second"]);
}

#[test]
fn dynamic_list_additions_preserve_known_installed_apps_prefix() {
    for source in [
        "INSTALLED_APPS = ['first']\nINSTALLED_APPS += EXTRA_APPS",
        "INSTALLED_APPS = ['first'] + EXTRA_APPS",
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        let cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert_eq!(cases.len(), 1, "{source}");
        let evidence = cases[0]["dynamic"]["evidence"]
            .as_array()
            .expect("expected JSON field should be an array");
        assert_eq!(evidence.len(), 2, "{source}");
        assert_eq!(evidence[0]["known"]["value"], "first", "{source}");
        assert_eq!(evidence[1]["issue"]["kind"], "unknown_unpack", "{source}");
    }
}

#[test]
fn binary_list_plus_string_does_not_preserve_stale_installed_apps() {
    // Binary `+` never delegates to iterable extension: a list-plus-string is a
    // wholly unsupported expression, so no known prefix survives.
    let source = "INSTALLED_APPS = ['stale'] + 'invalid'";
    let settings = extract(source).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{source}");
    assert!(cases[0].get("known").is_none(), "{source}");
    assert_eq!(
        cases[0]["dynamic"]["evidence"][0]["issue"]["kind"], "dynamic_expression",
        "{source}"
    );
    assert!(!cases[0].to_string().contains("stale"), "{source}");
}

#[test]
fn list_augmented_add_string_preserves_prefix_and_unknown_remainder() {
    // `list += str` recognizes a known-but-imprecise iterable: the known prefix
    // survives and the string contributes a typed unknown-unpack remainder.
    let source = "INSTALLED_APPS = ['stale']\nINSTALLED_APPS += 'invalid'";
    let settings = extract(source).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{source}");
    let evidence = cases[0]["dynamic"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(evidence.len(), 2, "{source}");
    assert_eq!(evidence[0]["known"]["value"], "stale", "{source}");
    assert_eq!(evidence[1]["issue"]["kind"], "unknown_unpack", "{source}");
}

#[test]
fn unsupported_mutations_keep_settings_conservative() {
    for (source, pointer, count) in [
        (
            "INSTALLED_APPS = []\nAPPS = INSTALLED_APPS\nAPPS.append('blog')",
            "/installed_apps/cases",
            1,
        ),
        (
            "INSTALLED_APPS = []\nWRAPPER = {'apps': INSTALLED_APPS}\nWRAPPER['apps'] += ['blog']",
            "/installed_apps/cases",
            1,
        ),
        (
            "DIRS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': DIRS},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': DIRS},\n]\nTEMPLATES[0]['DIRS'].append('/shared')",
            "/templates/cases",
            1,
        ),
        (
            "if FLAG:\n    BASE = ['first']\nelse:\n    BASE = ['second']\nINSTALLED_APPS = BASE\nBASE.append('blog')",
            "/installed_apps/cases",
            1,
        ),
        (
            "INSTALLED_APPS = []\nEXTRA = make_mapping()\nWRAPPER = {'apps': INSTALLED_APPS, **EXTRA}\nWRAPPER['apps'].append('blog')",
            "/installed_apps/cases",
            1,
        ),
        (
            "INSTALLED_APPS = ('first',)\nINSTALLED_APPS.append('second')",
            "/installed_apps/cases",
            1,
        ),
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        let cases =
            cases(&settings, pointer).expect("settings JSON pointer should identify an array");
        assert_eq!(cases.len(), count, "{source}\n{settings:#}");
        assert!(
            cases.iter().all(|case| case.get("dynamic").is_some()),
            "{source}\n{settings:#}"
        );
        assert!(
            cases.iter().all(|case| case.get("known").is_none()),
            "{source}\n{settings:#}"
        );
    }
}

#[test]
fn mutating_a_container_preserves_unmodified_nested_settings() {
    let settings = extract(
        "INSTALLED_APPS = ['kept']\nWRAPPER = [INSTALLED_APPS]\nWRAPPER.append('unrelated')",
    )
    .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(apps.len(), 1, "{settings:#}");
    assert_eq!(apps[0]["value"], "kept");
}

#[test]
fn arbitrary_calls_degrade_mutable_aliases() {
    let settings = extract("INSTALLED_APPS = []\nAPPS = INSTALLED_APPS\nconfigure(APPS)")
        .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 2, "{settings:#}");
    assert!(
        cases.iter().any(|case| case.get("dynamic").is_some()),
        "{settings:#}"
    );
}

#[test]
fn failed_recognized_mutations_degrade_argument_aliases() {
    let settings = extract(
        "INSTALLED_APPS = []\nAPPS = INSTALLED_APPS\nHANDLER = factory()\nHANDLER.append(APPS)",
    )
    .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert!(
        cases.iter().any(|case| case.get("dynamic").is_some()),
        "{settings:#}"
    );
}

#[test]
fn mutation_calls_with_keywords_or_starred_arguments_are_not_applied() {
    for source in [
        "INSTALLED_APPS = []\nINSTALLED_APPS.append('blog', unexpected=True)",
        "INSTALLED_APPS = []\nAPPS = ['blog']\nINSTALLED_APPS.append(*APPS)",
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        let cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");

        assert!(
            cases.iter().any(|case| case.get("dynamic").is_some()),
            "{source}\n{settings:#}"
        );
        assert!(
            cases.iter().all(|case| case.get("known").is_none()),
            "{source}\n{settings:#}"
        );
    }
}

#[test]
fn simple_mutable_alias_mutation_preserves_mutated_setting() {
    let settings = extract("APPS = []\nINSTALLED_APPS = APPS\nINSTALLED_APPS.append('blog')")
        .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(apps.len(), 1, "{settings:#}");
    assert_eq!(apps[0]["value"], "blog");
}

#[test]
fn tuple_installed_apps_forms_preserve_exact_order() {
    for (source, expected) in [
        (
            "INSTALLED_APPS = APPS = ('first', 'second')",
            ["first", "second"],
        ),
        ("INSTALLED_APPS = ('first', 'second')", ["first", "second"]),
        ("INSTALLED_APPS = ('alpha',) + ('beta',)", ["alpha", "beta"]),
        (
            "INSTALLED_APPS = ('alpha',)\nINSTALLED_APPS += ('beta',)",
            ["alpha", "beta"],
        ),
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        let apps = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
            .as_array()
            .expect("expected JSON field should be an array");
        assert_eq!(apps.len(), 2, "{source}\n{settings:#}");
        assert_eq!(apps[0]["value"], expected[0], "{source}");
        assert_eq!(apps[1]["value"], expected[1], "{source}");
    }
}

#[test]
fn string_installed_apps_are_not_accepted_as_a_collection() {
    let settings =
        extract("INSTALLED_APPS = 'blog'").expect("Django settings extraction should succeed");
    let case = &cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0];

    assert!(
        case.get("known").is_none(),
        "a bare string is not a collection: {settings:#}"
    );
    assert!(case.get("malformed").is_some(), "{settings:#}");
}

#[test]
fn tuple_collection_shaped_template_settings_are_accepted() {
    let source = "TEMPLATES = ({'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ('/templates',), 'OPTIONS': {'context_processors': ('app.context.processor',), 'builtins': ('app.builtins',)}},)";
    let settings = extract(source).expect("Django settings extraction should succeed");

    let backend = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["backends"][0];
    assert_eq!(backend["dirs"][0]["value"], "/templates");
    assert_eq!(
        backend["context_processors"][0],
        json!({
            "value": "app.context.processor",
            "spans": [expected_span(source, "'app.context.processor'").expect("test source should contain expected text")],
        })
    );
}

#[test]
fn binary_cross_kind_list_and_tuple_is_unsupported() {
    for source in [
        "INSTALLED_APPS = ['alpha'] + ('beta',)",
        "INSTALLED_APPS = ('alpha',) + ['beta']",
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        let cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");
        assert_eq!(cases.len(), 1, "{source}");
        assert!(cases[0].get("known").is_none(), "{source}");
        assert!(!cases[0].to_string().contains("alpha"), "{source}");
    }
}

#[test]
fn name_target_tuple_augmented_add_unknown_keeps_prefix() {
    let settings = extract("INSTALLED_APPS = ('alpha',)\nINSTALLED_APPS += EXTRA")
        .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(cases.len(), 1, "{settings:#}");
    let evidence = cases[0]["dynamic"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(evidence[0]["known"]["value"], "alpha");
    assert_eq!(evidence[1]["issue"]["kind"], "unknown_unpack");
}

#[test]
fn list_augmented_add_tuple_preserves_exact_order() {
    let settings = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS += ('beta',)")
        .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array")
        .iter()
        .map(|app| {
            app["value"]
                .as_str()
                .expect("expected JSON field should be an array")
        })
        .collect::<Vec<_>>();
    assert_eq!(apps, ["alpha", "beta"], "{settings:#}");
}

#[test]
fn list_augmented_add_bool_discards_prefix() {
    let settings = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS += True")
        .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
    assert!(!cases[0].to_string().contains("alpha"), "{settings:#}");
}

#[test]
fn list_extend_tuple_is_exact_but_extend_bool_degrades() {
    let exact = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS.extend(('beta',))")
        .expect("Django settings extraction should succeed");
    let apps = cases(&exact, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(apps.len(), 2, "{exact:#}");

    let degraded = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS.extend(True)")
        .expect("Django settings extraction should succeed");
    let cases = cases(&degraded, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert!(cases[0].get("known").is_none(), "{degraded:#}");
    assert!(!cases[0].to_string().contains("alpha"), "{degraded:#}");
}

#[test]
fn starred_construction_follows_iterable_matrix() {
    let exact = extract("INSTALLED_APPS = [*('alpha', 'beta')]")
        .expect("Django settings extraction should succeed");
    let apps = cases(&exact, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(apps.len(), 2, "{exact:#}");

    let imprecise = extract("INSTALLED_APPS = ['alpha', *'xy']")
        .expect("Django settings extraction should succeed");
    let imprecise_cases = cases(&imprecise, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert!(imprecise_cases[0].get("known").is_none(), "{imprecise:#}");
    assert!(
        imprecise_cases[0].to_string().contains("unknown_unpack"),
        "{imprecise:#}"
    );

    let bool_source = extract("INSTALLED_APPS = ['alpha', *True]")
        .expect("Django settings extraction should succeed");
    let bool_cases = cases(&bool_source, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert!(bool_cases[0].get("known").is_none(), "{bool_source:#}");
    assert!(
        !bool_cases[0].to_string().contains("alpha"),
        "{bool_source:#}"
    );
}

#[test]
fn starred_tuple_construction_follows_iterable_matrix() {
    let exact = extract("INSTALLED_APPS = (*('alpha', 'beta'),)")
        .expect("Django settings extraction should succeed");
    let apps = cases(&exact, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(apps.len(), 2, "{exact:#}");

    for rhs in ["''", "{}", "EXTRA"] {
        let source = format!("from pathlib import Path\nINSTALLED_APPS = ('alpha', *{rhs})");
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let case = &cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0];
        assert!(case.get("known").is_none(), "{source}: {settings:#}");
        assert!(case.to_string().contains("unknown_unpack"), "{source}");
    }

    for invalid in [
        extract("INSTALLED_APPS = ('alpha', *True)")
            .expect("Django settings extraction should succeed"),
        extract("from pathlib import Path\nINSTALLED_APPS = ('alpha', *Path(__file__))")
            .expect("Django settings extraction should succeed"),
    ] {
        let case = &cases(&invalid, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0];
        assert!(!case.to_string().contains("alpha"), "{invalid:#}");
    }
}

#[test]
fn name_target_tuple_augmented_add_incompatible_kinds_degrade() {
    for rhs in [
        "['beta']",
        "'beta'",
        "True",
        "{'beta': 1}",
        "Path(__file__)",
    ] {
        let source = format!(
            "from pathlib import Path\nINSTALLED_APPS = ('alpha',)\nINSTALLED_APPS += {rhs}"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");
        assert_eq!(cases.len(), 1, "{source}");
        assert!(cases[0].get("known").is_none(), "{source}: {settings:#}");
        assert!(
            !cases[0].to_string().contains("alpha"),
            "{source}: {settings:#}"
        );
    }
}

#[test]
fn list_augmented_add_imprecise_and_indeterminate_sources_keep_prefix() {
    for rhs in ["''", "{'k': 'v'}", "{}", "EXTRA"] {
        let source = format!(
            "from pathlib import Path\nINSTALLED_APPS = ['alpha']\nINSTALLED_APPS += {rhs}"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");
        assert_eq!(cases.len(), 1, "{source}");
        let evidence = cases[0]["dynamic"]["evidence"]
            .as_array()
            .expect("expected JSON field should be an array");
        assert_eq!(
            evidence[0]["known"]["value"], "alpha",
            "{source}: {settings:#}"
        );
        assert_eq!(
            evidence[1]["issue"]["kind"], "unknown_unpack",
            "{source}: {settings:#}"
        );
    }
}

#[test]
fn list_extend_imprecise_and_indeterminate_sources_keep_prefix() {
    for rhs in ["'xy'", "''", "{'k': 'v'}", "{}", "EXTRA"] {
        let source = format!(
            "from pathlib import Path\nINSTALLED_APPS = ['alpha']\nINSTALLED_APPS.extend({rhs})"
        );
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");
        assert_eq!(cases.len(), 1, "{source}");
        let evidence = cases[0]["dynamic"]["evidence"]
            .as_array()
            .expect("expected JSON field should be an array");
        assert_eq!(
            evidence[0]["known"]["value"], "alpha",
            "{source}: {settings:#}"
        );
        assert_eq!(
            evidence[1]["issue"]["kind"], "unknown_unpack",
            "{source}: {settings:#}"
        );
    }
}

#[test]
fn starred_construction_covers_list_mapping_and_indeterminate_sources() {
    let list = extract("INSTALLED_APPS = [*['alpha', 'beta']]")
        .expect("Django settings extraction should succeed");
    let apps = cases(&list, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(apps.len(), 2, "{list:#}");

    for rhs in ["''", "{'k': 'v'}", "{}", "EXTRA"] {
        let source = format!("from pathlib import Path\nINSTALLED_APPS = ['alpha', *{rhs}]");
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let cases = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array");
        assert!(cases[0].get("known").is_none(), "{source}: {settings:#}");
        assert!(
            cases[0].to_string().contains("unknown_unpack"),
            "{source}: {settings:#}"
        );
    }
}

#[test]
fn exact_path_values_are_definitely_non_iterable() {
    for source in [
        "from pathlib import Path\nINSTALLED_APPS = ['alpha']\nINSTALLED_APPS += Path(__file__)",
        "from pathlib import Path\nINSTALLED_APPS = ['alpha']\nINSTALLED_APPS.extend(Path(__file__))",
        "from pathlib import Path\nINSTALLED_APPS = ['alpha', *Path(__file__)]",
    ] {
        let settings = extract(source).expect("Django settings extraction should succeed");
        let case = &cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0];
        assert!(
            !case.to_string().contains("alpha"),
            "{source}: {settings:#}"
        );
        assert!(
            !case.to_string().contains("unknown_unpack"),
            "{source}: {settings:#}"
        );
    }
}

#[test]
fn chained_mutable_assignment_keeps_settings_conservative() {
    let settings = extract("INSTALLED_APPS = APPS = []\nAPPS.append('blog')")
        .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_assignment_from_existing_name_invalidates_the_source() {
    let settings =
        extract("INSTALLED_APPS = []\nAPPS = ALIAS = INSTALLED_APPS\nALIAS.append('blog')")
            .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_assignment_invalidates_existing_aliases() {
    let settings =
        extract("ROOT = []\nINSTALLED_APPS = ROOT\nLEFT = RIGHT = ROOT\nRIGHT.append('blog')")
            .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_assignment_finds_aliases_after_augmented_add() {
    let settings =
        extract("ROOT = []\nINSTALLED_APPS = ROOT\nROOT += ['blog']\nLEFT = RIGHT = ROOT")
            .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_dict_assignment_keeps_templates_conservative() {
    let settings = extract(
        "BACKEND = ALIAS = {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': []}\nTEMPLATES = [BACKEND]\nALIAS['DIRS'].append('/templates')",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn nested_template_dirs_append_and_extend_are_supported() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nTEMPLATES[0]['DIRS'].append('/b')\nTEMPLATES[0]['DIRS'].extend(['/c', '/d'])",
    ).expect("Django settings extraction should succeed");
    let dirs = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["backends"][0]["dirs"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(dirs.len(), 4);
    assert_eq!(dirs[1]["value"], "/b");
    assert_eq!(dirs[2]["value"], "/c");
    assert_eq!(dirs[3]["value"], "/d");
}

#[test]
fn nested_template_dirs_insert_and_remove_are_supported() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a', '/c', '/removed']}]
TEMPLATES[0]['DIRS'].insert(1, '/b')
TEMPLATES[0]['DIRS'].remove('/removed')",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    let dirs = cases[0]["known"]["backends"][0]["dirs"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert_eq!(dirs.len(), 3, "{settings:#}");
    assert_eq!(dirs[0]["value"], "/a");
    assert_eq!(dirs[1]["value"], "/b");
    assert_eq!(dirs[2]["value"], "/c");
}

#[test]
fn imported_template_nested_mutation_preserves_known_backend_fields() {
    let settings = extract_project(
        "from base import *\nif FLAG:\n    TEMPLATES[0]['OPTIONS']['loaders'] = ('loader',)\nTEMPLATES[0]['DIRS'].insert(0, dynamic_path())\n",
        &[(
            "base",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/base'], 'OPTIONS': {'context_processors': ['app.context.processor']}}]\n",
        )],
    )
    .expect("settings extraction project should build")
    .2;
    let template_case = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0];
    let backend = &template_case["dynamic"]["evidence"][0]["backend"];

    assert_eq!(
        backend["backend"]["known"]["value"], "django.template.backends.django.DjangoTemplates",
        "{settings:#}"
    );
    assert_eq!(
        backend["context_processors"]["known"][0]["value"],
        "app.context.processor"
    );
    assert!(backend["dirs"].to_string().contains("dynamic_expression"));
    assert!(backend["dirs"].to_string().contains("/base"));
}

#[test]
fn nested_dictionary_assignment_invalidates_stale_aliases_only() {
    let source = "OPTIONS = {}\nALIAS = OPTIONS\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'OPTIONS': OPTIONS}]\nTEMPLATES[0]['OPTIONS']['loaders'] = ('loader',)\n";
    let settings = extract(source).expect("Django settings extraction should succeed");
    let template_case = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0];
    assert_eq!(
        template_case["known"]["backends"][0]["backend"]["value"],
        "django.template.backends.django.DjangoTemplates"
    );
}

#[test]
fn self_extension_duplicates_elements_without_creating_recursion() {
    for operation in ["VALUES.extend(VALUES)", "VALUES += VALUES"] {
        let source = format!("VALUES = ['app']\n{operation}\nINSTALLED_APPS = VALUES\n");
        let settings = extract(&source).expect("Django settings extraction should succeed");
        let apps = cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
            .as_array()
            .expect("expected JSON field should be an array");

        assert_eq!(apps.len(), 2, "{operation}: {settings:#}");
        assert!(apps.iter().all(|app| app["value"] == "app"));
    }
}

#[test]
fn nested_template_dirs_mutation_updates_all_correlated_equal_lists() {
    let settings = extract(
        "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nTEMPLATES[0]['DIRS'].append('/b')",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");

    assert!(!cases.is_empty(), "{settings:#}");
    for case in cases {
        let dirs = case["known"]["backends"][0]["dirs"]
            .as_array()
            .expect("expected JSON field should be an array");
        assert_eq!(dirs.len(), 2, "{settings:#}");
        assert_eq!(dirs[0]["value"], "/a");
        assert_eq!(dirs[1]["value"], "/b");
    }
}

#[test]
fn nested_template_dirs_augmented_assignment_is_supported() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nTEMPLATES[0]['DIRS'] += ['/b']",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1);
    assert_eq!(
        cases[0]["known"]["backends"][0]["dirs"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2
    );
    assert_eq!(cases[0]["known"]["backends"][0]["dirs"][1]["value"], "/b");
}

#[test]
fn differing_branch_scalars_distribute_through_settings_collections() {
    let settings = extract_project(
        "if FLAG:\n    from one.values import ROOT, BASE_APPS\nelse:\n    from two.values import ROOT, BASE_APPS\nINSTALLED_APPS = BASE_APPS + ['local']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT]}]",
        &[
            ("one.values", "ROOT = '/one'\nBASE_APPS = ['one']"),
            ("two.values", "ROOT = '/two'\nBASE_APPS = ['two']"),
        ],
    ).expect("settings extraction project should build")
    .2;

    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .map(|case| {
            case["known"]["apps"]
                .as_array()
                .expect("expected JSON field should be an array")
                .iter()
                .map(|app| {
                    app["value"]
                        .as_str()
                        .expect("expected JSON field should be an array")
                })
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        apps,
        [vec!["one", "local"], vec!["two", "local"]]
            .into_iter()
            .collect()
    );

    let paths = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"][0]["value"]
                .as_str()
                .expect("expected JSON value should be a string")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(paths, ["/one", "/two"].into_iter().collect());
}

#[test]
fn independent_imported_scalar_paths_expand_to_a_cartesian_product() {
    let settings = extract_project(
        "if FIRST_FLAG:\n    from one.values import FIRST\nelse:\n    from two.values import FIRST\nif SECOND_FLAG:\n    from one.values import SECOND\nelse:\n    from two.values import SECOND\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [FIRST, SECOND]}]",
        &[
            ("one.values", "FIRST = 'first'\nSECOND = 'second'"),
            ("two.values", "FIRST = 'first'\nSECOND = 'second'"),
        ],
    ).expect("settings extraction project should build")
    .2;

    let template_dirs = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"]
                .as_array()
                .expect("expected JSON field should be an array")
                .iter()
                .map(|dir| {
                    dir["value"]
                        .as_str()
                        .expect("expected JSON field should be an array")
                })
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    let expected = [
        vec![
            "/project/settings/one/first",
            "/project/settings/one/second",
        ],
        vec![
            "/project/settings/one/first",
            "/project/settings/two/second",
        ],
        vec![
            "/project/settings/two/first",
            "/project/settings/one/second",
        ],
        vec![
            "/project/settings/two/first",
            "/project/settings/two/second",
        ],
    ]
    .into_iter()
    .collect();

    assert_eq!(template_dirs, expected);
    assert_eq!(
        cases(&settings, "/templates/cases")
            .expect("settings JSON pointer should identify an array")
            .len(),
        4
    );
}

#[test]
fn repeated_branch_selected_scalar_retains_two_feasible_settings_cases() {
    let repeated = iter::repeat_n("SHARED", 7).collect::<Vec<_>>().join(", ");
    let source = format!(
        "if FLAG:\n    from one.values import SHARED\nelse:\n    from two.values import SHARED\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{repeated}]}}]"
    );
    let settings = extract_project(
        &source,
        &[
            ("one.values", "SHARED = 'shared'"),
            ("two.values", "SHARED = 'shared'"),
        ],
    )
    .expect("settings extraction project should build")
    .2;

    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(cases.len(), 2);
    assert!(cases.iter().all(|case| case.get("known").is_some()));
    let roots = cases
        .iter()
        .map(|case| {
            let paths = case["known"]["backends"][0]["dirs"]
                .as_array()
                .expect("expected JSON field should be an array");
            assert_eq!(paths.len(), 7);
            let root = paths[0]["value"]
                .as_str()
                .expect("expected JSON field should be an array");
            assert!(
                paths
                    .iter()
                    .all(|path| path["value"].as_str() == Some(root))
            );
            root
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        roots,
        [
            "/project/settings/one/shared",
            "/project/settings/two/shared",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn typed_value_order_setting_lists_keep_cap_projection_for_reversed_input() {
    let at_limit = equal_top_level_list_alternatives(64, false)
        .expect("list alternatives should be extracted");
    let installed_apps = cases(&at_limit, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(installed_apps.len(), 1, "{at_limit:#}");
    assert_eq!(installed_apps[0]["known"]["apps"][0]["value"], "shared");
    assert_eq!(
        installed_apps[0]["known"]["apps"][0]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        64,
        "{at_limit:#}"
    );
    let templates = cases(&at_limit, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(templates.len(), 64, "{at_limit:#}");
    assert!(templates.iter().all(|case| case.get("known").is_some()));

    let overflowed = equal_top_level_list_alternatives(65, false)
        .expect("list alternatives should be extracted");
    assert_eq!(
        overflowed,
        equal_top_level_list_alternatives(65, true).expect("list alternatives should be extracted"),
        "top-level list projection must ignore equivalent reversed module installation"
    );

    let installed_apps = cases(&overflowed, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(installed_apps.len(), 2, "{overflowed:#}");
    assert_eq!(installed_apps[0]["known"]["apps"][0]["value"], "shared");
    let installed_remainder = installed_apps[1]["dynamic"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(installed_remainder.len(), 1, "{overflowed:#}");
    assert_eq!(
        installed_remainder[0]["issue"]["kind"],
        "dynamic_expression"
    );
    assert_eq!(
        installed_remainder[0]["issue"]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        2,
        "remainder should retain the omitted path and overflow operation: {overflowed:#}"
    );

    let templates = cases(&overflowed, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(templates.len(), 65, "{overflowed:#}");
    assert!(
        templates[..64]
            .iter()
            .all(|case| case.get("known").is_some()),
        "{overflowed:#}"
    );
    let template_remainder = templates[64]["dynamic"]["evidence"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(template_remainder.len(), 1, "{overflowed:#}");
    assert_eq!(template_remainder[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(
        template_remainder[0]["issue"]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1,
        "settings-level capping selects one fallback operation: {overflowed:#}"
    );
}

#[test]
fn capped_dynamic_remainder_merges_later_syntax_evidence() {
    let mut source = String::new();
    for index in 0..65 {
        if index == 0 {
            source.push_str("if FLAG_0:\n");
        } else if index == 64 {
            source.push_str("else:\n");
        } else {
            writeln!(source, "elif FLAG_{index}:")
                .expect("writing generated test source should succeed");
        }
        if index == 0 {
            source.push_str("    INSTALLED_APPS = False\n");
        } else {
            writeln!(source, "    INSTALLED_APPS = ['app_{index:02}']")
                .expect("writing generated test source should succeed");
        }
    }
    source.push_str("INSTALLED_APPS.append(@)\n");

    let settings = extract(&source).expect("Django settings extraction should succeed");
    let setting_cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(setting_cases.len(), 65, "{settings:#}");
    let malformed_cases = setting_cases
        .iter()
        .filter_map(|case| case.get("malformed"))
        .collect::<Vec<_>>();
    let malformed = only(&malformed_cases).expect("expected one Malformed case");
    assert_eq!(
        malformed["evidence"],
        json!([
            {
                "issue": {
                    "kind": "invalid_shape",
                    "spans": [expected_span(&source, "False").expect("test source should contain expected text")],
                }
            },
        ]),
        "the Malformed case must not absorb overflow or syntax evidence: {settings:#}"
    );

    let dynamic_cases = setting_cases
        .iter()
        .filter_map(|case| case.get("dynamic"))
        .collect::<Vec<_>>();
    let remainder = only(&dynamic_cases).expect("expected one capped Dynamic remainder");
    assert_eq!(
        remainder["evidence"],
        json!([
            {
                "issue": {
                    "kind": "dynamic_expression",
                    "spans": [Span::saturating_from_parts_usize(
                        0,
                        source.find("\nINSTALLED_APPS.append").expect("test source should contain the expected marker"),
                    )],
                }
            },
            {
                "issue": {
                    "kind": "syntax_error",
                    "spans": [
                        expected_span(&source, "@").expect("test source should contain expected text"),
                        expected_span(&source, ")").expect("test source should contain expected text"),
                    ],
                }
            },
        ]),
        "{settings:#}"
    );
}

#[test]
fn two_backends_sharing_a_branch_path_keep_only_feasible_settings_cases() {
    let settings = extract_project(
        "if FLAG:\n    from one.values import ROOT\nelse:\n    from two.values import ROOT\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT]},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT]},\n]",
        &[
            ("one.values", "ROOT = 'templates'"),
            ("two.values", "ROOT = 'templates'"),
        ],
    ).expect("settings extraction project should build")
    .2;

    let settings_cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(settings_cases.len(), 2, "{settings:#}");
    let roots = settings_cases
        .iter()
        .map(|case| {
            let backends = case["known"]["backends"]
                .as_array()
                .expect("expected JSON field should be an array");
            let first = backends[0]["dirs"][0]["value"]
                .as_str()
                .expect("expected JSON field should be an array");
            let second = backends[1]["dirs"][0]["value"]
                .as_str()
                .expect("expected JSON field should be an array");
            assert_eq!(first, second, "both backends must select the same branch");
            first
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        roots,
        [
            "/project/settings/one/templates",
            "/project/settings/two/templates",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn same_join_settings_reach_only_matching_template_winners() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project/settings")
        .django_settings_module("config.settings")
        .file(
            "/project/settings/config/settings.py",
            "if FLAG:\n    INSTALLED_APPS = ['first']\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/settings/a'], 'APP_DIRS': True}]\nelse:\n    INSTALLED_APPS = ['second']\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/settings/b'], 'APP_DIRS': True}]\n",
        )
        .file("/project/settings/a/shared.html", "a")
        .file("/project/settings/first/__init__.py", "")
        .file(
            "/project/settings/first/templates/shared.html",
            "impossible cross-pair",
        )
        .file("/project/settings/second/__init__.py", "")
        .file(
            "/project/settings/second/templates/shared.html",
            "second",
        )
        .build(&db).expect("template-resolution project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the two feasible settings branches should have different winners");
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        possible_paths,
        [
            "/project/settings/a/shared.html",
            "/project/settings/second/templates/shared.html",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn template_backend_products_have_a_global_deterministic_64_plus_one_cap() {
    let evaluate = |second_backend_width: usize| {
        let width = 3 + second_backend_width;
        let names = (0..width)
            .map(|index| format!("SHARED_{index}"))
            .collect::<Vec<_>>();
        let mut branches = String::new();
        for (index, name) in names.iter().enumerate() {
            write!(
                branches,
                "if FLAG_{index}:\n    from one.values import {name}\nelse:\n    from two.values import {name}\n"
            )
            .expect("writing generated test source should succeed");
        }
        let first = names[..3].join(", ");
        let second = names[3..].join(", ");
        let source = format!(
            "{branches}TEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{first}]}}, {{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{second}]}}]"
        );
        let mut module = String::new();
        for name in &names {
            writeln!(module, "{name} = 'shared'")
                .expect("writing generated test source should succeed");
        }
        extract_project(
            &source,
            &[
                ("one.values", module.as_str()),
                ("two.values", module.as_str()),
            ],
        )
        .expect("settings extraction project should build")
        .2
    };

    let at_limit = evaluate(3);
    let at_limit_cases = cases(&at_limit, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(at_limit_cases.len(), 64);
    assert!(at_limit_cases.iter().all(|case| {
        case["known"]["backends"]
            .as_array()
            .is_some_and(|backends| backends.len() == 2)
    }));

    let overflowed = evaluate(4);
    assert_eq!(
        overflowed,
        evaluate(4),
        "expansion order must be deterministic"
    );
    let overflowed = cases(&overflowed, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    assert_eq!(overflowed.len(), 65);
    assert!(overflowed[..64].iter().all(|case| {
        case["known"]["backends"]
            .as_array()
            .is_some_and(|backends| backends.len() == 2)
    }));
    let remainder = &overflowed[64]["dynamic"]["evidence"];
    assert_eq!(
        remainder
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
    assert_eq!(remainder[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(
        remainder[0]["issue"]["spans"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
}

#[test]
fn imported_template_fields_remain_correlated_alternatives() {
    let one = "PREFIX = None\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['first', 'second'], 'OPTIONS': {'context_processors': ['one.context.processor']}}]";
    let two = "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['first', 'second'], 'OPTIONS': {'context_processors': ['two.context.processor']}}]";
    let settings = extract_project(
        "if FLAG:\n    from one.base import TEMPLATES\nelse:\n    from two.base import TEMPLATES",
        &[("one.base", one), ("two.base", two)],
    )
    .expect("settings extraction project should build")
    .2;

    let template_cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");
    let template_dirs = template_cases
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"]
                .as_array()
                .expect("expected JSON field should be an array")
                .iter()
                .map(|dir| {
                    dir["value"]
                        .as_str()
                        .expect("expected JSON field should be an array")
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        template_dirs,
        [
            vec![
                "/project/settings/one/first",
                "/project/settings/one/second",
            ],
            vec![
                "/project/settings/two/first",
                "/project/settings/two/second",
            ],
        ]
    );

    let context_processors = template_cases
        .iter()
        .map(|case| &case["known"]["backends"][0]["context_processors"][0])
        .collect::<Vec<_>>();
    assert_eq!(
        context_processors,
        [
            &json!({
                "value": "one.context.processor",
                "spans": [expected_span(one, "'one.context.processor'").expect("test source should contain expected text")],
            }),
            &json!({
                "value": "two.context.processor",
                "spans": [expected_span(two, "'two.context.processor'").expect("test source should contain expected text")],
            }),
        ]
    );
}

#[test]
fn equal_mixed_origin_path_lists_retain_each_original_settings_case() {
    let settings = extract_project(
        "if FLAG:\n    from one.base import TEMPLATES\nelse:\n    from two.base import TEMPLATES",
        &[
            ("one.values", "FIRST = 'first'\nSECOND = 'second'"),
            ("two.values", "FIRST = 'first'\nSECOND = 'second'"),
            (
                "one.base",
                "from one.values import FIRST\nfrom two.values import SECOND\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [FIRST, SECOND]}]",
            ),
            (
                "two.base",
                "from two.values import FIRST\nfrom one.values import SECOND\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [FIRST, SECOND]}]",
            ),
        ],
    ).expect("settings extraction project should build")
    .2;

    let expected = [
        vec![
            "/project/settings/one/first",
            "/project/settings/two/second",
        ],
        vec![
            "/project/settings/two/first",
            "/project/settings/one/second",
        ],
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let template_dirs = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"]
                .as_array()
                .expect("expected JSON field should be an array")
                .iter()
                .map(|dir| {
                    dir["value"]
                        .as_str()
                        .expect("expected JSON field should be an array")
                })
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(template_dirs, expected);
}

#[test]
fn all_setting_families_distinguish_known_unset_dynamic_and_malformed() {
    let known = extract("INSTALLED_APPS = []\nTEMPLATES = []")
        .expect("Django settings extraction should succeed");
    let unset = extract("").expect("Django settings extraction should succeed");
    let dynamic = extract("INSTALLED_APPS = unknown\nTEMPLATES = unknown")
        .expect("Django settings extraction should succeed");
    let malformed = extract("INSTALLED_APPS = False\nTEMPLATES = False")
        .expect("Django settings extraction should succeed");
    for pointer in ["/installed_apps/cases", "/templates/cases"] {
        assert!(
            cases(&known, pointer).expect("settings JSON pointer should identify an array")[0]
                .get("known")
                .is_some(),
            "{pointer}"
        );
        assert_eq!(
            cases(&unset, pointer).expect("settings JSON pointer should identify an array"),
            [json!("unset")],
            "{pointer}"
        );

        let dynamic_case =
            &cases(&dynamic, pointer).expect("settings JSON pointer should identify an array")[0];
        let malformed_case =
            &cases(&malformed, pointer).expect("settings JSON pointer should identify an array")[0];
        let dynamic_payload = dynamic_case["dynamic"]
            .as_object()
            .expect("expected JSON value should be an object");
        let malformed_payload = malformed_case["malformed"]
            .as_object()
            .expect("expected JSON value should be an object");
        assert_eq!(
            dynamic_case
                .as_object()
                .expect("expected JSON value should be an object")
                .len(),
            1,
            "{pointer}"
        );
        assert_eq!(
            malformed_case
                .as_object()
                .expect("expected JSON value should be an object")
                .len(),
            1,
            "{pointer}"
        );
        assert_eq!(
            dynamic_payload
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["evidence"],
            "{pointer}"
        );
        assert_eq!(
            malformed_payload
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["evidence"],
            "{pointer}"
        );
    }
}

#[test]
fn unknown_backend_key_weakens_prior_claim_and_later_exact_key_is_authoritative() {
    let weakened =
        extract("TEMPLATES = [{'BACKEND': 'before.backend', unknown_key: 'maybe.backend'}]")
            .expect("Django settings extraction should succeed");
    let backend = &cases(&weakened, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"];
    assert_eq!(backend["backend"]["known"]["value"], "before.backend");
    assert_eq!(
        backend["backend"]["issues"][0]["kind"],
        "dynamic_expression"
    );

    let restored = extract(
        "TEMPLATES = [{'BACKEND': 'before.backend', unknown_key: 'maybe.backend', 'BACKEND': 'after.backend'}]",
    ).expect("Django settings extraction should succeed");
    let backend = &cases(&restored, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"];
    assert_eq!(backend["backend"]["known"]["value"], "after.backend");
    assert!(
        backend["backend"]["issues"]
            .as_array()
            .expect("expected JSON field should be an array")
            .is_empty()
    );
}

#[test]
fn unknown_library_key_weakens_prior_alias_and_later_exact_key_is_authoritative() {
    let weakened = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'alias': 'before.tags', unknown_key: 'maybe.tags'}}}]",
    ).expect("Django settings extraction should succeed");
    let libraries = &cases(&weakened, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"]
        ["libraries"];
    assert!(
        libraries["known"]
            .as_array()
            .expect("expected JSON field should be an array")
            .is_empty()
    );
    assert_eq!(libraries["issues"][0]["kind"], "dynamic_expression");

    let restored = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'alias': 'before.tags', unknown_key: 'maybe.tags', 'alias': 'after.tags'}}}]",
    ).expect("Django settings extraction should succeed");
    let libraries = &cases(&restored, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"]
        ["libraries"];
    assert_eq!(
        libraries["known"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
    assert_eq!(libraries["known"][0][0], "alias");
    assert_eq!(libraries["known"][0][1]["value"], "after.tags");
    assert_eq!(libraries["issues"][0]["kind"], "dynamic_expression");
}

#[test]
fn canonical_unknown_origins_project_mapping_unpack_spans() {
    let source = "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {**first()}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {**second()}}}]\n";
    let settings = extract(source).expect("Django settings extraction should succeed");
    let libraries = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"]
        ["libraries"];
    let issues = libraries["issues"]
        .as_array()
        .expect("unknown mapping unpack should produce issues");
    let issue = only(issues).expect("equal unknown mapping branches should produce one issue");
    assert_eq!(issue["kind"], "unknown_unpack");
    assert_eq!(
        issue["spans"],
        to_value([
            expected_span(source, "first()").expect("test source should contain expected text"),
            expected_span(source, "second()").expect("test source should contain expected text"),
        ])
        .expect("expected JSON field should be an array")
    );
}

#[test]
fn unknown_library_unpack_removes_prior_authority_but_later_entry_wins() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'before': 'before.tags', **unknown, 'after': 'after.tags'}}}]",
    ).expect("Django settings extraction should succeed");
    let libraries = &cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array")[0]["dynamic"]["evidence"][0]["backend"]
        ["libraries"];

    assert_eq!(
        libraries["known"]
            .as_array()
            .expect("expected JSON field should be an array")
            .len(),
        1
    );
    assert_eq!(libraries["known"][0][0], "after");
    assert_eq!(libraries["issues"][0]["kind"], "unknown_unpack");
}

#[test]
fn imports_feed_values_and_semantic_dependencies() {
    let (db, project, settings) = extract_project(
        "from base import INSTALLED_APPS",
        &[("base", "INSTALLED_APPS = ['base']")],
    )
    .expect("settings extraction project should build");
    assert_eq!(
        cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]["known"]["apps"][0]["value"],
        "base"
    );
    assert_eq!(
        compute_project_facts(&db, project).file_paths(),
        [
            Utf8PathBuf::from("/project/settings/base.py"),
            Utf8PathBuf::from("/project/settings/config/settings.py"),
        ]
    );
}

#[test]
fn unreachable_import_is_not_a_dependency() {
    let (db, project, settings) = extract_project(
        "if False:\n    from base import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['local']",
        &[("base", "INSTALLED_APPS = [")],
    )
    .expect("settings extraction project should build");
    assert_eq!(
        cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]["known"]["apps"][0]["value"],
        "local"
    );
    assert_eq!(
        compute_project_facts(&db, project).file_paths(),
        [Utf8PathBuf::from("/project/settings/config/settings.py")]
    );
}
