use std::fmt::Write as _;
use std::io;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::TagSpecDef;
use djls_project::FindTemplateResult;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::PythonModule;
use djls_project::PythonModuleName;
use djls_project::SearchPaths;
use djls_project::TemplateName;
use djls_project::template_resolution;
use djls_project::testing::PythonBindingAlternativeView;
use djls_project::testing::PythonBoundValueView;
use djls_project::testing::PythonImportErrorView;
use djls_project::testing::PythonImportOutcomeView;
use djls_project::testing::PythonListItemView;
use djls_project::testing::PythonMutationOperationView;
use djls_project::testing::PythonMutationPathSegmentView;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::PythonUnknownCauseView;
use djls_project::testing::PythonValueKindView;
use djls_project::testing::compute_project_facts;
use djls_project::testing::django_settings;
use djls_project::testing::python_module_evaluation;
use djls_project::testing::python_module_evaluation_for_module;
use djls_project::testing::settings_module_file;
use djls_source::ChangeEvent;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::RootWalk;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_source::WalkOptions;
use djls_source::path_to_file;
use djls_testing::OsTestDatabase;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use serde_json::Value;
use serde_json::json;

fn extract_project(source: &str, modules: &[(&str, &str)]) -> (TestDatabase, Project, Value) {
    let mut fixture = ProjectFixture::new("/project/settings")
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
    let project = fixture.install(&mut db);
    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();
    (db, project, settings)
}

fn extract(source: &str) -> Value {
    extract_project(source, &[]).2
}

fn cases<'a>(settings: &'a Value, pointer: &str) -> &'a [Value] {
    settings.pointer(pointer).unwrap().as_array().unwrap()
}

fn binding_unknown_origin(source: &str, name: &str) -> djls_source::Origin {
    let (db, project, _) = extract_project(source, &[]);
    let file = settings_module_file(&db, project).unwrap();
    let evaluation = python_module_evaluation(&db, project, file);
    let binding = evaluation.binding(name).unwrap();
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("expected one bound alternative for {name}");
    };
    let PythonValueKindView::Unknown(unknown) = &bound.value.kind else {
        panic!("expected unknown value for {name}");
    };
    assert_eq!(unknown.cause, PythonUnknownCauseView::UnsupportedMutation);
    unknown.origin.unwrap()
}

fn python_project(db: &dyn djls_project::Db) -> Project {
    python_project_with_paths(db, &[])
}

fn python_project_with_paths(db: &dyn djls_project::Db, pythonpath: &[Utf8PathBuf]) -> Project {
    let root = Utf8Path::new("/project");
    let interpreter = Interpreter::Auto;
    let search_paths =
        SearchPaths::from_project_settings(db.file_system(), root, &interpreter, pythonpath);
    search_paths.register_roots(db);
    Project::new(
        db,
        root.to_path_buf(),
        search_paths,
        interpreter,
        None,
        Vec::new(),
        Vec::new(),
        TagSpecDef::default(),
    )
}

fn expected_span(source: &str, needle: &str) -> Span {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("expected source to contain {needle:?}"));
    Span::saturating_from_parts_usize(start, needle.len())
}

struct ReadFailingFileSystem {
    inner: InMemoryFileSystem,
    unreadable: Utf8PathBuf,
}

impl FileSystem for ReadFailingFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        if path == self.unreadable {
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

fn is_alternative_limit_unknown(alternative: &PythonBindingAlternativeView) -> bool {
    matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if unknown.cause == PythonUnknownCauseView::AlternativeLimitExceeded
    )
}

fn branch_alternatives(count: usize) -> String {
    let mut source = String::new();
    for index in 0..count {
        if index == 0 {
            source.push_str("if condition_0:\n");
        } else {
            source.push_str(format!("elif condition_{index}:\n").as_str());
        }
        source.push_str(format!("    VALUE = '{index:02}'\n").as_str());
    }
    source
}

fn equal_top_level_list_alternatives(count: usize) -> Value {
    let mut source = String::new();
    let mut modules = Vec::new();
    for index in 0..count {
        if index == 0 {
            source.push_str("if FLAG_0:\n");
        } else if index + 1 == count {
            source.push_str("else:\n");
        } else {
            writeln!(source, "elif FLAG_{index}:").unwrap();
        }
        writeln!(
            source,
            "    from variant_{index:02} import INSTALLED_APPS, TEMPLATES"
        )
        .unwrap();
        modules.push((
            format!("variant_{index:02}"),
            "INSTALLED_APPS = ['shared']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates']}]\n",
        ));
    }
    let module_refs = modules
        .iter()
        .map(|(name, body)| (name.as_str(), *body))
        .collect::<Vec<_>>();
    extract_project(&source, &module_refs).2
}

#[test]
fn python_module_evaluation_follows_recursive_imports() {
    let db = TestDatabase::new();
    db.add_file("/project/base.py", "VALUE = 'from base'\n");
    db.add_file(
        "/project/settings.py",
        "from base import VALUE\nCOPY = VALUE\n",
    );
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);

    let value = evaluation.binding("VALUE").expect("VALUE should be bound");
    let copy = evaluation.binding("COPY").expect("COPY should be bound");
    assert_eq!(value.alternatives.len(), 1);
    assert_eq!(copy.alternatives.len(), 1);
    assert_eq!(evaluation.dependency_files.len(), 2);
    assert!(evaluation
        .imports
        .iter()
        .any(|outcome| matches!(outcome, PythonImportOutcomeView::Resolved { file, .. } if *file == db.file(Utf8Path::new("/project/base.py")))));
}

#[test]
fn python_binding_alternative_limit_has_exact_boundary_and_unknown_remainder() {
    let evaluate = |count| {
        let db = TestDatabase::new();
        db.add_file("/project/settings.py", branch_alternatives(count).as_str());
        let project = python_project(&db);
        let settings = db.file(Utf8Path::new("/project/settings.py"));
        python_module_evaluation(&db, project, settings)
    };

    let at_limit = evaluate(63);
    let at_limit = at_limit.binding("VALUE").expect("VALUE should be bound");
    assert_eq!(at_limit.alternatives.len(), 64);
    assert!(
        at_limit
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
    assert!(
        !at_limit
            .alternatives
            .iter()
            .any(is_alternative_limit_unknown)
    );

    let overflowed = evaluate(64);
    let overflowed = overflowed.binding("VALUE").expect("VALUE should be bound");
    assert_eq!(overflowed.alternatives.len(), 65);
    assert!(
        overflowed
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
    assert!(
        overflowed
            .alternatives
            .iter()
            .any(is_alternative_limit_unknown)
    );
}

#[test]
fn python_evaluator_produces_unbound_for_a_path_without_assignment() {
    let db = TestDatabase::new();
    let source = "if condition:\n    VALUE = 'set'\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation
        .binding("VALUE")
        .expect("VALUE should be tracked");

    assert_eq!(binding.alternatives.len(), 2);
    assert!(
        binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Str(value),
                origins,
            },
            binding_origins,
        }) if value == "set"
            && origins.as_slice() == [djls_source::Origin::new(settings, expected_span(source, "'set'"))]
            && binding_origins.as_slice() == [djls_source::Origin::new(settings, expected_span(source, "'set'"))]
    )));
}

#[test]
fn python_binding_normalizes_nested_unknowns_and_merges_their_evidence() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "if condition:\n    VALUE = [dynamic()]\nelse:\n    VALUE = [other_dynamic()]\n",
    );
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation.binding("VALUE").expect("VALUE should be bound");
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("nested unknowns should normalize into one bound alternative");
    };
    let PythonValueKindView::List(items) = &bound.value.kind else {
        panic!("VALUE should remain a list");
    };
    let [PythonListItemView::UnknownElement(unknown)] = items.as_slice() else {
        panic!("the list should contain one typed unknown element");
    };
    assert_eq!(unknown.cause, PythonUnknownCauseView::UnsupportedExpression);
    assert_eq!(
        unknown
            .origin
            .expect("unknown should retain an origin")
            .span
            .start(),
        27
    );
    assert_eq!(
        bound
            .value
            .origins
            .iter()
            .map(|origin| origin.span.start())
            .collect::<Vec<_>>(),
        [26, 56],
    );
}

#[test]
fn python_module_evaluation_records_only_path_feasible_imports() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "if False:\n    from unreachable import VALUE\nif condition:\n    from feasible import VALUE\n",
    );
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    assert_eq!(evaluation.imports.len(), 1);
    assert!(matches!(
        &evaluation.imports[0],
        PythonImportOutcomeView::NotFound { module, .. } if module.as_str() == "feasible"
    ));
}

#[test]
fn unresolved_moduleless_relative_import_records_canonical_failure() {
    let db = TestDatabase::new();
    let source = "VALUE = 'local'\nfrom . import VALUE\n";
    db.add_file("/project/pkg/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/pkg/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::NotFound { origin, module }]
            if *origin == djls_source::Origin::new(
                settings,
                expected_span(source, "from . import VALUE"),
            ) && module.as_str() == "pkg"
    ));
    let binding = evaluation
        .binding("VALUE")
        .expect("failed relative import should replace the prior binding");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module)
            if module.as_str() == "pkg")
    ));
}

#[test]
fn relative_import_cannot_escape_the_importer_package() {
    let db = TestDatabase::new();
    let source = "from ..missing import VALUE\n";
    db.add_file("/project/pkg/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/pkg/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::InvalidImport { origin, reason }]
            if *origin == djls_source::Origin::new(
                settings,
                expected_span(source, source.trim_end()),
            ) && *reason == PythonImportErrorView::TooManyDots
    ));
}

#[test]
fn relative_import_uses_the_inbound_module_identity() {
    let db = TestDatabase::new();
    let source = "from .sibling import VALUE\n";
    db.add_file("/project/lib/pkg/settings.py", source);
    db.add_file(
        "/project/pkg/sibling.py",
        "VALUE = 'inbound module identity'\n",
    );
    db.add_file(
        "/project/lib/pkg/sibling.py",
        "VALUE = 'canonical file identity'\n",
    );
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/project/lib")]);
    let module = PythonModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.settings").unwrap(),
    )
    .expect("pkg.settings should resolve through the nested search path");
    let sibling = db.file(Utf8Path::new("/project/pkg/sibling.py"));

    let evaluation = python_module_evaluation_for_module(&db, project, module.clone());

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::Resolved { file, .. }] if *file == sibling
    ));
    let binding = evaluation
        .binding("VALUE")
        .expect("relative import should bind VALUE");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "inbound module identity"
    ));

    let canonical = python_module_evaluation(&db, project, module.file());
    let canonical_binding = canonical
        .binding("VALUE")
        .expect("canonical file identity should also be evaluated");
    assert!(matches!(
        canonical_binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "canonical file identity"
    ));
}

#[test]
fn relative_import_from_package_init_alias_uses_parent_package() {
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "from .base import VALUE\n");
    db.add_file("/project/pkg/base.py", "VALUE = 'package value'\n");
    let project = python_project(&db);
    let module = PythonModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.__init__").unwrap(),
    )
    .expect("pkg.__init__ should resolve as a file-module alias");

    let evaluation = python_module_evaluation_for_module(&db, project, module);
    let binding = evaluation
        .binding("VALUE")
        .expect("the package-relative import should bind VALUE");

    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "package value"
    ));
}

#[test]
fn python_module_evaluation_keeps_typed_import_and_namespace_outcomes() {
    let db = TestDatabase::new();
    let source = "from missing_named import VALUE\nfrom missing_star import *\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    let not_found = evaluation
        .imports
        .iter()
        .map(|outcome| match outcome {
            PythonImportOutcomeView::NotFound { origin, module } => {
                (origin.file, origin.span, module.as_str())
            }
            _ => panic!("expected only typed not-found outcomes"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        not_found,
        [
            (
                settings,
                expected_span(source, "from missing_named import VALUE"),
                "missing_named"
            ),
            (
                settings,
                expected_span(source, "from missing_star import *"),
                "missing_star"
            ),
        ]
    );
    let value = evaluation.binding("VALUE").expect("VALUE should be bound");
    assert!(value.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing_named")
    )));
    assert!(evaluation.namespace_open());
    assert!(matches!(
        evaluation.namespace_unknowns.as_slice(),
        [unknown] if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing_star")
    ));
}

#[test]
fn named_import_of_absent_open_name_preserves_unset_and_typed_dynamic_outcomes() {
    let source = "from plugin import INSTALLED_APPS\n";
    let (db, project, settings) = extract_project(
        source,
        &[("plugin", "if ENABLED:\n    from missing import *\n")],
    );
    let settings_file = settings_module_file(&db, project).unwrap();
    let evaluation = python_module_evaluation(&db, project, settings_file);
    let binding = evaluation
        .binding("INSTALLED_APPS")
        .expect("the named import should retain absence and namespace uncertainty");

    assert!(
        binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            binding_origins,
        }) if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing")
            && unknown.origin == Some(djls_source::Origin::new(
                settings_file,
                expected_span(source, "INSTALLED_APPS"),
            ))
            && binding_origins.as_slice() == [djls_source::Origin::new(
                settings_file,
                expected_span(source, "INSTALLED_APPS"),
            )]
    )));

    let cases = cases(&settings, "/installed_apps/cases");
    assert_eq!(cases.len(), 2, "{settings:#}");
    assert!(cases.iter().any(|case| case == "unset"));
    assert!(cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn named_import_of_conditional_binding_preserves_known_unset_and_dynamic_outcomes() {
    let settings = extract_project(
        "from plugin import INSTALLED_APPS\n",
        &[(
            "plugin",
            "if ENABLED:\n    INSTALLED_APPS = ['imported']\n    from missing import *\n",
        )],
    )
    .2;
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 3, "{settings:#}");
    assert!(cases.iter().any(|case| case == "unset"));
    assert!(cases.iter().any(|case| case.get("dynamic").is_some()));
    assert!(cases.iter().any(|case| {
        case.pointer("/known/apps/0/value") == Some(&serde_json::json!("imported"))
    }));
}

#[test]
fn star_import_translates_every_typed_namespace_cause_to_the_import_site() {
    let source = "VALUE = 'local'\nfrom plugin import *\n";
    let (db, project, _) = extract_project(
        source,
        &[(
            "plugin",
            "if ENABLED:\n    from missing import *\nelse:\n    from ...invalid import *\n",
        )],
    );
    let settings_file = settings_module_file(&db, project).unwrap();
    let evaluation = python_module_evaluation(&db, project, settings_file);
    let binding = evaluation
        .binding("VALUE")
        .expect("VALUE should remain bound");
    let unknowns = binding
        .alternatives
        .iter()
        .filter_map(|alternative| match alternative {
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value:
                    djls_project::testing::PythonValueView {
                        kind: PythonValueKindView::Unknown(unknown),
                        ..
                    },
                ..
            }) => Some(unknown),
            PythonBindingAlternativeView::Bound(_) | PythonBindingAlternativeView::Unbound => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(unknowns.len(), 2, "{evaluation:#?}");
    assert!(unknowns.iter().any(|unknown| matches!(
        &unknown.cause,
        PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing"
    )));
    assert!(unknowns.iter().any(|unknown| {
        unknown.cause == PythonUnknownCauseView::InvalidImport(PythonImportErrorView::TooManyDots)
    }));
    assert!(unknowns.iter().all(|unknown| {
        unknown.origin
            == Some(djls_source::Origin::new(
                settings_file,
                expected_span(source, "from plugin import *"),
            ))
    }));
}

#[test]
fn deterministic_false_while_executes_only_else_body() {
    let source = "while False:\n    from missing_body import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['else']\n";
    let (db, project, settings) = extract_project(source, &[]);
    assert_eq!(
        cases(&settings, "/installed_apps/cases")[0]["known"]["apps"][0]["value"],
        "else"
    );

    let file = settings_module_file(&db, project).unwrap();
    let evaluation = python_module_evaluation(&db, project, file);
    assert!(
        evaluation.imports.is_empty(),
        "the unreachable while body must contribute no import outcome"
    );
}

#[test]
fn ambiguous_while_degrades_writes_and_retains_branch_effects() {
    let source = "INSTALLED_APPS = []\nwhile FLAG:\n    INSTALLED_APPS.append('loop')\nelse:\n    from plugin import STATIC_URL\n";
    let (db, project, settings) = extract_project(source, &[("plugin", "STATIC_URL = '/static/'")]);
    let app_cases = cases(&settings, "/installed_apps/cases");
    assert_eq!(app_cases.len(), 2, "{settings:#}");
    assert_eq!(
        app_cases[0]["known"]["apps"].as_array().unwrap().len(),
        0,
        "{settings:#}"
    );
    assert_eq!(
        app_cases[1]["dynamic"]["apps"]["evidence"][0]["issue"]["kind"], "dynamic_expression",
        "{settings:#}"
    );
    assert_eq!(
        cases(&settings, "/staticfiles/static_url/cases")[0]["dynamic"]["issues"][0]["kind"],
        "dynamic_expression",
        "{settings:#}"
    );

    let file = settings_module_file(&db, project).unwrap();
    let plugin = db.file(Utf8Path::new("/project/settings/plugin.py"));
    let evaluation = python_module_evaluation(&db, project, file);
    assert!(evaluation.dependency_files.contains(&plugin));
    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::Resolved { file: imported, .. }] if *imported == plugin
    ));
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation]
            if mutation.binding == "INSTALLED_APPS"
                && mutation.operation == PythonMutationOperationView::Append
                && mutation.path.is_empty()
                && mutation.origin
                    == djls_source::Origin::new(
                        file,
                        expected_span(source, "INSTALLED_APPS.append('loop')"),
                    )
    ));
}

#[test]
fn ambiguous_branch_annotation_only_name_is_absent() {
    let db = TestDatabase::new();
    db.add_file("/project/settings.py", "if FLAG:\n    VALUE: str\n");
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(evaluation.binding("VALUE").is_none());
}

#[test]
fn ambiguous_branch_skips_nested_deterministically_dead_write() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "if FLAG:\n    if False:\n        VALUE = 'dead'\n",
    );
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(evaluation.binding("VALUE").is_none());
}

#[test]
fn ambiguous_branch_same_value_reassignment_preserves_origins() {
    let db = TestDatabase::new();
    let source = "VALUE = 'same'\nif FLAG:\n    VALUE = 'same'\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation.binding("VALUE").expect("VALUE should be bound");
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("the equal values should normalize into one bound alternative");
    };
    let expected_origins = source
        .match_indices("'same'")
        .map(|(start, value)| {
            djls_source::Origin::new(
                settings,
                Span::saturating_from_parts_usize(start, value.len()),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(bound.value.origins, expected_origins);
    assert_eq!(bound.binding_origins, expected_origins);
}

#[test]
fn ambiguous_branch_restoration_preserves_only_base_and_restoration_origins() {
    let db = TestDatabase::new();
    let source = "VALUE = 'base'\nif FLAG:\n    VALUE = 'temporary'\n    VALUE = 'base'\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation.binding("VALUE").expect("VALUE should be bound");
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("the restored value should normalize into one bound alternative");
    };
    let expected_origins = source
        .match_indices("'base'")
        .map(|(start, value)| {
            djls_source::Origin::new(
                settings,
                Span::saturating_from_parts_usize(start, value.len()),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(bound.value.origins, expected_origins);
    assert_eq!(bound.binding_origins, expected_origins);
    assert!(
        bound
            .value
            .origins
            .iter()
            .all(|origin| origin.span != expected_span(source, "'temporary'"))
    );
}

#[test]
fn ambiguous_branch_append_remove_retains_mutation_evidence() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nif FLAG:\n    VALUES.append('item')\n    VALUES.remove('item')\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("the restored list should normalize into one bound alternative");
    };
    let PythonValueKindView::List(items) = &bound.value.kind else {
        panic!("VALUES should remain a list");
    };

    assert!(items.is_empty());
    assert_eq!(bound.value.origins.len(), 3);
    assert_eq!(evaluation.mutations.len(), 2);
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.binding == "VALUES"
            && mutation.operation == PythonMutationOperationView::Append
            && mutation.origin.span == expected_span(source, "VALUES.append('item')")
    }));
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.binding == "VALUES"
            && mutation.operation == PythonMutationOperationView::Remove
            && mutation.origin.span == expected_span(source, "VALUES.remove('item')")
    }));
}

#[test]
fn ambiguous_branch_reassignment_clears_prior_mutation_evidence() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nif FLAG:\n    VALUES.append('stale')\n    VALUES = []\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("the reassigned lists should normalize into one bound alternative");
    };

    assert!(evaluation.mutations.is_empty());
    assert_eq!(bound.value.origins.len(), 2);
    assert!(
        bound
            .value
            .origins
            .iter()
            .all(|origin| origin.span != expected_span(source, "VALUES.append('stale')"))
    );
}

#[test]
fn ambiguous_branch_noop_extend_retains_mutation_evidence() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nif FLAG:\n    VALUES.extend([])\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("the empty lists should normalize into one bound alternative");
    };

    assert_eq!(bound.value.origins.len(), 2);
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation]
            if mutation.binding == "VALUES"
                && mutation.operation == PythonMutationOperationView::Extend
                && mutation.origin.span == expected_span(source, "VALUES.extend([])")
    ));
}

#[test]
fn ambiguous_branch_mutations_remain_uncorrelated_may_have_evidence() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nif FLAG:\n    VALUES.append('item')\nelse:\n    VALUES.extend([])\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let mut list_lengths = binding
        .alternatives
        .iter()
        .map(|alternative| match alternative {
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value:
                    djls_project::testing::PythonValueView {
                        kind: PythonValueKindView::List(items),
                        ..
                    },
                ..
            }) => items.len(),
            PythonBindingAlternativeView::Bound(_) | PythonBindingAlternativeView::Unbound => {
                panic!("both branches should retain exact list values")
            }
        })
        .collect::<Vec<_>>();
    list_lengths.sort_unstable();

    assert_eq!(list_lengths, [0, 1]);
    assert_eq!(evaluation.mutations.len(), 2);
    assert!(
        evaluation
            .mutations
            .iter()
            .any(|mutation| mutation.operation == PythonMutationOperationView::Append)
    );
    assert!(
        evaluation
            .mutations
            .iter()
            .any(|mutation| mutation.operation == PythonMutationOperationView::Extend)
    );
}

#[test]
fn ambiguous_match_joins_only_evaluated_pattern_and_body_names() {
    let db = TestDatabase::new();
    let source = "match SUBJECT:\n    case {'item': ITEM} if GUARD:\n        VALUE = 'matched'\n        DEAD: str\n    case _:\n        FALLBACK = 'fallback'\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);

    for name in ["ITEM", "VALUE", "FALLBACK"] {
        let binding = evaluation
            .binding(name)
            .unwrap_or_else(|| panic!("{name} should participate in the match join"));
        assert_eq!(binding.alternatives.len(), 2, "{name}");
        assert!(
            binding
                .alternatives
                .contains(&PythonBindingAlternativeView::Unbound),
            "{name}"
        );
        assert!(
            binding
                .alternatives
                .iter()
                .any(|alternative| matches!(alternative, PythonBindingAlternativeView::Bound(_))),
            "{name}"
        );
    }
    assert!(evaluation.binding("DEAD").is_none());
}

#[test]
fn python_module_evaluation_canonicalizes_branch_mutation_order() {
    let db = TestDatabase::new();
    let source = "A_VALUES = []\nZ_VALUES = []\nif FLAG:\n    Z_VALUES.append('z')\nelse:\n    A_VALUES.append('a')\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);

    assert_eq!(
        evaluation
            .mutations
            .iter()
            .map(|mutation| mutation.binding.as_str())
            .collect::<Vec<_>>(),
        ["A_VALUES", "Z_VALUES"]
    );
    assert_eq!(
        evaluation.mutations[0].origin.span,
        expected_span(source, "A_VALUES.append('a')")
    );
    assert_eq!(
        evaluation.mutations[1].origin.span,
        expected_span(source, "Z_VALUES.append('z')")
    );
}

#[test]
fn python_module_evaluation_keeps_failed_star_import_from_loop_body() {
    let db = TestDatabase::new();
    let source = "for item in ITEMS:\n    from missing_star import *\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::NotFound { origin, module }]
            if origin.file == settings
                && origin.span == expected_span(source, "from missing_star import *")
                && module.as_str() == "missing_star"
    ));
    assert!(matches!(
        evaluation.namespace_unknowns.as_slice(),
        [unknown]
            if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing_star")
                && unknown.origin == Some(djls_source::Origin::new(
                    settings,
                    expected_span(source, "from missing_star import *"),
                ))
    ));
}

#[test]
fn python_module_evaluation_reports_invalid_import_with_typed_cause() {
    let db = TestDatabase::new();
    let source = "from ...missing import VALUE\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::InvalidImport { origin, reason }]
            if origin.file == settings
                && origin.span == expected_span(source, source.trim_end())
                && *reason == PythonImportErrorView::TooManyDots
    ));
    let binding = evaluation
        .binding("VALUE")
        .expect("failed named import should bind unknown");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::InvalidImport(PythonImportErrorView::TooManyDots)
            && unknown.origin.expect("unknown should retain import origin").file == settings
    ));
}

#[test]
fn python_module_evaluation_follows_named_and_star_imports_from_extra_roots() {
    for source in ["from shared import VALUE\n", "from shared import *\n"] {
        let db = TestDatabase::new();
        db.add_file("/vendor/shared.py", "VALUE = 'extra'\n");
        db.add_file("/project/settings.py", source);
        let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor")]);
        let settings = db.file(Utf8Path::new("/project/settings.py"));
        let shared = db.file(Utf8Path::new("/vendor/shared.py"));
        let evaluation = python_module_evaluation(&db, project, settings);

        assert!(matches!(
            evaluation.imports.as_slice(),
            [PythonImportOutcomeView::Resolved { file, .. }] if *file == shared
        ));
        assert_eq!(evaluation.dependency_files, [settings, shared]);
        assert!(matches!(
            evaluation.binding("VALUE").unwrap().alternatives.as_slice(),
            [PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: djls_project::testing::PythonValueView {
                    kind: PythonValueKindView::Str(value),
                    ..
                },
                ..
            })] if value == "extra"
        ));
    }
}

#[test]
fn python_module_evaluation_reports_skipped_external_import() {
    let db = TestDatabase::new();
    db.add_file("/vendor/site-packages/external.py", "VALUE = 'external'\n");
    let source = "from external import VALUE\n";
    db.add_file("/project/settings.py", source);
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor/site-packages")]);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::SkippedExternal { origin, module }]
            if origin.file == settings && module.as_str() == "external"
    ));
    let binding = evaluation
        .binding("VALUE")
        .expect("skipped import should bind unknown");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if matches!(&unknown.cause, PythonUnknownCauseView::SkippedExternal(module) if module.as_str() == "external")
    ));
}

#[test]
fn python_module_evaluation_skips_external_star_import() {
    let db = TestDatabase::new();
    db.add_file("/vendor/site-packages/external.py", "VALUE = 'external'\n");
    let source = "from external import *\n";
    db.add_file("/project/settings.py", source);
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor/site-packages")]);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::SkippedExternal { origin, module }]
            if origin.file == settings && module.as_str() == "external"
    ));
    assert!(evaluation.binding("VALUE").is_none());
    assert_eq!(evaluation.dependency_files, [settings]);
    assert!(matches!(
        evaluation.namespace_unknowns.as_slice(),
        [unknown]
            if matches!(&unknown.cause, PythonUnknownCauseView::SkippedExternal(module) if module.as_str() == "external")
    ));
}

#[test]
fn python_module_evaluation_skips_named_and_star_imports_from_editable_roots() {
    for source in ["from external import VALUE\n", "from external import *\n"] {
        let db = TestDatabase::new();
        db.add_file("/vendor/site-packages/project.pth", "/editable-package\n");
        db.add_file("/editable-package/external.py", "VALUE = 'external'\n");
        db.add_file("/project/settings.py", source);
        let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor/site-packages")]);
        let settings = db.file(Utf8Path::new("/project/settings.py"));
        let evaluation = python_module_evaluation(&db, project, settings);

        assert!(matches!(
            evaluation.imports.as_slice(),
            [PythonImportOutcomeView::SkippedExternal { origin, module }]
                if origin.file == settings && module.as_str() == "external"
        ));
        assert_eq!(evaluation.dependency_files, [settings]);
    }
}

#[test]
fn python_module_evaluation_reports_unreadable_import() {
    let mut inner = InMemoryFileSystem::new();
    inner.add_file(
        "/project/unreadable.py".into(),
        "VALUE = 'hidden'\n".to_string(),
    );
    let source = "from unreadable import VALUE\n";
    inner.add_file("/project/settings.py".into(), source.to_string());
    let fs = ReadFailingFileSystem {
        inner,
        unreadable: "/project/unreadable.py".into(),
    };
    let db = OsTestDatabase::with_file_system(Arc::new(fs));
    let project = python_project(&db);
    let unreadable = path_to_file(&db, Utf8Path::new("/project/unreadable.py"))
        .expect("unreadable fixture should still be discoverable");
    let settings = path_to_file(&db, Utf8Path::new("/project/settings.py"))
        .expect("settings fixture should exist");
    let evaluation = python_module_evaluation(&db, project, settings);

    let [
        PythonImportOutcomeView::Unreadable {
            origin,
            file,
            error,
            ..
        },
    ] = evaluation.imports.as_slice()
    else {
        panic!("expected one typed unreadable import outcome");
    };
    assert_eq!(origin.file, settings);
    assert_eq!(*file, unreadable);
    assert_eq!(error.path, Utf8Path::new("/project/unreadable.py"));
    assert_eq!(error.kind, io::ErrorKind::PermissionDenied);
    let binding = evaluation
        .binding("VALUE")
        .expect("unreadable import should bind unknown");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if matches!(&unknown.cause, PythonUnknownCauseView::Unreadable(error)
            if error.path == Utf8Path::new("/project/unreadable.py")
                && error.kind == io::ErrorKind::PermissionDenied)
    ));
}

#[test]
fn python_module_evaluation_reports_import_syntax_errors() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/broken.py",
        "if FLAG:\n    VALUE = 'known'\n    broken(\n",
    );
    let broken = db.file(Utf8Path::new("/project/broken.py"));
    let source = "from broken import VALUE\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    let [
        PythonImportOutcomeView::SyntaxErrors {
            origin,
            file,
            errors,
            ..
        },
    ] = evaluation.imports.as_slice()
    else {
        panic!("syntax failure should have a typed import outcome");
    };
    assert_eq!(origin.file, settings);
    assert_eq!(*file, broken);
    assert!(
        errors
            .iter()
            .any(|error| error.class == PythonSyntaxErrorClass::Ordinary)
    );
    let binding = evaluation
        .binding("VALUE")
        .expect("syntax failure should bind unknown");
    assert!(binding.alternatives.iter().any(|alternative| {
        matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: djls_project::testing::PythonValueView {
                    kind: PythonValueKindView::Unknown(unknown),
                    ..
                },
                ..
            }) if matches!(&unknown.cause, PythonUnknownCauseView::SyntaxErrors(binding_errors)
                if binding_errors == errors)
        )
    }));
}

#[test]
fn python_module_evaluation_records_mutation_origins_on_values_and_effects() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nVALUES.append('added')\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    let [mutation] = evaluation.mutations.as_slice() else {
        panic!("append should record one mutation");
    };
    assert_eq!(mutation.binding, "VALUES");
    assert_eq!(mutation.operation, PythonMutationOperationView::Append);
    assert!(mutation.path.is_empty());
    assert_eq!(
        mutation.origin,
        djls_source::Origin::new(settings, expected_span(source, "VALUES.append('added')"))
    );
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("VALUES should have one bound alternative");
    };
    assert!(
        bound
            .value
            .origins
            .iter()
            .any(|origin| origin.span == mutation.origin.span)
    );
}

#[test]
fn python_module_evaluation_records_typed_nested_mutation_facts() {
    let db = TestDatabase::new();
    let source = "TEMPLATES = [{'DIRS': []}]\nTEMPLATES[0]['DIRS'].append('/one')\nTEMPLATES[0]['DIRS'] += ['/two']\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    let [append, augmented_add] = evaluation.mutations.as_slice() else {
        panic!("the nested mutations should produce two durable facts");
    };
    let expected_path = [
        PythonMutationPathSegmentView::Index(0),
        PythonMutationPathSegmentView::Key("DIRS".to_string()),
    ];
    assert_eq!(append.binding, "TEMPLATES");
    assert_eq!(append.path, expected_path);
    assert_eq!(append.operation, PythonMutationOperationView::Append);
    assert_eq!(
        append.origin,
        djls_source::Origin::new(
            settings,
            expected_span(source, "TEMPLATES[0]['DIRS'].append('/one')"),
        )
    );
    assert_eq!(augmented_add.binding, "TEMPLATES");
    assert_eq!(augmented_add.path, expected_path);
    assert_eq!(augmented_add.operation, PythonMutationOperationView::Extend);
    assert_eq!(
        augmented_add.origin,
        djls_source::Origin::new(
            settings,
            expected_span(source, "TEMPLATES[0]['DIRS'] += ['/two']"),
        )
    );
}

#[test]
fn python_module_evaluation_discards_facts_after_unsupported_mutation() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nVALUES.append('stale')\nVALUES.clear()\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    assert!(evaluation.mutations.is_empty());
    let binding = evaluation
        .binding("VALUES")
        .expect("the invalidated binding should remain observable");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
            && unknown.origin == Some(djls_source::Origin::new(
                settings,
                expected_span(source, "VALUES.clear()"),
            ))
    ));
}

#[test]
fn python_module_evaluation_records_and_degrades_unsupported_nested_augmented_add() {
    let db = TestDatabase::new();
    let source = "TEMPLATES = [{'DIRS': []}]\nTEMPLATES[0]['DIRS'].append('/one')\nTEMPLATES[0]['DIRS'] += 'invalid'\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    let [append, augmented_add] = evaluation.mutations.as_slice() else {
        panic!("both attempted mutations should remain observable");
    };
    assert_eq!(append.operation, PythonMutationOperationView::Append);
    assert_eq!(augmented_add.operation, PythonMutationOperationView::Extend);
    assert_eq!(append.binding, "TEMPLATES");
    assert_eq!(augmented_add.binding, "TEMPLATES");

    let binding = evaluation
        .binding("TEMPLATES")
        .expect("the degraded binding should remain observable");
    assert!(binding.alternatives.iter().any(|alternative| {
        matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: djls_project::testing::PythonValueView {
                    kind: PythonValueKindView::List(_),
                    ..
                },
                ..
            })
        )
    }));
    assert!(binding.alternatives.iter().any(|alternative| {
        matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: djls_project::testing::PythonValueView {
                    kind: PythonValueKindView::Unknown(unknown),
                    ..
                },
                ..
            }) if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
                && unknown.origin == Some(djls_source::Origin::new(
                    settings,
                    expected_span(source, "TEMPLATES[0]['DIRS'] += 'invalid'"),
                ))
        )
    }));
}

#[test]
fn python_module_evaluation_keeps_mutation_from_loop_body() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nfor item in ITEMS:\n    VALUES.append(item)\n";
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, settings);

    let [mutation] = evaluation.mutations.as_slice() else {
        panic!("the loop body mutation should be retained");
    };
    assert_eq!(mutation.binding, "VALUES");
    assert_eq!(mutation.operation, PythonMutationOperationView::Append);
    assert!(mutation.path.is_empty());
    assert_eq!(
        mutation.origin,
        djls_source::Origin::new(settings, expected_span(source, "VALUES.append(item)")),
    );
}

fn cycle_products(
    query_a_first: bool,
) -> (
    djls_project::testing::PythonModuleEvaluationView,
    djls_project::testing::PythonModuleEvaluationView,
) {
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "from b import B\nA = B\nAFTER_A = 'a'\n");
    db.add_file("/project/b.py", "from a import A\nB = A\nAFTER_B = 'b'\n");
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"));
    let b = db.file(Utf8Path::new("/project/b.py"));

    if query_a_first {
        let _ = python_module_evaluation(&db, project, a);
    } else {
        let _ = python_module_evaluation(&db, project, b);
    }
    (
        python_module_evaluation(&db, project, a),
        python_module_evaluation(&db, project, b),
    )
}

#[test]
fn python_module_cycle_products_are_entry_order_independent() {
    let a_first = cycle_products(true);
    let b_first = cycle_products(false);

    assert_eq!(a_first, b_first);
    for evaluation in [&a_first.0, &a_first.1] {
        assert_eq!(
            evaluation
                .imports
                .iter()
                .filter(|outcome| matches!(outcome, PythonImportOutcomeView::Cycle { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn relative_import_cycle_uses_module_identities_in_overlapping_roots() {
    let db = TestDatabase::new();
    db.add_file("/project/lib/pkg/a.py", "from .b import B\nA = B\n");
    db.add_file("/project/lib/pkg/b.py", "from lib.pkg.a import A\nB = A\n");
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/project/lib")]);
    let module = PythonModule::resolve(&db, project, PythonModuleName::parse("pkg.a").unwrap())
        .expect("pkg.a should resolve through the nested search path");

    let evaluation = python_module_evaluation_for_module(&db, project, module);
    let cycles = evaluation
        .imports
        .iter()
        .filter_map(|outcome| match outcome {
            PythonImportOutcomeView::Cycle {
                importer_module,
                imported_module,
                ..
            } => Some((importer_module.as_str(), imported_module.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(cycles, [("lib.pkg.a", "lib.pkg.b")]);
}

#[test]
fn disjoint_import_cycles_each_keep_a_canonical_edge() {
    let db = TestDatabase::new();
    db.add_file("/project/settings.py", "from a import A\nfrom x import X\n");
    db.add_file("/project/a.py", "from b import B\nA = B\n");
    db.add_file("/project/b.py", "from a import A\nB = A\n");
    db.add_file("/project/x.py", "from y import Y\nX = Y\n");
    db.add_file("/project/y.py", "from x import X\nY = X\n");
    let project = python_project(&db);
    let settings = db.file(Utf8Path::new("/project/settings.py"));

    let evaluation = python_module_evaluation(&db, project, settings);
    let cycles = evaluation
        .imports
        .iter()
        .filter_map(|outcome| match outcome {
            PythonImportOutcomeView::Cycle {
                importer_module,
                imported_module,
                ..
            } => Some((importer_module.as_str(), imported_module.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(cycles, [("a", "b"), ("x", "y")]);
}

#[test]
fn cycle_edge_preserves_imported_module_syntax_errors() {
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "from b import B\nA = B\n");
    db.add_file("/project/b.py", "from a import A\nB = A\nbroken(\n");
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"));

    let evaluation = python_module_evaluation(&db, project, a);
    let cycle = evaluation
        .imports
        .iter()
        .find_map(|outcome| match outcome {
            PythonImportOutcomeView::Cycle {
                importer_module,
                imported_module,
                syntax_errors,
                ..
            } if importer_module.as_str() == "a" && imported_module.as_str() == "b" => {
                Some(syntax_errors)
            }
            _ => None,
        })
        .expect("the canonical a-to-b cycle edge should be retained");

    assert!(
        cycle
            .iter()
            .any(|error| error.class == PythonSyntaxErrorClass::Ordinary)
    );
}

#[test]
fn python_module_cycle_widens_cyclic_values_but_keeps_post_cycle_assignments() {
    let (a, b) = cycle_products(true);

    for (evaluation, cycle_name, stable_name, stable_value) in
        [(&a, "A", "AFTER_A", "a"), (&b, "B", "AFTER_B", "b")]
    {
        let cycle = evaluation
            .binding(cycle_name)
            .expect("cyclic value should be represented");
        assert!(matches!(
            cycle.alternatives.as_slice(),
            [PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: djls_project::testing::PythonValueView {
                    kind: PythonValueKindView::Unknown(unknown),
                    ..
                },
                ..
            })] if unknown.cause == PythonUnknownCauseView::Cycle
        ));
        let stable = evaluation
            .binding(stable_name)
            .expect("post-cycle assignment should be retained");
        assert!(matches!(
            stable.alternatives.as_slice(),
            [PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: djls_project::testing::PythonValueView {
                    kind: PythonValueKindView::Str(value),
                    ..
                },
                ..
            })] if value == stable_value
        ));
    }
}

#[test]
fn external_star_import_of_settled_cycle_attributes_dynamic_namespace_to_each_import_edge() {
    let source = "from a import *\nfrom b import *\n";
    let (db, project, settings) = extract_project(
        source,
        &[("a", "from b import *\n"), ("b", "from a import *\n")],
    );
    let settings_file = settings_module_file(&db, project).unwrap();
    let evaluation = python_module_evaluation(&db, project, settings_file);
    let expected_origins = ["from a import *", "from b import *"]
        .map(|statement| djls_source::Origin::new(settings_file, expected_span(source, statement)));
    assert_eq!(evaluation.namespace_unknowns.len(), 2, "{evaluation:#?}");
    assert!(
        evaluation
            .namespace_unknowns
            .iter()
            .all(|unknown| unknown.cause == PythonUnknownCauseView::Cycle)
    );
    assert_eq!(
        evaluation
            .namespace_unknowns
            .iter()
            .map(|unknown| unknown
                .origin
                .expect("cycle remainder should have an edge origin"))
            .collect::<Vec<_>>(),
        expected_origins
    );

    let dynamic_namespace_issues = cases(&settings, "/installed_apps/cases")
        .iter()
        .find_map(|case| case.get("dynamic"))
        .expect("the open namespace should produce a dynamic settings case")["apps"]["evidence"]
        .as_array()
        .unwrap();
    assert_eq!(dynamic_namespace_issues.len(), 2);
    for (evidence, expected_origin) in dynamic_namespace_issues.iter().zip(expected_origins) {
        assert_eq!(evidence["issue"]["kind"], "dynamic_namespace");
        assert_eq!(
            evidence["issue"]["spans"][0],
            serde_json::to_value(expected_origin.span).unwrap()
        );
    }
}

#[test]
fn external_star_import_of_settled_cycle_rebases_setting_issue_to_import_edge() {
    let source = "from a import *\n";
    let (db, project, settings) = extract_project(
        source,
        &[
            ("a", "from b import INSTALLED_APPS\n"),
            ("b", "from a import INSTALLED_APPS\n"),
        ],
    );
    let settings_file = settings_module_file(&db, project).unwrap();
    let import_origin =
        djls_source::Origin::new(settings_file, expected_span(source, "from a import *"));
    let evaluation = python_module_evaluation(&db, project, settings_file);
    let binding = evaluation
        .binding("INSTALLED_APPS")
        .expect("the star import should copy the cyclic setting binding");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                origins,
            },
            binding_origins,
        })] if unknown.cause == PythonUnknownCauseView::Cycle
            && unknown.origin == Some(import_origin)
            && origins.as_slice() == [import_origin]
            && binding_origins.as_slice() == [import_origin]
    ));

    let evidence = cases(&settings, "/installed_apps/cases")[0]["dynamic"]["apps"]["evidence"]
        .as_array()
        .expect("the cycle should produce dynamic setting evidence");
    assert_eq!(evidence.len(), 1, "{settings:#}");
    assert_eq!(evidence[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(
        evidence[0]["issue"]["spans"],
        serde_json::to_value([import_origin.span]).unwrap()
    );
}

fn external_star_cycle_product(
    query_a_first: bool,
) -> (
    djls_project::testing::PythonModuleEvaluationView,
    djls_source::File,
) {
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "from b import *\n");
    db.add_file("/project/b.py", "from a import *\n");
    db.add_file("/project/external.py", "from a import *\n");
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"));
    let b = db.file(Utf8Path::new("/project/b.py"));
    let external = db.file(Utf8Path::new("/project/external.py"));

    if query_a_first {
        let _ = python_module_evaluation(&db, project, a);
    } else {
        let _ = python_module_evaluation(&db, project, b);
    }
    (python_module_evaluation(&db, project, external), external)
}

#[test]
fn external_star_import_of_cycle_is_entry_order_independent() {
    let (a_first, a_external) = external_star_cycle_product(true);
    let (b_first, b_external) = external_star_cycle_product(false);

    assert_eq!(a_first, b_first);
    for (evaluation, external) in [(&a_first, a_external), (&b_first, b_external)] {
        assert!(matches!(
            evaluation.namespace_unknowns.as_slice(),
            [unknown]
                if unknown.cause == PythonUnknownCauseView::Cycle
                    && unknown.origin == Some(djls_source::Origin::new(
                        external,
                        expected_span("from a import *\n", "from a import *"),
                    ))
        ));
    }
}

#[test]
fn python_module_cycle_preserves_stable_side_dependencies() {
    let db = TestDatabase::new();
    db.add_file("/project/side.py", "SIDE = 'stable'\n");
    db.add_file(
        "/project/a.py",
        "from side import SIDE\nfrom b import B\nA = B\n",
    );
    db.add_file("/project/b.py", "from a import A\nB = A\n");
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"));
    let side = db.file(Utf8Path::new("/project/side.py"));
    let evaluation = python_module_evaluation(&db, project, a);

    assert!(evaluation.dependency_files.contains(&side));
    assert!(evaluation.imports.iter().any(
        |outcome| matches!(outcome, PythonImportOutcomeView::Resolved { file, .. } if *file == side)
    ));
}

#[test]
fn python_module_cycle_unions_all_feasible_mutations_independent_of_entry_order() {
    fn products(
        query_a_first: bool,
    ) -> (
        djls_project::testing::PythonModuleEvaluationView,
        djls_project::testing::PythonModuleEvaluationView,
    ) {
        let db = TestDatabase::new();
        db.add_file(
            "/project/a.py",
            "from b import *\nVALUES_A = []\nVALUES_A.append('a')\n",
        );
        db.add_file(
            "/project/b.py",
            "from a import *\nVALUES_B = []\nVALUES_B.append('b')\n",
        );
        let project = python_project(&db);
        let a = db.file(Utf8Path::new("/project/a.py"));
        let b = db.file(Utf8Path::new("/project/b.py"));
        if query_a_first {
            let _ = python_module_evaluation(&db, project, a);
        } else {
            let _ = python_module_evaluation(&db, project, b);
        }
        (
            python_module_evaluation(&db, project, a),
            python_module_evaluation(&db, project, b),
        )
    }

    let a_first = products(true);
    let b_first = products(false);
    assert_eq!(a_first, b_first);
    for evaluation in [&a_first.0, &a_first.1] {
        assert!(
            evaluation
                .mutations
                .iter()
                .any(|mutation| mutation.binding == "VALUES_A")
        );
        assert!(
            evaluation
                .mutations
                .iter()
                .any(|mutation| mutation.binding == "VALUES_B")
        );
    }
}

#[test]
fn python_module_cycle_side_dependency_change_invalidates_the_product() {
    let mut db = TestDatabase::new();
    db.add_file("/project/side.py", "SIDE = 'before'\n");
    db.add_file(
        "/project/a.py",
        "from side import SIDE\nfrom b import B\nA = B\n",
    );
    db.add_file("/project/b.py", "from a import A\nB = A\n");
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"));

    let before = python_module_evaluation(&db, project, a);
    assert!(matches!(
        before.binding("SIDE").unwrap().alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "before"
    ));

    db.add_file("/project/side.py", "SIDE = 'after'\n");
    SourceChanges::new([ChangeEvent::ContentChanged("/project/side.py".into())]).apply(&mut db);

    let after = python_module_evaluation(&db, project, a);
    assert!(matches!(
        after.binding("SIDE").unwrap().alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "after"
    ));
}

#[test]
fn python_module_long_cycle_stays_within_the_internal_iteration_guard() {
    const MEMBERS: usize = 20;

    let db = TestDatabase::new();
    for index in 0..MEMBERS {
        let next = (index + 1) % MEMBERS;
        db.add_file(
            format!("/project/member_{index}.py").as_str(),
            format!("from member_{next} import VALUE_{next}\nVALUE_{index} = '{index}'\n").as_str(),
        );
    }
    let project = python_project(&db);
    let root = db.file(Utf8Path::new("/project/member_0.py"));
    let evaluation = python_module_evaluation(&db, project, root);

    let value = evaluation
        .binding("VALUE_0")
        .expect("root assignment should be retained");
    assert!(matches!(
        value.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: djls_project::testing::PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "0"
    ));
    assert_eq!(
        evaluation
            .imports
            .iter()
            .filter(|outcome| matches!(outcome, PythonImportOutcomeView::Cycle { .. }))
            .count(),
        1
    );
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
        let settings = extract(source);
        assert!(
            cases(&settings, pointer)
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
        let settings = extract(source);
        assert!(
            cases(&settings, "/installed_apps/cases")
                .iter()
                .any(|case| case.get("dynamic").is_some()),
            "{source}"
        );
    }
}

#[test]
fn later_unconditional_exact_assignment_dominates_local_syntax_impact() {
    let settings =
        extract("INSTALLED_APPS = [\n    'stale',\n    @\n]\nINSTALLED_APPS = ['local']\n");
    let cases = cases(&settings, "/installed_apps/cases");

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
        let settings = extract(&source);
        let setting_cases = cases(&settings, "/installed_apps/cases");

        assert!(
            setting_cases
                .iter()
                .any(|case| case.to_string().contains("syntax_error")),
            "{later}: {settings:#}"
        );
    }

    let settings = extract(
        "INSTALLED_APPS = [\n    'stale',\n    @\n]\n(INSTALLED_APPS, OTHER) = (['clean'], None)\n",
    );
    let setting_cases = cases(&settings, "/installed_apps/cases");
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
        let settings = extract(&source);
        let setting_cases = cases(&settings, "/installed_apps/cases");

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
        let settings = extract(&source);
        let setting_cases = cases(&settings, "/installed_apps/cases");

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
        let settings = extract(&source);
        let setting_cases = cases(&settings, "/installed_apps/cases");

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
    );
    let setting_cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(setting_cases.len(), 1, "{settings:#}");
    assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], "clean");
}

#[test]
fn later_conditional_assignment_does_not_dominate_local_syntax_impact() {
    let settings = extract(
        "INSTALLED_APPS = [\n    'stale',\n    @\n]\nif FLAG:\n    INSTALLED_APPS = ['conditional']\n",
    );
    let cases = cases(&settings, "/installed_apps/cases");

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
    .2;
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "local");
}

#[test]
fn assignment_reading_a_name_does_not_dominate_namespace_wide_syntax_impact() {
    let settings = extract_project(
        "BASE_APPS = ['base']\nif FLAG:\n    from clean import *\n    broken(]\nINSTALLED_APPS = BASE_APPS\n",
        &[("clean", "")],
    )
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases");

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
    )
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases");

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
        .2;
        let setting_cases = cases(&settings, "/installed_apps/cases");

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
        let settings = extract_project(&source, &[("clean", "")]).2;
        let setting_cases = cases(&settings, "/installed_apps/cases");

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
    )
    .2;
    let setting_cases = cases(&settings, "/installed_apps/cases");
    assert_eq!(setting_cases.len(), 1);
    assert_eq!(setting_cases[0]["known"]["apps"][0]["value"], "selected");
}

#[test]
fn unrelated_later_syntax_error_preserves_all_exact_settings() {
    let settings = extract(
        "INSTALLED_APPS = ['blog']\nTEMPLATES = []\nSTATIC_URL = '/static/'\nSTATIC_ROOT = '/static-root'\nSTATICFILES_DIRS = ['/assets']\ndef broken(\n",
    );

    for pointer in [
        "/installed_apps/cases",
        "/templates/cases",
        "/staticfiles/static_url/cases",
        "/staticfiles/static_root/cases",
        "/staticfiles/staticfiles_dirs/cases",
    ] {
        let setting_cases = cases(&settings, pointer);
        assert_eq!(setting_cases.len(), 1, "{pointer}");
        assert!(setting_cases[0].get("known").is_some(), "{pointer}");
    }
}

#[test]
fn explicit_empty_and_unset_are_distinct() {
    let unset = extract("");
    let empty = extract("INSTALLED_APPS = []");

    assert_eq!(cases(&unset, "/installed_apps/cases"), [json!("unset")]);
    assert_eq!(
        cases(&empty, "/installed_apps/cases")[0]["known"]["apps"],
        json!([])
    );
}

#[test]
fn installed_apps_preserve_exact_branch_alternatives() {
    let settings =
        extract("if FLAG:\n    INSTALLED_APPS = ['a']\nelse:\n    INSTALLED_APPS = ['b']");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "a");
    assert_eq!(cases[1]["known"]["apps"][0]["value"], "b");
}

#[test]
fn installed_apps_dynamic_member_retains_ordered_known_fragment() {
    let settings = extract("INSTALLED_APPS = ['a', env('APP'), 'b']");
    let dynamic = &cases(&settings, "/installed_apps/cases")[0]["dynamic"];

    let evidence = &dynamic["apps"]["evidence"];
    assert_eq!(evidence[0]["known"]["value"], "a");
    assert_eq!(evidence[1]["issue"]["kind"], "unknown_element");
    assert_eq!(evidence[2]["known"]["value"], "b");
}

#[test]
fn installed_apps_exact_list_mutations_preserve_known_order() {
    let settings = extract(
        "INSTALLED_APPS = ['middle']\nINSTALLED_APPS.append('last')\nINSTALLED_APPS.extend(['later', 'removed'])\nINSTALLED_APPS.insert(100, 'bounded-last')\nINSTALLED_APPS.insert(-100, 'first')\nINSTALLED_APPS.remove('removed')",
    );
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|app| app["value"].as_str().unwrap())
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
            let cases = cases(&extract(&source), "/installed_apps/cases").to_vec();

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
    );
    let try_apps = cases(&try_settings, "/installed_apps/cases")
        .iter()
        .map(|case| case["known"]["apps"][0]["value"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(try_apps, ["except", "try"].into_iter().collect());

    let match_settings = extract(
        "match VALUE:\n    case 1:\n        INSTALLED_APPS = ['one']\n    case _:\n        INSTALLED_APPS = ['other']",
    );
    let match_apps = cases(&match_settings, "/installed_apps/cases")
        .iter()
        .map(|case| case["known"]["apps"][0]["value"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(match_apps, ["one", "other"].into_iter().collect());

    let loop_settings =
        extract("INSTALLED_APPS = ['before']\nfor app in APPS:\n    INSTALLED_APPS = [app]");
    let loop_cases = cases(&loop_settings, "/installed_apps/cases");
    assert!(loop_cases.iter().any(|case| case.get("known").is_some()));
    assert!(loop_cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn wrong_installed_apps_shape_is_malformed() {
    let settings = extract("INSTALLED_APPS = 'not-a-list'");
    assert!(
        cases(&settings, "/installed_apps/cases")[0]
            .get("malformed")
            .is_some()
    );
}

#[test]
fn templates_keep_simultaneous_backends_correlated() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/b']}]",
    );
    let known = &cases(&settings, "/templates/cases")[0]["known"];

    assert_eq!(known["backends"].as_array().unwrap().len(), 2);
    assert_eq!(known["backends"][0]["dirs"][0]["value"]["resolved"], "/a");
    assert_eq!(known["backends"][1]["dirs"][0]["value"]["resolved"], "/b");
}

#[test]
fn templates_keep_mutually_exclusive_configurations_separate() {
    let settings = extract(
        "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/b']}]",
    );
    let cases = cases(&settings, "/templates/cases");

    assert_eq!(cases.len(), 2);
    let roots: std::collections::BTreeSet<_> = cases
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"][0]["value"]["resolved"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(roots, ["/a", "/b"].into_iter().collect());
}

#[test]
fn template_field_uncertainty_does_not_erase_siblings() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'OPTIONS': {'libraries': {'good': 'app.templatetags.good'}, 'context_processors': [unknown]}}]",
    );
    let backend =
        &cases(&settings, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]["backend"];

    assert_eq!(
        backend["dirs"]["evidence"][0]["known"]["value"]["resolved"],
        "/templates"
    );
    assert_eq!(backend["libraries"]["known"][0][0], "good");
    assert_eq!(backend["dirs"]["evidence"].as_array().unwrap().len(), 1);
    assert!(
        backend["libraries"]["issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        backend["context_processors"]["issues"][0]["kind"],
        "unknown_element"
    );
}

#[test]
fn missing_template_backend_is_malformed() {
    let settings = extract("TEMPLATES = [{'DIRS': ['/templates']}]");
    assert!(
        cases(&settings, "/templates/cases")[0]
            .get("malformed")
            .is_some()
    );
}

#[test]
fn explicit_app_dirs_retains_origins_and_absence_stays_distinct() {
    let complete = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'APP_DIRS': True}]",
    );
    let app_dirs = &cases(&complete, "/templates/cases")[0]["known"]["backends"][0]["app_dirs"];
    assert_eq!(app_dirs["value"], true);
    assert_eq!(app_dirs["spans"].as_array().unwrap().len(), 1);

    let absent =
        extract("TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates'}]");
    assert!(cases(&absent, "/templates/cases")[0]["known"]["backends"][0]["app_dirs"].is_null());

    let partial = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN], 'APP_DIRS': False}]",
    );
    let app_dirs = &cases(&partial, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]["backend"]
        ["app_dirs"]["known"];
    assert_eq!(app_dirs["value"], false);
    assert_eq!(app_dirs["spans"].as_array().unwrap().len(), 1);
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
        let settings = extract(&source);
        assert!(
            cases(&settings, "/templates/cases")[0]
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
    );
    let case = &cases(&settings, "/templates/cases")[0];
    assert!(case.get("dynamic").is_some());
    assert!(case.get("malformed").is_none());
}

#[test]
fn dynamic_and_malformed_templates_preserve_backend_order_and_complete_siblings() {
    let dynamic = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/first']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN]}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/third']}]",
    );
    let evidence = cases(&dynamic, "/templates/cases")[0]["dynamic"]["templates"]["evidence"]
        .as_array()
        .unwrap();
    let backends = evidence
        .iter()
        .map(|evidence| &evidence["backend"])
        .collect::<Vec<_>>();
    assert_eq!(backends.len(), 3);
    assert_eq!(
        backends[0]["dirs"]["evidence"][0]["known"]["value"]["resolved"],
        "/first"
    );
    assert_eq!(
        backends[2]["dirs"]["evidence"][0]["known"]["value"]["resolved"],
        "/third"
    );

    let malformed = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/first']}, {'DIRS': ['/broken']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/third']}]",
    );
    let evidence = cases(&malformed, "/templates/cases")[0]["malformed"]["templates"]["evidence"]
        .as_array()
        .unwrap();
    let backends = evidence
        .iter()
        .map(|evidence| &evidence["backend"])
        .collect::<Vec<_>>();
    assert_eq!(backends.len(), 3);
    assert_eq!(
        backends[0]["dirs"]["evidence"][0]["known"]["value"]["resolved"],
        "/first"
    );
    assert_eq!(
        backends[2]["dirs"]["evidence"][0]["known"]["value"]["resolved"],
        "/third"
    );
}

#[test]
fn template_library_aliases_use_last_exact_value() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'same': 'first.tags', 'same': 'last.tags'}}}]",
    );
    let libraries = &cases(&settings, "/templates/cases")[0]["known"]["backends"][0]["libraries"];
    assert_eq!(libraries.as_array().unwrap().len(), 1);
    assert_eq!(libraries[0][0], "same");
    assert_eq!(libraries[0][1]["value"], "last.tags");
}

#[test]
fn overwritten_invalid_library_value_contributes_no_issue() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'same': False, 'same': 'last.tags'}}}]",
    );
    let backend = &cases(&settings, "/templates/cases")[0]["known"]["backends"][0];

    assert_eq!(backend["libraries"].as_array().unwrap().len(), 1);
    assert_eq!(backend["libraries"][0][1]["value"], "last.tags");
}

#[test]
fn duplicate_mapping_keys_use_last_exact_value() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': unknown, 'DIRS': ['/last']}]",
    );
    let backend = &cases(&settings, "/templates/cases")[0]["known"]["backends"][0];
    assert_eq!(backend["dirs"][0]["value"]["resolved"], "/last");
}

#[test]
fn equivalent_template_cases_merge_all_value_origins() {
    let settings = extract(
        "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates']}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates']}]",
    );
    let cases = cases(&settings, "/templates/cases");

    assert_eq!(cases.len(), 1);
    assert_eq!(
        cases[0]["known"]["backends"][0]["backend"]["spans"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        cases[0]["known"]["backends"][0]["dirs"][0]["spans"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn scalar_alternatives_preserve_equal_value_origins() {
    let settings =
        extract("if FLAG:\n    STATIC_URL = '/static/'\nelse:\n    STATIC_URL = '/static/'");
    let cases = cases(&settings, "/staticfiles/static_url/cases");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["value"], "/static/");
    assert_eq!(cases[0]["known"]["spans"].as_array().unwrap().len(), 2);
}

#[test]
fn equal_malformed_scalar_alternatives_merge_all_issue_origins() {
    let settings = extract("if FLAG:\n    STATIC_URL = False\nelse:\n    STATIC_URL = False");
    let cases = cases(&settings, "/staticfiles/static_url/cases");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["malformed"]["issues"][0]["kind"], "invalid_shape");
    assert_eq!(
        cases[0]["malformed"]["issues"][0]["spans"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn equal_top_level_unknown_alternatives_merge_all_issue_origins() {
    let source =
        "if FLAG:\n    STATIC_URL = first_dynamic()\nelse:\n    STATIC_URL = second_dynamic()";
    let settings = extract(source);
    let cases = cases(&settings, "/staticfiles/static_url/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    let issues = cases[0]["dynamic"]["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1, "{settings:#}");
    assert_eq!(issues[0]["kind"], "dynamic_expression");
    assert_eq!(
        issues[0]["spans"],
        serde_json::to_value([
            expected_span(source, "first_dynamic()"),
            expected_span(source, "second_dynamic()"),
        ])
        .unwrap()
    );
}

#[test]
fn branch_merged_unknown_appended_to_installed_apps_retains_all_origins() {
    let source = "if FLAG:\n    APP = first_dynamic()\nelse:\n    APP = second_dynamic()\nINSTALLED_APPS = []\nINSTALLED_APPS.append(APP)";
    let settings = extract(source);
    let evidence = cases(&settings, "/installed_apps/cases")[0]["dynamic"]["apps"]["evidence"]
        .as_array()
        .unwrap();

    assert_eq!(evidence.len(), 1, "{settings:#}");
    assert_eq!(evidence[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(
        evidence[0]["issue"]["spans"],
        serde_json::to_value([
            expected_span(source, "first_dynamic()"),
            expected_span(source, "second_dynamic()"),
        ])
        .unwrap()
    );
}

#[test]
fn typed_unknowns_reach_module_name_and_path_extractors_with_all_origins() {
    let source = "if FLAG:\n    VALUE = []\n    VALUE.clear()\nelse:\n    VALUE = []\n    VALUE.clear()\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': VALUE, 'OPTIONS': {'builtins': VALUE}},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': []},\n]\nTEMPLATES[1]['DIRS'].append(VALUE)";
    let settings = extract(source);
    let expected_spans = serde_json::to_value([
        expected_span(source, "VALUE.clear()"),
        Span::saturating_from_parts_usize(
            source.rfind("VALUE.clear()").unwrap(),
            "VALUE.clear()".len(),
        ),
    ])
    .unwrap();

    let backends = cases(&settings, "/templates/cases")[0]["dynamic"]["templates"]["evidence"]
        .as_array()
        .unwrap();
    let direct_dirs_issue = &backends[0]["backend"]["dirs"]["evidence"][0]["issue"];
    let builtins_issue = &backends[0]["backend"]["builtins"]["issues"][0];
    let appended_dirs_issue = &backends[1]["backend"]["dirs"]["evidence"][0]["issue"];

    for issue in [direct_dirs_issue, builtins_issue, appended_dirs_issue] {
        assert_eq!(issue["kind"], "unsupported_mutation", "{settings:#}");
        assert_eq!(issue["spans"], expected_spans, "{settings:#}");
    }
}

#[test]
fn static_root_resolves_relative_to_imported_origin_file() {
    let settings = extract_project(
        "from one.base import STATIC_ROOT",
        &[("one.base", "STATIC_ROOT = 'static'")],
    )
    .2;
    assert_eq!(
        cases(&settings, "/staticfiles/static_root/cases")[0]["known"]["value"]["resolved"],
        "/project/settings/one/static"
    );
}

#[test]
fn equal_static_roots_from_modules_in_same_directory_merge_origins() {
    let settings = extract_project(
        "if FLAG:\n    from shared.one import STATIC_ROOT\nelse:\n    from shared.two import STATIC_ROOT",
        &[
            ("shared.one", "STATIC_ROOT = 'static'"),
            ("shared.two", "STATIC_ROOT = 'static'"),
        ],
    )
    .2;
    let cases = cases(&settings, "/staticfiles/static_root/cases");

    assert_eq!(cases.len(), 1);
    assert_eq!(
        cases[0]["known"]["value"]["resolved"],
        "/project/settings/shared/static"
    );
    assert_eq!(cases[0]["known"]["spans"].as_array().unwrap().len(), 2);
}

#[test]
fn path_collections_keep_known_paths_and_uncertainty_in_source_order() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/first', *UNKNOWN, '/later']}]\nSTATICFILES_DIRS = ['/a', UNKNOWN, '/b']",
    );
    let template_evidence = &cases(&settings, "/templates/cases")[0]["dynamic"]["templates"]["evidence"]
        [0]["backend"]["dirs"]["evidence"];
    assert_eq!(template_evidence[0]["known"]["value"]["resolved"], "/first");
    assert_eq!(template_evidence[1]["issue"]["kind"], "unknown_unpack");
    assert_eq!(template_evidence[2]["known"]["value"]["resolved"], "/later");

    let static_evidence =
        &cases(&settings, "/staticfiles/staticfiles_dirs/cases")[0]["dynamic"]["paths"]["evidence"];
    assert_eq!(static_evidence[0]["known"]["value"]["resolved"], "/a");
    assert_eq!(static_evidence[1]["issue"]["kind"], "unknown_element");
    assert_eq!(static_evidence[2]["known"]["value"]["resolved"], "/b");
}

#[test]
fn exact_path_lists_preserve_duplicate_entries() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/same', '/same']} ]\nSTATICFILES_DIRS = ['/same', '/same']",
    );

    let template_dirs = &cases(&settings, "/templates/cases")[0]["known"]["backends"][0]["dirs"];
    assert_eq!(template_dirs.as_array().unwrap().len(), 2);
    assert_eq!(template_dirs[0]["value"]["resolved"], "/same");
    assert_eq!(template_dirs[1]["value"]["resolved"], "/same");

    let static_dirs = &cases(&settings, "/staticfiles/staticfiles_dirs/cases")[0]["known"]["dirs"];
    assert_eq!(static_dirs.as_array().unwrap().len(), 2);
}

#[test]
fn uncertain_star_import_preserves_known_setting_alternatives() {
    let settings = extract(
        "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nfrom missing import *",
    );
    let cases = cases(&settings, "/installed_apps/cases");
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| known["apps"][0]["value"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(known, ["first", "second"].into_iter().collect());
    assert!(cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn conditional_uncertain_star_import_preserves_known_setting_alternatives() {
    let settings = extract(
        "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nif PLUGINS:\n    from missing import *",
    );
    let cases = cases(&settings, "/installed_apps/cases");
    let known = cases
        .iter()
        .filter_map(|case| case.get("known"))
        .map(|known| known["apps"][0]["value"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(known, ["first", "second"].into_iter().collect());
    assert!(cases.iter().any(|case| case.get("dynamic").is_some()));
}

#[test]
fn exact_assignment_after_uncertain_star_import_restores_certainty() {
    let settings =
        extract("INSTALLED_APPS = ['stale']\nfrom missing import *\nINSTALLED_APPS = ['local']");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["known"]["apps"][0]["value"], "local");
}

#[test]
fn later_exact_assignment_replaces_dynamic_case() {
    let settings = extract("STATIC_URL = dynamic()\nSTATIC_URL = '/static/'");
    let cases = cases(&settings, "/staticfiles/static_url/cases");
    assert_eq!(cases.len(), 1);
    assert!(cases[0].get("known").is_some());
}

#[test]
fn straight_line_unsupported_mutations_discard_stale_settings() {
    for (source, pointer, issue_pointer) in [
        (
            "STATIC_URL = '/stale/'\nSTATIC_URL.clear()",
            "/staticfiles/static_url/cases",
            "/dynamic/issues/0",
        ),
        (
            "STATIC_ROOT = '/stale'\nSTATIC_ROOT.clear()",
            "/staticfiles/static_root/cases",
            "/dynamic/issues/0",
        ),
        (
            "INSTALLED_APPS = ['stale']\nINSTALLED_APPS.clear()",
            "/installed_apps/cases",
            "/dynamic/apps/evidence/0/issue",
        ),
        (
            "STATICFILES_DIRS = ['/stale']\nSTATICFILES_DIRS.clear()",
            "/staticfiles/staticfiles_dirs/cases",
            "/dynamic/paths/evidence/0/issue",
        ),
    ] {
        let settings = extract(source);
        let cases = cases(&settings, pointer);

        assert_eq!(cases.len(), 1, "{source}");
        assert!(cases[0].get("known").is_none(), "{source}");
        assert_eq!(
            cases[0].pointer(issue_pointer).unwrap()["kind"],
            "unsupported_mutation",
            "{source}"
        );
        assert!(!cases[0].to_string().contains("stale"), "{source}");

        let origin = binding_unknown_origin(
            source,
            source.lines().next().unwrap().split_once(' ').unwrap().0,
        );
        assert_eq!(
            &source[origin.span.start_usize()..origin.span.end_usize()],
            source.lines().nth(1).unwrap(),
            "{source}"
        );
    }
}

#[test]
fn branch_local_unsupported_mutation_preserves_unaffected_known_alternative() {
    let settings = extract("INSTALLED_APPS = ['kept']\nif FLAG:\n    INSTALLED_APPS.clear()");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 2);
    assert!(cases.iter().any(|case| {
        case["known"]["apps"][0]["value"]
            .as_str()
            .is_some_and(|app| app == "kept")
    }));
    assert!(cases.iter().any(|case| {
        case["dynamic"]["apps"]["evidence"][0]["issue"]["kind"]
            .as_str()
            .is_some_and(|kind| kind == "unsupported_mutation")
    }));
}

#[test]
fn unsupported_staticfiles_dirs_list_mutations_discard_all_path_evidence() {
    for mutation in [
        "STATICFILES_DIRS += ['/invented']",
        "STATICFILES_DIRS.append('/invented')",
        "STATICFILES_DIRS.extend(['/invented'])",
    ] {
        let source = format!("STATICFILES_DIRS = ['/known']\n{mutation}");
        let cases = cases(&extract(&source), "/staticfiles/staticfiles_dirs/cases").to_vec();

        assert_eq!(cases.len(), 1, "{mutation}");
        assert!(cases[0].get("known").is_none(), "{mutation}");
        let evidence = cases[0]["dynamic"]["paths"]["evidence"].as_array().unwrap();
        assert_eq!(evidence.len(), 1, "{mutation}");
        assert_eq!(
            evidence[0]["issue"]["kind"], "unsupported_mutation",
            "{mutation}"
        );
        assert!(!cases[0].to_string().contains("/known"), "{mutation}");
        assert!(!cases[0].to_string().contains("/invented"), "{mutation}");
    }
}

#[test]
fn installed_apps_augmented_assignment_remains_exact() {
    let settings = extract("INSTALLED_APPS = ['first']\nINSTALLED_APPS += ['second']");
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|app| app["value"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(apps, ["first", "second"]);
}

#[test]
fn dynamic_list_additions_preserve_known_installed_apps_prefix() {
    for source in [
        "INSTALLED_APPS = ['first']\nINSTALLED_APPS += EXTRA_APPS",
        "INSTALLED_APPS = ['first'] + EXTRA_APPS",
    ] {
        let settings = extract(source);
        let cases = cases(&settings, "/installed_apps/cases");

        assert_eq!(cases.len(), 1, "{source}");
        let evidence = cases[0]["dynamic"]["apps"]["evidence"].as_array().unwrap();
        assert_eq!(evidence.len(), 2, "{source}");
        assert_eq!(evidence[0]["known"]["value"], "first", "{source}");
        assert_eq!(evidence[1]["issue"]["kind"], "unknown_unpack", "{source}");
    }
}

#[test]
fn invalid_list_additions_do_not_preserve_stale_installed_apps() {
    for source in [
        "INSTALLED_APPS = ['stale']\nINSTALLED_APPS += 'invalid'",
        "INSTALLED_APPS = ['stale'] + 'invalid'",
    ] {
        let settings = extract(source);
        let cases = cases(&settings, "/installed_apps/cases");

        assert_eq!(cases.len(), 1, "{source}");
        assert!(cases[0].get("known").is_none(), "{source}");
        assert_eq!(
            cases[0]["dynamic"]["apps"]["evidence"][0]["issue"]["kind"], "dynamic_expression",
            "{source}"
        );
        assert!(!cases[0].to_string().contains("stale"), "{source}");
    }
}

#[test]
fn exact_assignment_after_unsupported_mutation_restores_known_setting() {
    for mutation in [
        "STATICFILES_DIRS += ['/discarded']",
        "STATICFILES_DIRS.append('/discarded')",
        "STATICFILES_DIRS.extend(['/discarded'])",
    ] {
        let source =
            format!("STATICFILES_DIRS = ['/stale']\n{mutation}\nSTATICFILES_DIRS = ['/restored']");
        let cases = cases(&extract(&source), "/staticfiles/staticfiles_dirs/cases").to_vec();

        assert_eq!(cases.len(), 1, "{mutation}");
        assert_eq!(
            cases[0]["known"]["dirs"][0]["value"]["resolved"], "/restored",
            "{mutation}"
        );
        assert!(cases[0].get("dynamic").is_none(), "{mutation}");
    }
}

#[test]
fn chained_immutable_assignment_remains_exact() {
    let settings = extract("STATIC_URL = URL = '/static/'");
    let cases = cases(&settings, "/staticfiles/static_url/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert_eq!(cases[0]["known"]["value"], "/static/");
}

#[test]
fn chained_immutable_tuple_assignment_remains_exact() {
    let settings = extract("INSTALLED_APPS = APPS = ('first', 'second')");
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();

    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "first");
    assert_eq!(apps[1]["value"], "second");
}

#[test]
fn chained_mutable_assignment_keeps_settings_conservative() {
    let settings = extract("INSTALLED_APPS = APPS = []\nAPPS.append('blog')");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_assignment_from_existing_name_invalidates_the_source() {
    let settings =
        extract("INSTALLED_APPS = []\nAPPS = ALIAS = INSTALLED_APPS\nALIAS.append('blog')");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_assignment_invalidates_existing_aliases() {
    let settings =
        extract("ROOT = []\nINSTALLED_APPS = ROOT\nLEFT = RIGHT = ROOT\nRIGHT.append('blog')");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_assignment_finds_aliases_after_augmented_add() {
    let settings =
        extract("ROOT = []\nINSTALLED_APPS = ROOT\nROOT += ['blog']\nLEFT = RIGHT = ROOT");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_container_assignment_invalidates_nested_mutable_sources() {
    let settings = extract(
        "DIRS = []\nWRAPPER = {'DIRS': DIRS}\nLEFT = RIGHT = WRAPPER\nRIGHT['DIRS'].append('/templates')\nSTATICFILES_DIRS = DIRS",
    );
    let cases = cases(&settings, "/staticfiles/staticfiles_dirs/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn chained_mutable_dict_assignment_keeps_templates_conservative() {
    let settings = extract(
        "BACKEND = ALIAS = {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': []}\nTEMPLATES = [BACKEND]\nALIAS['DIRS'].append('/templates')",
    );
    let cases = cases(&settings, "/templates/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn nested_template_dirs_append_and_extend_are_supported() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nTEMPLATES[0]['DIRS'].append('/b')\nTEMPLATES[0]['DIRS'].extend(['/c', '/d'])",
    );
    let dirs = cases(&settings, "/templates/cases")[0]["known"]["backends"][0]["dirs"]
        .as_array()
        .unwrap();

    assert_eq!(dirs.len(), 4);
    assert_eq!(dirs[1]["value"]["resolved"], "/b");
    assert_eq!(dirs[2]["value"]["resolved"], "/c");
    assert_eq!(dirs[3]["value"]["resolved"], "/d");
}

#[test]
fn nested_template_dirs_insert_and_remove_are_supported() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a', '/c', '/removed']}]
TEMPLATES[0]['DIRS'].insert(1, '/b')
TEMPLATES[0]['DIRS'].remove('/removed')",
    );
    let cases = cases(&settings, "/templates/cases");
    let dirs = cases[0]["known"]["backends"][0]["dirs"].as_array().unwrap();

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert_eq!(dirs.len(), 3, "{settings:#}");
    assert_eq!(dirs[0]["value"]["resolved"], "/a");
    assert_eq!(dirs[1]["value"]["resolved"], "/b");
    assert_eq!(dirs[2]["value"]["resolved"], "/c");
}

#[test]
fn nested_template_dirs_mutation_updates_all_correlated_equal_lists() {
    let settings = extract(
        "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nTEMPLATES[0]['DIRS'].append('/b')",
    );
    let cases = cases(&settings, "/templates/cases");

    assert!(!cases.is_empty(), "{settings:#}");
    for case in cases {
        let dirs = case["known"]["backends"][0]["dirs"].as_array().unwrap();
        assert_eq!(dirs.len(), 2, "{settings:#}");
        assert_eq!(dirs[0]["value"]["resolved"], "/a");
        assert_eq!(dirs[1]["value"]["resolved"], "/b");
    }
}

#[test]
fn nested_template_dirs_augmented_assignment_is_supported() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nTEMPLATES[0]['DIRS'] += ['/b']",
    );
    let cases = cases(&settings, "/templates/cases");

    assert_eq!(cases.len(), 1);
    assert_eq!(
        cases[0]["known"]["backends"][0]["dirs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        cases[0]["known"]["backends"][0]["dirs"][1]["value"]["resolved"],
        "/b"
    );
}

#[test]
fn relative_paths_from_distinct_import_origins_remain_alternatives() {
    let settings = extract_project(
        "if FLAG:\n    from one.base import STATIC_ROOT\nelse:\n    from two.base import STATIC_ROOT",
        &[
            ("one.base", "STATIC_ROOT = 'static'"),
            ("two.base", "STATIC_ROOT = 'static'"),
        ],
    )
    .2;
    let roots = cases(&settings, "/staticfiles/static_root/cases")
        .iter()
        .map(|case| case["known"]["value"]["resolved"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        roots,
        [
            "/project/settings/one/static",
            "/project/settings/two/static",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn differing_branch_scalars_distribute_through_settings_collections() {
    let settings = extract_project(
        "if FLAG:\n    from one.values import ROOT, BASE_APPS\nelse:\n    from two.values import ROOT, BASE_APPS\nINSTALLED_APPS = BASE_APPS + ['local']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT]}]\nSTATICFILES_DIRS = [ROOT]",
        &[
            ("one.values", "ROOT = '/one'\nBASE_APPS = ['one']"),
            ("two.values", "ROOT = '/two'\nBASE_APPS = ['two']"),
        ],
    )
    .2;

    let apps = cases(&settings, "/installed_apps/cases")
        .iter()
        .map(|case| {
            case["known"]["apps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|app| app["value"].as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        apps,
        [vec!["one", "local"], vec!["two", "local"]]
            .into_iter()
            .collect()
    );

    for pointer in ["/templates/cases", "/staticfiles/staticfiles_dirs/cases"] {
        let paths = cases(&settings, pointer)
            .iter()
            .map(|case| {
                let dirs = if pointer == "/templates/cases" {
                    &case["known"]["backends"][0]["dirs"]
                } else {
                    &case["known"]["dirs"]
                };
                dirs[0]["value"]["resolved"].as_str().unwrap()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(paths, ["/one", "/two"].into_iter().collect());
    }
}

#[test]
fn independent_differing_scalars_form_exact_collection_cross_products() {
    let settings = extract_project(
        "if FIRST_FLAG:\n    from one.values import FIRST\nelse:\n    from two.values import FIRST\nif SECOND_FLAG:\n    from one.values import SECOND\nelse:\n    from two.values import SECOND\nSTATICFILES_DIRS = [FIRST, SECOND]",
        &[
            ("one.values", "FIRST = '/one-first'\nSECOND = '/one-second'"),
            ("two.values", "FIRST = '/two-first'\nSECOND = '/two-second'"),
        ],
    )
    .2;
    let paths = cases(&settings, "/staticfiles/staticfiles_dirs/cases")
        .iter()
        .map(|case| {
            case["known"]["dirs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dir| dir["value"]["resolved"].as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(paths.len(), 4);
    assert!(paths.contains(&vec!["/one-first", "/two-second"]));
    assert!(paths.contains(&vec!["/two-first", "/one-second"]));
}

#[test]
fn independent_imported_scalar_paths_expand_to_a_cartesian_product() {
    let settings = extract_project(
        "if FIRST_FLAG:\n    from one.values import FIRST\nelse:\n    from two.values import FIRST\nif SECOND_FLAG:\n    from one.values import SECOND\nelse:\n    from two.values import SECOND\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [FIRST, SECOND]}]\nSTATICFILES_DIRS = [FIRST, SECOND]",
        &[
            ("one.values", "FIRST = 'first'\nSECOND = 'second'"),
            ("two.values", "FIRST = 'first'\nSECOND = 'second'"),
        ],
    )
    .2;

    let template_dirs = cases(&settings, "/templates/cases")
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dir| dir["value"]["resolved"].as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();
    let static_dirs = cases(&settings, "/staticfiles/staticfiles_dirs/cases")
        .iter()
        .map(|case| {
            case["known"]["dirs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dir| dir["value"]["resolved"].as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();
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
    assert_eq!(static_dirs, expected);
    assert_eq!(cases(&settings, "/templates/cases").len(), 4);
    assert_eq!(
        cases(&settings, "/staticfiles/staticfiles_dirs/cases").len(),
        4
    );
}

#[test]
fn repeated_branch_selected_scalar_retains_two_feasible_configurations() {
    let repeated = std::iter::repeat_n("SHARED", 7)
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        "if FLAG:\n    from one.values import SHARED\nelse:\n    from two.values import SHARED\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{repeated}]}}]\nSTATICFILES_DIRS = [{repeated}]"
    );
    let settings = extract_project(
        &source,
        &[
            ("one.values", "SHARED = 'shared'"),
            ("two.values", "SHARED = 'shared'"),
        ],
    )
    .2;

    for pointer in ["/templates/cases", "/staticfiles/staticfiles_dirs/cases"] {
        let cases = cases(&settings, pointer);
        assert_eq!(cases.len(), 2);
        assert!(cases.iter().all(|case| case.get("known").is_some()));
        let roots = cases
            .iter()
            .map(|case| {
                let paths = if pointer == "/templates/cases" {
                    case["known"]["backends"][0]["dirs"].as_array().unwrap()
                } else {
                    case["known"]["dirs"].as_array().unwrap()
                };
                assert_eq!(paths.len(), 7);
                let root = paths[0]["value"]["resolved"].as_str().unwrap();
                assert!(
                    paths
                        .iter()
                        .all(|path| path["value"]["resolved"].as_str() == Some(root))
                );
                root
            })
            .collect::<std::collections::BTreeSet<_>>();
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
}

#[test]
fn equal_top_level_setting_lists_have_an_exact_boundary_and_typed_remainder() {
    let at_limit = equal_top_level_list_alternatives(64);
    let installed_apps = cases(&at_limit, "/installed_apps/cases");
    assert_eq!(installed_apps.len(), 1, "{at_limit:#}");
    assert_eq!(installed_apps[0]["known"]["apps"][0]["value"], "shared");
    assert_eq!(
        installed_apps[0]["known"]["apps"][0]["spans"]
            .as_array()
            .unwrap()
            .len(),
        64,
        "{at_limit:#}"
    );
    let templates = cases(&at_limit, "/templates/cases");
    assert_eq!(templates.len(), 64, "{at_limit:#}");
    assert!(templates.iter().all(|case| case.get("known").is_some()));

    let overflowed = equal_top_level_list_alternatives(65);
    assert_eq!(
        overflowed,
        equal_top_level_list_alternatives(65),
        "top-level list projection must be deterministic"
    );

    let installed_apps = cases(&overflowed, "/installed_apps/cases");
    assert_eq!(installed_apps.len(), 2, "{overflowed:#}");
    assert_eq!(installed_apps[0]["known"]["apps"][0]["value"], "shared");
    let installed_remainder = installed_apps[1]["dynamic"]["apps"]["evidence"]
        .as_array()
        .unwrap();
    assert_eq!(installed_remainder.len(), 1, "{overflowed:#}");
    assert_eq!(
        installed_remainder[0]["issue"]["kind"],
        "dynamic_expression"
    );
    assert_eq!(
        installed_remainder[0]["issue"]["spans"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{overflowed:#}"
    );

    let templates = cases(&overflowed, "/templates/cases");
    assert_eq!(templates.len(), 65, "{overflowed:#}");
    assert!(
        templates[..64]
            .iter()
            .all(|case| case.get("known").is_some()),
        "{overflowed:#}"
    );
    let template_remainder = templates[64]["dynamic"]["templates"]["evidence"]
        .as_array()
        .unwrap();
    assert_eq!(template_remainder.len(), 1, "{overflowed:#}");
    assert_eq!(template_remainder[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(
        template_remainder[0]["issue"]["spans"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{overflowed:#}"
    );
}

#[test]
fn path_list_caps_the_union_of_list_variants_at_64_plus_one_remainder() {
    let evaluate = |count: usize| {
        let mut source = String::new();
        let mut modules = Vec::new();
        for index in 0..count {
            if index == 0 {
                source.push_str("if FLAG_0:\n");
            } else if index + 1 == count {
                source.push_str("else:\n");
            } else {
                source.push_str(format!("elif FLAG_{index}:\n").as_str());
            }
            source.push_str(
                format!("    from variant_{index:02} import STATICFILES_DIRS\n").as_str(),
            );
            modules.push((
                format!("variant_{index:02}"),
                "STATICFILES_DIRS = ['shared']",
            ));
        }
        let module_refs = modules
            .iter()
            .map(|(name, body)| (name.as_str(), *body))
            .collect::<Vec<_>>();
        extract_project(&source, &module_refs).2
    };

    let at_limit = evaluate(64);
    let at_limit = cases(&at_limit, "/staticfiles/staticfiles_dirs/cases");
    assert_eq!(at_limit.len(), 64);
    assert!(at_limit.iter().all(|case| case.get("known").is_some()));

    let overflowed = evaluate(65);
    let overflowed = cases(&overflowed, "/staticfiles/staticfiles_dirs/cases");
    assert_eq!(overflowed.len(), 65);
    assert!(
        overflowed[..64]
            .iter()
            .all(|case| case.get("known").is_some())
    );
    let remainder = &overflowed[64]["dynamic"]["paths"]["evidence"];
    assert_eq!(remainder.as_array().unwrap().len(), 1);
    assert_eq!(remainder[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(remainder[0]["issue"]["spans"].as_array().unwrap().len(), 1);
}

#[test]
fn capped_dynamic_remainder_merges_later_syntax_evidence() {
    let mut source = String::from("if BROKEN:\n    STATICFILES_DIRS = ['stale']\n    broken(]\n");
    let mut modules = Vec::new();
    for index in 0..65 {
        if index == 0 {
            source.push_str("if FLAG_0:\n");
        } else if index == 64 {
            source.push_str("else:\n");
        } else {
            source.push_str(format!("elif FLAG_{index}:\n").as_str());
        }
        source.push_str(format!("    from variant_{index:02} import STATICFILES_DIRS\n").as_str());
        modules.push((
            format!("variant_{index:02}"),
            format!("STATICFILES_DIRS = ['/path-{index:02}']"),
        ));
    }
    let module_refs = modules
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect::<Vec<_>>();

    let settings = extract_project(&source, &module_refs).2;
    let setting_cases = cases(&settings, "/staticfiles/staticfiles_dirs/cases");
    assert_eq!(setting_cases.len(), 65, "{settings:#}");
    let evidence = setting_cases
        .iter()
        .find_map(|case| case.get("dynamic"))
        .expect("overflow should retain a dynamic remainder")["paths"]["evidence"]
        .as_array()
        .unwrap();
    let issues = evidence
        .iter()
        .map(|evidence| &evidence["issue"])
        .collect::<Vec<_>>();

    assert_eq!(issues.len(), 2, "{settings:#}");
    assert!(issues.iter().any(|issue| {
        issue["kind"] == "dynamic_expression"
            && issue["spans"]
                .as_array()
                .is_some_and(|spans| spans.len() == 1)
    }));
    assert!(
        issues.iter().any(|issue| {
            issue["kind"] == "syntax_error"
                && issue["spans"]
                    .as_array()
                    .is_some_and(|spans| !spans.is_empty())
        }),
        "{issues:#?}"
    );
}

#[test]
fn two_backends_sharing_a_branch_path_keep_only_feasible_configurations() {
    let settings = extract_project(
        "if FLAG:\n    from one.values import ROOT\nelse:\n    from two.values import ROOT\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT]},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT]},\n]",
        &[
            ("one.values", "ROOT = 'templates'"),
            ("two.values", "ROOT = 'templates'"),
        ],
    )
    .2;

    let configurations = cases(&settings, "/templates/cases");
    assert_eq!(configurations.len(), 2, "{settings:#}");
    let roots = configurations
        .iter()
        .map(|case| {
            let backends = case["known"]["backends"].as_array().unwrap();
            let first = backends[0]["dirs"][0]["value"]["resolved"]
                .as_str()
                .unwrap();
            let second = backends[1]["dirs"][0]["value"]["resolved"]
                .as_str()
                .unwrap();
            assert_eq!(first, second, "both backends must select the same branch");
            first
        })
        .collect::<std::collections::BTreeSet<_>>();
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("the two feasible settings branches have different winners");
    };
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<std::collections::BTreeSet<_>>();

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
            .unwrap();
        }
        let first = names[..3].join(", ");
        let second = names[3..].join(", ");
        let source = format!(
            "{branches}TEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{first}]}}, {{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{second}]}}]"
        );
        let mut module = String::new();
        for name in &names {
            writeln!(module, "{name} = 'shared'").unwrap();
        }
        extract_project(
            &source,
            &[
                ("one.values", module.as_str()),
                ("two.values", module.as_str()),
            ],
        )
        .2
    };

    let at_limit = evaluate(3);
    let at_limit_cases = cases(&at_limit, "/templates/cases");
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
    let overflowed = cases(&overflowed, "/templates/cases");
    assert_eq!(overflowed.len(), 65);
    assert!(overflowed[..64].iter().all(|case| {
        case["known"]["backends"]
            .as_array()
            .is_some_and(|backends| backends.len() == 2)
    }));
    let remainder = &overflowed[64]["dynamic"]["templates"]["evidence"];
    assert_eq!(remainder.as_array().unwrap().len(), 1);
    assert_eq!(remainder[0]["issue"]["kind"], "dynamic_expression");
    assert_eq!(remainder[0]["issue"]["spans"].as_array().unwrap().len(), 1);
}

#[test]
fn equal_relative_path_lists_from_distinct_modules_remain_correlated_alternatives() {
    let settings = extract_project(
        "if FLAG:\n    from one.base import TEMPLATES, STATICFILES_DIRS\nelse:\n    from two.base import TEMPLATES, STATICFILES_DIRS",
        &[
            (
                "one.base",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['first', 'second']}]\nSTATICFILES_DIRS = ['first', 'second']",
            ),
            (
                "two.base",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['first', 'second']}]\nSTATICFILES_DIRS = ['first', 'second']",
            ),
        ],
    )
    .2;

    let template_dirs = cases(&settings, "/templates/cases")
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dir| dir["value"]["resolved"].as_str().unwrap())
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

    let static_dirs = cases(&settings, "/staticfiles/staticfiles_dirs/cases")
        .iter()
        .map(|case| {
            case["known"]["dirs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dir| dir["value"]["resolved"].as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(static_dirs, template_dirs);
}

#[test]
fn equal_mixed_origin_path_lists_retain_each_original_configuration() {
    let settings = extract_project(
        "if FLAG:\n    from one.base import TEMPLATES, STATICFILES_DIRS\nelse:\n    from two.base import TEMPLATES, STATICFILES_DIRS",
        &[
            ("one.values", "FIRST = 'first'\nSECOND = 'second'"),
            ("two.values", "FIRST = 'first'\nSECOND = 'second'"),
            (
                "one.base",
                "from one.values import FIRST\nfrom two.values import SECOND\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [FIRST, SECOND]}]\nSTATICFILES_DIRS = [FIRST, SECOND]",
            ),
            (
                "two.base",
                "from two.values import FIRST\nfrom one.values import SECOND\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [FIRST, SECOND]}]\nSTATICFILES_DIRS = [FIRST, SECOND]",
            ),
        ],
    )
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
    .collect::<std::collections::BTreeSet<_>>();
    let template_dirs = cases(&settings, "/templates/cases")
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dir| dir["value"]["resolved"].as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();
    let static_dirs = cases(&settings, "/staticfiles/staticfiles_dirs/cases")
        .iter()
        .map(|case| {
            case["known"]["dirs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dir| dir["value"]["resolved"].as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(template_dirs, expected);
    assert_eq!(static_dirs, expected);
}

#[test]
fn all_setting_families_distinguish_known_unset_dynamic_and_malformed() {
    let known = extract(
        "INSTALLED_APPS = []\nTEMPLATES = []\nSTATIC_URL = ''\nSTATIC_ROOT = ''\nSTATICFILES_DIRS = []",
    );
    let unset = extract("");
    let dynamic = extract(
        "INSTALLED_APPS = unknown\nTEMPLATES = unknown\nSTATIC_URL = unknown\nSTATIC_ROOT = unknown\nSTATICFILES_DIRS = unknown",
    );
    let malformed = extract(
        "INSTALLED_APPS = False\nTEMPLATES = False\nSTATIC_URL = False\nSTATIC_ROOT = False\nSTATICFILES_DIRS = False",
    );
    for pointer in [
        "/installed_apps/cases",
        "/templates/cases",
        "/staticfiles/static_url/cases",
        "/staticfiles/static_root/cases",
        "/staticfiles/staticfiles_dirs/cases",
    ] {
        assert!(
            cases(&known, pointer)[0].get("known").is_some(),
            "{pointer}"
        );
        assert_eq!(cases(&unset, pointer), [json!("unset")], "{pointer}");
        assert!(
            cases(&dynamic, pointer)[0].get("dynamic").is_some(),
            "{pointer}"
        );
        assert!(
            cases(&malformed, pointer)[0].get("malformed").is_some(),
            "{pointer}"
        );
    }
}

#[test]
fn unknown_backend_key_weakens_prior_claim_and_later_exact_key_is_authoritative() {
    let weakened =
        extract("TEMPLATES = [{'BACKEND': 'before.backend', unknown_key: 'maybe.backend'}]");
    let backend =
        &cases(&weakened, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]["backend"];
    assert_eq!(backend["backend"]["known"]["value"], "before.backend");
    assert_eq!(
        backend["backend"]["issues"][0]["kind"],
        "dynamic_expression"
    );

    let restored = extract(
        "TEMPLATES = [{'BACKEND': 'before.backend', unknown_key: 'maybe.backend', 'BACKEND': 'after.backend'}]",
    );
    let backend =
        &cases(&restored, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]["backend"];
    assert_eq!(backend["backend"]["known"]["value"], "after.backend");
    assert!(backend["backend"]["issues"].as_array().unwrap().is_empty());
}

#[test]
fn unknown_library_key_weakens_prior_alias_and_later_exact_key_is_authoritative() {
    let weakened = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'alias': 'before.tags', unknown_key: 'maybe.tags'}}}]",
    );
    let libraries = &cases(&weakened, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]
        ["backend"]["libraries"];
    assert!(libraries["known"].as_array().unwrap().is_empty());
    assert_eq!(libraries["issues"][0]["kind"], "dynamic_expression");

    let restored = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'alias': 'before.tags', unknown_key: 'maybe.tags', 'alias': 'after.tags'}}}]",
    );
    let libraries = &cases(&restored, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]
        ["backend"]["libraries"];
    assert_eq!(libraries["known"].as_array().unwrap().len(), 1);
    assert_eq!(libraries["known"][0][0], "alias");
    assert_eq!(libraries["known"][0][1]["value"], "after.tags");
    assert_eq!(libraries["issues"][0]["kind"], "dynamic_expression");
}

#[test]
fn unknown_library_unpack_removes_prior_authority_but_later_entry_wins() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'before': 'before.tags', **unknown, 'after': 'after.tags'}}}]",
    );
    let libraries = &cases(&settings, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]
        ["backend"]["libraries"];

    assert_eq!(libraries["known"].as_array().unwrap().len(), 1);
    assert_eq!(libraries["known"][0][0], "after");
    assert_eq!(libraries["issues"][0]["kind"], "unknown_unpack");
}

#[test]
fn imports_feed_values_and_semantic_dependencies() {
    let (db, project, settings) = extract_project(
        "from base import INSTALLED_APPS",
        &[("base", "INSTALLED_APPS = ['base']")],
    );
    assert_eq!(
        cases(&settings, "/installed_apps/cases")[0]["known"]["apps"][0]["value"],
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
    );
    assert_eq!(
        cases(&settings, "/installed_apps/cases")[0]["known"]["apps"][0]["value"],
        "local"
    );
    assert_eq!(
        compute_project_facts(&db, project).file_paths(),
        [Utf8PathBuf::from("/project/settings/config/settings.py")]
    );
}
