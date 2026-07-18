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
use djls_project::testing::PythonModuleEvaluationView;
use djls_project::testing::PythonMutationOperationView;
use djls_project::testing::PythonMutationPathSegmentView;
use djls_project::testing::PythonSequenceItemView;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::PythonUnknownCauseView;
use djls_project::testing::PythonValueKindView;
use djls_project::testing::PythonValueView;
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
    unknown
        .origins
        .first()
        .copied()
        .expect("unknown should retain an origin")
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

fn equal_top_level_list_alternatives(count: usize, reverse_module_installation: bool) -> Value {
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
    if reverse_module_installation {
        modules.reverse();
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
    let [PythonSequenceItemView::UnknownElement(unknown)] = items.as_slice() else {
        panic!("the list should contain one typed unknown element");
    };
    assert_eq!(unknown.cause, PythonUnknownCauseView::UnsupportedExpression);
    assert_eq!(
        unknown
            .origins
            .first()
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
fn python_module_package_identity_relative_import_from_init_alias_uses_parent_package() {
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
fn missing_resolved_named_import_member_is_typed_dynamic_and_replaces_stale_binding() {
    let source = "INSTALLED_APPS = ['stale']\nfrom plugin import MISSING as INSTALLED_APPS\n";
    let (db, project, settings) = extract_project(source, &[("plugin", "PRESENT = 'known'\n")]);
    let settings_file = settings_module_file(&db, project).unwrap();
    let plugin_file = db.file(Utf8Path::new("/project/settings/plugin.py"));
    let evaluation = python_module_evaluation(&db, project, settings_file);

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::Resolved {
            file,
            importer_module,
            imported_module,
            ..
        }] if *file == plugin_file
            && importer_module.as_str() == "config.settings"
            && imported_module.as_str() == "plugin"
    ));
    assert_eq!(evaluation.dependency_files, [settings_file, plugin_file]);

    let binding = evaluation
        .binding("INSTALLED_APPS")
        .expect("the named import should replace the stale local binding");
    let import_origin = djls_source::Origin::new(
        settings_file,
        expected_span(source, "MISSING as INSTALLED_APPS"),
    );
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                origins,
            },
            binding_origins,
        })] if matches!(
            &unknown.cause,
            PythonUnknownCauseView::MissingImportMember { module, member }
                if module.as_str() == "plugin" && member == "MISSING"
        ) && unknown.origins.as_slice() == [import_origin]
            && origins.as_slice() == [import_origin]
            && binding_origins.as_slice() == [import_origin]
    ));

    let setting_cases = cases(&settings, "/installed_apps/cases");
    assert_eq!(setting_cases.len(), 1, "{settings:#}");
    assert!(setting_cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(setting_cases.iter().all(|case| case != "unset"));
    assert_eq!(
        setting_cases[0]["dynamic"]["apps"]["evidence"][0]["issue"]["kind"],
        "dynamic_expression",
    );
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
fn named_import_of_absent_open_name_preserves_member_and_namespace_uncertainty() {
    let source = "from plugin import INSTALLED_APPS\n";
    let (db, project, settings) = extract_project(
        source,
        &[("plugin", "if ENABLED:\n    from missing import *\n")],
    );
    let settings_file = settings_module_file(&db, project).unwrap();
    let plugin_file = db.file(Utf8Path::new("/project/settings/plugin.py"));
    let evaluation = python_module_evaluation(&db, project, settings_file);
    let binding = evaluation
        .binding("INSTALLED_APPS")
        .expect("the named import should retain member and namespace uncertainty");

    assert!(
        !binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
    let import_origin =
        djls_source::Origin::new(settings_file, expected_span(source, "INSTALLED_APPS"));
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            binding_origins,
        }) if matches!(
            &unknown.cause,
            PythonUnknownCauseView::MissingImportMember { module, member }
                if module.as_str() == "plugin" && member == "INSTALLED_APPS"
        ) && unknown.origins.as_slice() == [import_origin]
            && binding_origins.as_slice() == [import_origin]
    )));
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            binding_origins,
        }) if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing")
            && unknown.origins.as_slice() == [import_origin]
            && binding_origins.as_slice() == [import_origin]
    )));
    assert_eq!(
        evaluation
            .imports
            .iter()
            .filter(|outcome| matches!(outcome, PythonImportOutcomeView::Resolved { file, .. } if *file == plugin_file))
            .count(),
        1,
    );
    assert_eq!(
        evaluation
            .dependency_files
            .iter()
            .filter(|file| **file == plugin_file)
            .count(),
        1,
    );

    let setting_cases = cases(&settings, "/installed_apps/cases");
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
    let (db, project, settings) = extract_project(
        source,
        &[(
            "plugin",
            "if ENABLED:\n    INSTALLED_APPS = ['imported']\n    from missing import *\n",
        )],
    );
    let settings_file = settings_module_file(&db, project).unwrap();
    let plugin_file = db.file(Utf8Path::new("/project/settings/plugin.py"));
    let evaluation = python_module_evaluation(&db, project, settings_file);
    let binding = evaluation
        .binding("INSTALLED_APPS")
        .expect("the conditional named import should bind all feasible alternatives");

    assert!(
        !binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::List(items),
                ..
            },
            ..
        }) if matches!(items.as_slice(), [PythonSequenceItemView::Value(PythonValueView {
            kind: PythonValueKindView::Str(value),
            ..
        })] if value == "imported")
    )));
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(
            &unknown.cause,
            PythonUnknownCauseView::MissingImportMember { module, member }
                if module.as_str() == "plugin" && member == "INSTALLED_APPS"
        )
    )));
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing")
    )));
    assert_eq!(
        evaluation
            .imports
            .iter()
            .filter(|outcome| matches!(outcome, PythonImportOutcomeView::Resolved { file, .. } if *file == plugin_file))
            .count(),
        1,
    );
    assert_eq!(
        evaluation
            .dependency_files
            .iter()
            .filter(|file| **file == plugin_file)
            .count(),
        1,
    );

    let setting_cases = cases(&settings, "/installed_apps/cases");
    assert!(
        setting_cases.iter().all(|case| case != "unset"),
        "{settings:#}"
    );
    assert!(
        setting_cases
            .iter()
            .any(|case| case.get("dynamic").is_some())
    );
    assert!(setting_cases.iter().any(|case| {
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
        unknown.origins.as_slice()
            == [djls_source::Origin::new(
                settings_file,
                expected_span(source, "from plugin import *"),
            )]
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
    let source = "INSTALLED_APPS = []\nwhile FLAG:\n    INSTALLED_APPS.append('loop')\nelse:\n    from plugin import VALUE\n";
    let (db, project, settings) = extract_project(source, &[("plugin", "VALUE = '/static/'")]);
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

fn evaluate_module(source: &str) -> (djls_source::File, PythonModuleEvaluationView) {
    let db = TestDatabase::new();
    db.add_file("/project/settings.py", source);
    let project = python_project(&db);
    let file = db.file(Utf8Path::new("/project/settings.py"));
    let evaluation = python_module_evaluation(&db, project, file);
    (file, evaluation)
}

fn bound_value<'a>(
    evaluation: &'a PythonModuleEvaluationView,
    name: &str,
) -> &'a PythonBoundValueView {
    let binding = evaluation
        .binding(name)
        .unwrap_or_else(|| panic!("{name} should be bound"));
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        panic!("{name} should have exactly one bound alternative");
    };
    bound
}

fn has_unknown_alternative(evaluation: &PythonModuleEvaluationView, name: &str) -> bool {
    evaluation.binding(name).is_some_and(|binding| {
        binding.alternatives.iter().any(|alternative| {
            matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(_),
                        ..
                    },
                    ..
                })
            )
        })
    })
}

fn string_list_items(value: &PythonValueView) -> Vec<String> {
    let (PythonValueKindView::List(items) | PythonValueKindView::Tuple(items)) = &value.kind else {
        panic!("expected a sequence value");
    };
    items
        .iter()
        .map(|item| match item {
            PythonSequenceItemView::Value(PythonValueView {
                kind: PythonValueKindView::Str(text),
                ..
            }) => format!("str:{text}"),
            PythonSequenceItemView::Value(_) => "value".to_string(),
            PythonSequenceItemView::UnknownElement(_) => "element".to_string(),
            PythonSequenceItemView::UnknownUnpack(_) => "unpack".to_string(),
        })
        .collect()
}

fn nested_list_at_index0(value: &PythonValueView) -> &PythonValueView {
    let PythonValueKindView::Tuple(items) = &value.kind else {
        panic!("expected a tuple value");
    };
    let [PythonSequenceItemView::Value(nested)] = items.as_slice() else {
        panic!("expected one nested value in the tuple");
    };
    nested
}

#[test]
fn tuple_index_augmented_add_mutates_nested_list_transactionally() {
    // ROOT is an immutable tuple, but tuple indexing reaches the nested mutable
    // list, so `ROOT[0] += [...]` mutates that list in place while the tuple
    // structure is preserved.
    let source = "ROOT = ([],)\nROOT[0] += ['a']\n";
    let (file, evaluation) = evaluate_module(source);
    let bound = bound_value(&evaluation, "ROOT");
    let nested = nested_list_at_index0(&bound.value);

    assert_eq!(string_list_items(nested), vec!["str:a"]);
    // Ancestor provenance: the operation origin is recorded up the path onto
    // the root tuple value.
    let op_origin = djls_source::Origin::new(file, expected_span(source, "ROOT[0] += ['a']"));
    assert!(
        bound.value.origins.contains(&op_origin),
        "the augmented-add origin should be recorded on the tuple ancestor",
    );
    // A single `Extend` mutation fact rooted at ROOT with an Index(0) path.
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation]
            if mutation.binding == "ROOT"
                && mutation.operation == PythonMutationOperationView::Extend
                && mutation.path == vec![PythonMutationPathSegmentView::Index(0)]
    ));
}

#[test]
fn tuple_index_extend_mutates_nested_list_and_invalidates_stale_alias() {
    // INNER and ROOT[0] name the same nested list allocation. Extending through
    // the tuple index mutates it in place and conservatively invalidates the
    // stale `INNER` alias.
    let source = "INNER = []\nROOT = (INNER,)\nROOT[0].extend(['a'])\n";
    let (_file, evaluation) = evaluate_module(source);
    let bound = bound_value(&evaluation, "ROOT");
    let nested = nested_list_at_index0(&bound.value);

    assert_eq!(string_list_items(nested), vec!["str:a"]);
    assert!(
        has_unknown_alternative(&evaluation, "INNER"),
        "the stale alias should be conservatively invalidated",
    );
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation]
            if mutation.binding == "ROOT"
                && mutation.operation == PythonMutationOperationView::Extend
                && mutation.path == vec![PythonMutationPathSegmentView::Index(0)]
    ));
}

#[test]
fn tuple_index_augmented_add_bool_failure_degrades_root_and_alias() {
    // A definitely non-iterable source through a nested list fails: the root
    // and its reachable aliases degrade, RHS bindings are untouched, and the
    // attempted `Extend` fact is still recorded.
    let source = "INNER = []\nROOT = (INNER,)\nROOT[0] += True\n";
    let (_file, evaluation) = evaluate_module(source);

    assert!(
        has_unknown_alternative(&evaluation, "ROOT"),
        "a failed nested mutation degrades the root binding",
    );
    assert!(
        has_unknown_alternative(&evaluation, "INNER"),
        "a failed nested mutation degrades reachable aliases",
    );
    assert!(
        evaluation.mutations.iter().any(|mutation| {
            mutation.binding == "ROOT"
                && mutation.operation == PythonMutationOperationView::Extend
                && mutation.path == vec![PythonMutationPathSegmentView::Index(0)]
        }),
        "the attempted mutation fact is recorded even on failure",
    );
}

#[test]
fn nested_augmented_add_failure_joins_prior_values_with_mutation_unknowns() {
    let source = "INNER = []\nROOT = (INNER,)\nROOT[0] += True\n";
    let (_file, evaluation) = evaluate_module(source);

    let root = evaluation.binding("ROOT").expect("ROOT should be bound");
    assert_eq!(root.alternatives.len(), 2);
    assert!(root.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Tuple(_),
                ..
            },
            ..
        })
    )));
    assert!(root.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
    )));

    let inner = evaluation.binding("INNER").expect("INNER should be bound");
    assert_eq!(inner.alternatives.len(), 2);
    assert!(inner.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::List(_),
                ..
            },
            ..
        })
    )));
    assert!(inner.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
    )));
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation]
            if mutation.binding == "ROOT"
                && mutation.operation == PythonMutationOperationView::Extend
    ));
}

#[test]
fn repeated_tuple_alias_invalidation_retains_augmented_add_fact() {
    let source = "INNER = []\nROOT = (INNER, INNER)\nROOT[0] += ['x']\n";
    let (_file, evaluation) = evaluate_module(source);

    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation]
            if mutation.binding == "ROOT"
                && mutation.operation == PythonMutationOperationView::Extend
                && mutation.path == vec![PythonMutationPathSegmentView::Index(0)]
    ));
    let root = evaluation.binding("ROOT").expect("ROOT should be bound");
    assert!(matches!(
        root.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::UnsupportedExpression
    ));
}

#[test]
fn name_target_list_augmented_add_preserves_prior_append_fact() {
    // A successful name-target list `+=` updates the binding in place without
    // clearing prior mutation facts, so the `Append` and `Extend` facts remain
    // in order.
    let source = "VALUES = []\nVALUES.append('a')\nVALUES += ['b']\n";
    let (file, evaluation) = evaluate_module(source);
    let bound = bound_value(&evaluation, "VALUES");

    assert_eq!(string_list_items(&bound.value), vec!["str:a", "str:b"]);
    assert_eq!(
        bound.binding_origins,
        vec![djls_source::Origin::new(file, expected_span(source, "[]"))],
        "in-place `+=` preserves the original assignment origin",
    );
    let operations = evaluation
        .mutations
        .iter()
        .filter(|mutation| mutation.binding == "VALUES")
        .map(|mutation| mutation.operation.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        vec![
            PythonMutationOperationView::Append,
            PythonMutationOperationView::Extend,
        ],
        "a successful in-place `+=` keeps the prior append fact and appends extend",
    );
}

#[test]
fn name_target_list_augmented_add_failure_rebinds_and_clears_prior_facts() {
    // A failed name-target list `+=` rebinds the target to an unknown, which is
    // permitted to clear the prior mutation history; only the attempted
    // `Extend` fact remains.
    let source = "VALUES = []\nVALUES.append('a')\nVALUES += True\n";
    let (_file, evaluation) = evaluate_module(source);

    assert!(has_unknown_alternative(&evaluation, "VALUES"));
    let operations = evaluation
        .mutations
        .iter()
        .filter(|mutation| mutation.binding == "VALUES")
        .map(|mutation| mutation.operation.clone())
        .collect::<Vec<_>>();
    assert_eq!(operations, vec![PythonMutationOperationView::Extend]);
}

#[test]
fn augmented_add_and_extend_bool_failures_differ_on_mutation_facts() {
    // Name-target `+= bool` records an attempted `Extend` fact; a `.extend()`
    // call whose RHS is definitely non-iterable records no fact. Both degrade
    // the receiver.
    let (_file, augmented) = evaluate_module("VALUES = []\nVALUES += True\n");
    assert!(has_unknown_alternative(&augmented, "VALUES"));
    assert!(
        augmented
            .mutations
            .iter()
            .any(|mutation| mutation.binding == "VALUES"
                && mutation.operation == PythonMutationOperationView::Extend),
        "name-target `+= bool` records an attempted extend fact",
    );

    let (_file, extended) = evaluate_module("FLAG = True\nVALUES = []\nVALUES.extend(FLAG)\n");
    assert!(has_unknown_alternative(&extended, "VALUES"));
    assert!(
        extended.mutations.is_empty(),
        "a non-iterable `.extend()` call records no mutation fact",
    );
    assert!(
        has_unknown_alternative(&extended, "FLAG"),
        "the named RHS is degraded by the failed call",
    );
}

#[test]
fn failed_extend_replaces_root_but_only_joins_rhs_degradation() {
    let source = "FLAG = True\nVALUES = []\nVALUES.extend(FLAG)\n";
    let (_file, evaluation) = evaluate_module(source);

    let values = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    assert!(matches!(
        values.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
    ));

    let flag = evaluation.binding("FLAG").expect("FLAG should be bound");
    assert_eq!(flag.alternatives.len(), 2);
    assert!(flag.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Bool(true),
                ..
            },
            ..
        })
    )));
    assert!(flag.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
    )));
    assert!(evaluation.mutations.is_empty());
}

#[test]
fn failed_self_aliasing_name_augmented_add_keeps_direct_target_cause() {
    let source = "VALUES = []\nVALUES.append(VALUES)\nVALUES += True\n";
    let (_file, evaluation) = evaluate_module(source);

    let values = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    assert!(matches!(
        values.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::UnsupportedExpression
    ));
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation] if mutation.operation == PythonMutationOperationView::Extend
    ));
}

#[test]
fn failed_name_augmented_add_preserves_rhs_only_binding_exactly() {
    let source = "FLAG = True\nVALUES = []\nVALUES += FLAG\n";
    let (_file, evaluation) = evaluate_module(source);

    let values = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    assert!(matches!(
        values.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::UnsupportedExpression
    ));
    let flag = bound_value(&evaluation, "FLAG");
    assert!(matches!(flag.value.kind, PythonValueKindView::Bool(true)));
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation] if mutation.operation == PythonMutationOperationView::Extend
    ));
}

#[test]
fn successful_augmented_add_preserves_rhs_only_binding() {
    // A named RHS that is not itself an alias of the target is preserved as-is
    // by a successful `+=`.
    let source = "SRC = ['x']\nVALUES = []\nVALUES += SRC\n";
    let (_file, evaluation) = evaluate_module(source);

    let src = bound_value(&evaluation, "SRC");
    assert_eq!(string_list_items(&src.value), vec!["str:x"]);
    let values = bound_value(&evaluation, "VALUES");
    assert_eq!(string_list_items(&values.value), vec!["str:x"]);
}

#[test]
fn failed_extend_alias_rhs_degradation_takes_precedence_over_preservation() {
    // The RHS `ALIAS` aliases the failing dictionary receiver `ROOT`. Because
    // it is selected by the mutation-failure alias set, its degradation takes
    // precedence over RHS-only preservation.
    let source = "ROOT = {}\nALIAS = ROOT\nROOT.extend(ALIAS)\n";
    let (_file, evaluation) = evaluate_module(source);

    for name in ["ROOT", "ALIAS"] {
        let binding = evaluation
            .binding(name)
            .unwrap_or_else(|| panic!("{name} should be bound"));
        assert!(matches!(
            binding.alternatives.as_slice(),
            [PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::Unknown(unknown),
                    ..
                },
                ..
            })] if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
        ));
    }
    assert!(
        evaluation.mutations.is_empty(),
        "a failed non-list `.extend()` records no mutation fact",
    );
}

#[test]
fn name_target_augmented_add_unsupported_receiver_categories_fallback() {
    // Every non-list/tuple/string receiver category rebinds the target to an
    // unknown and records the attempted `Extend` fact.
    for receiver in ["{}", "Path('/x')", "True", "MISSING"] {
        let source = format!("from pathlib import Path\nTARGET = {receiver}\nTARGET += ['a']\n");
        let (_file, evaluation) = evaluate_module(&source);
        assert!(
            has_unknown_alternative(&evaluation, "TARGET"),
            "receiver `{receiver}` should degrade the target",
        );
        assert!(
            evaluation
                .mutations
                .iter()
                .any(|mutation| mutation.binding == "TARGET"
                    && mutation.operation == PythonMutationOperationView::Extend),
            "receiver `{receiver}` should record the attempted extend fact",
        );
    }

    // A dict target with a mutable alias degrades that alias too.
    let source = "D = {}\nALIAS = D\nD += ['a']\n";
    let (_file, evaluation) = evaluate_module(source);
    assert!(has_unknown_alternative(&evaluation, "D"));
    assert!(
        has_unknown_alternative(&evaluation, "ALIAS"),
        "the dict alias should be degraded by the unsupported `+=`",
    );
}

#[test]
fn nested_augmented_add_non_list_target_categories_fallback() {
    // Every nested target category other than list cannot be extended: the
    // root degrades and the attempted `Extend` fact is kept.
    for source in [
        "ROOT = ['x']\nROOT[0] += ['a']\n",
        "ROOT = ((),)\nROOT[0] += ['a']\n",
        "ROOT = ({},)\nROOT[0] += ['a']\n",
        "from pathlib import Path\nROOT = (Path('/x'),)\nROOT[0] += ['a']\n",
        "ROOT = (True,)\nROOT[0] += ['a']\n",
        "ROOT = (MISSING,)\nROOT[0] += ['a']\n",
    ] {
        let (_file, evaluation) = evaluate_module(source);
        assert!(
            has_unknown_alternative(&evaluation, "ROOT"),
            "{source}: a non-list nested target degrades the root",
        );
        assert!(
            evaluation
                .mutations
                .iter()
                .any(|mutation| mutation.binding == "ROOT"
                    && mutation.operation == PythonMutationOperationView::Extend),
            "{source}: the attempted extend fact is recorded",
        );
    }
}

#[test]
fn extend_non_list_receiver_categories_fallback() {
    // `.extend()` on every non-list receiver category is an unsupported-mutation
    // call: the receiver degrades and no mutation fact is recorded.
    for receiver in ["()", "'text'", "{}", "Path('/x')", "True", "MISSING"] {
        let source =
            format!("from pathlib import Path\nRECEIVER = {receiver}\nRECEIVER.extend(['a'])\n");
        let (_file, evaluation) = evaluate_module(&source);
        assert!(
            has_unknown_alternative(&evaluation, "RECEIVER"),
            "receiver `{receiver}` should degrade",
        );
        assert!(
            evaluation.mutations.is_empty(),
            "receiver `{receiver}` should record no mutation fact",
        );
    }
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
                && unknown.origins.as_slice() == [djls_source::Origin::new(
                    settings,
                    expected_span(source, "from missing_star import *"),
                )]
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
            && unknown.origins.as_slice() == [djls_source::Origin::new(
                settings,
                expected_span(source, source.trim_end()),
            )]
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
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(
            &unknown.cause,
            PythonUnknownCauseView::MissingImportMember { module, member }
                if module.as_str() == "broken" && member == "VALUE"
        )
    )));
    assert!(
        !binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
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
            && unknown.origins.as_slice() == [djls_source::Origin::new(
                settings,
                expected_span(source, "VALUES.clear()"),
            )]
    ));
}

#[test]
fn python_module_evaluation_records_nested_string_augmented_add_as_iterable_extension() {
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

    // A nested `list += str` consumes a known-but-imprecise iterable in place:
    // the receiver stays a list rather than degrading to an unsupported-mutation
    // unknown.
    let binding = evaluation
        .binding("TEMPLATES")
        .expect("the mutated binding should remain observable");
    assert!(binding.alternatives.iter().all(|alternative| {
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
fn typed_module_order_disjoint_import_cycles_are_root_order_independent() {
    let cycle_edges = |settings_source| {
        let db = TestDatabase::new();
        db.add_file("/project/settings.py", settings_source);
        db.add_file("/project/a.py", "from b import B\nA = B\n");
        db.add_file("/project/b.py", "from a import A\nB = A\n");
        db.add_file("/project/x.py", "from y import Y\nX = Y\n");
        db.add_file("/project/y.py", "from x import X\nY = X\n");
        let project = python_project(&db);
        let settings = db.file(Utf8Path::new("/project/settings.py"));

        python_module_evaluation(&db, project, settings)
            .imports
            .iter()
            .filter_map(|outcome| match outcome {
                PythonImportOutcomeView::Cycle {
                    importer_module,
                    imported_module,
                    ..
                } => Some((
                    importer_module.as_str().to_string(),
                    imported_module.as_str().to_string(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    let expected = [
        ("a".to_string(), "b".to_string()),
        ("x".to_string(), "y".to_string()),
    ];
    assert_eq!(cycle_edges("from a import A\nfrom x import X\n"), expected);
    assert_eq!(cycle_edges("from x import X\nfrom a import A\n"), expected);
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
fn canonical_unknown_origins_merge_dynamic_namespace_import_edges() {
    let source = "from a import *\nfrom b import *\n";
    let (db, project, settings) = extract_project(
        source,
        &[("a", "from b import *\n"), ("b", "from a import *\n")],
    );
    let settings_file = settings_module_file(&db, project).unwrap();
    let evaluation = python_module_evaluation(&db, project, settings_file);
    let expected_origins = ["from a import *", "from b import *"]
        .map(|statement| djls_source::Origin::new(settings_file, expected_span(source, statement)));
    let [unknown] = evaluation.namespace_unknowns.as_slice() else {
        panic!("cycle causes should merge into one plural unknown: {evaluation:#?}");
    };
    assert_eq!(unknown.cause, PythonUnknownCauseView::Cycle);
    assert_eq!(unknown.origins, expected_origins);

    let dynamic_namespace_issues = cases(&settings, "/installed_apps/cases")
        .iter()
        .find_map(|case| case.get("dynamic"))
        .expect("the open namespace should produce a dynamic settings case")["apps"]["evidence"]
        .as_array()
        .unwrap();
    let [evidence] = dynamic_namespace_issues.as_slice() else {
        panic!("plural namespace provenance should project through one issue");
    };
    assert_eq!(evidence["issue"]["kind"], "dynamic_namespace");
    assert_eq!(
        evidence["issue"]["spans"],
        serde_json::to_value(expected_origins.map(|origin| origin.span)).unwrap()
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
            && unknown.origins.as_slice() == [import_origin]
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
                    && unknown.origins.as_slice() == [djls_source::Origin::new(
                        external,
                        expected_span("from a import *\n", "from a import *"),
                    )]
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
    let settings = extract("INSTALLED_APPS = ['blog']\nTEMPLATES = []\ndef broken(\n");

    for pointer in ["/installed_apps/cases", "/templates/cases"] {
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
fn path_collections_keep_known_paths_and_uncertainty_in_source_order() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/first', *UNKNOWN, '/later']}]",
    );
    let template_evidence = &cases(&settings, "/templates/cases")[0]["dynamic"]["templates"]["evidence"]
        [0]["backend"]["dirs"]["evidence"];
    assert_eq!(template_evidence[0]["known"]["value"]["resolved"], "/first");
    assert_eq!(template_evidence[1]["issue"]["kind"], "unknown_unpack");
    assert_eq!(template_evidence[2]["known"]["value"]["resolved"], "/later");
}

#[test]
fn exact_path_lists_preserve_duplicate_entries() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/same', '/same']} ]",
    );

    let template_dirs = &cases(&settings, "/templates/cases")[0]["known"]["backends"][0]["dirs"];
    assert_eq!(template_dirs.as_array().unwrap().len(), 2);
    assert_eq!(template_dirs[0]["value"]["resolved"], "/same");
    assert_eq!(template_dirs[1]["value"]["resolved"], "/same");
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
fn straight_line_unsupported_mutation_discards_stale_installed_apps() {
    let source = "INSTALLED_APPS = ['stale']\nINSTALLED_APPS.clear()";
    let settings = extract(source);
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{source}");
    assert!(cases[0].get("known").is_none(), "{source}");
    assert_eq!(
        cases[0]["dynamic"]["apps"]["evidence"][0]["issue"]["kind"], "unsupported_mutation",
        "{source}"
    );
    assert!(!cases[0].to_string().contains("stale"), "{source}");

    let origin = binding_unknown_origin(source, "INSTALLED_APPS");
    assert_eq!(
        &source[origin.span.start_usize()..origin.span.end_usize()],
        "INSTALLED_APPS.clear()",
    );
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
fn binary_list_plus_string_does_not_preserve_stale_installed_apps() {
    // Binary `+` never delegates to iterable extension: a list-plus-string is a
    // wholly unsupported expression, so no known prefix survives.
    let source = "INSTALLED_APPS = ['stale'] + 'invalid'";
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

#[test]
fn list_augmented_add_string_preserves_prefix_and_unknown_remainder() {
    // `list += str` recognizes a known-but-imprecise iterable: the known prefix
    // survives and the string contributes a typed unknown-unpack remainder.
    let source = "INSTALLED_APPS = ['stale']\nINSTALLED_APPS += 'invalid'";
    let settings = extract(source);
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{source}");
    let evidence = cases[0]["dynamic"]["apps"]["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 2, "{source}");
    assert_eq!(evidence[0]["known"]["value"], "stale", "{source}");
    assert_eq!(evidence[1]["issue"]["kind"], "unknown_unpack", "{source}");
}

#[test]
fn simple_mutable_alias_mutation_keeps_source_setting_conservative() {
    let settings = extract("INSTALLED_APPS = []\nAPPS = INSTALLED_APPS\nAPPS.append('blog')");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn mutating_a_container_preserves_unmodified_nested_settings() {
    let settings = extract(
        "INSTALLED_APPS = ['kept']\nWRAPPER = [INSTALLED_APPS]\nWRAPPER.append('unrelated')",
    );
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();

    assert_eq!(apps.len(), 1, "{settings:#}");
    assert_eq!(apps[0]["value"], "kept");
}

#[test]
fn nested_augmented_assignment_invalidates_mutable_aliases() {
    let settings = extract(
        "INSTALLED_APPS = []\nWRAPPER = {'apps': INSTALLED_APPS}\nWRAPPER['apps'] += ['blog']",
    );
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn repeated_nested_aliases_in_the_mutated_root_become_conservative() {
    let settings = extract(
        "DIRS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': DIRS},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': DIRS},\n]\nTEMPLATES[0]['DIRS'].append('/shared')",
    );
    let cases = cases(&settings, "/templates/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn mutating_multi_case_bindings_invalidates_their_aliases() {
    let settings = extract(
        "if FLAG:\n    BASE = ['first']\nelse:\n    BASE = ['second']\nINSTALLED_APPS = BASE\nBASE.append('blog')",
    );
    let cases = cases(&settings, "/installed_apps/cases");

    assert!(!cases.is_empty(), "{settings:#}");
    assert!(
        cases.iter().all(|case| case.get("dynamic").is_some()),
        "{settings:#}"
    );
    assert!(
        cases.iter().all(|case| case.get("known").is_none()),
        "{settings:#}"
    );
}

#[test]
fn arbitrary_calls_degrade_mutable_aliases() {
    let settings = extract("INSTALLED_APPS = []\nAPPS = INSTALLED_APPS\nconfigure(APPS)");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 2, "{settings:#}");
    assert!(
        cases.iter().any(|case| case.get("dynamic").is_some()),
        "{settings:#}"
    );
}

#[test]
fn uncertain_dictionary_overrides_invalidate_possible_mutation_aliases() {
    let settings = extract(
        "INSTALLED_APPS = []\nEXTRA = make_mapping()\nWRAPPER = {'apps': INSTALLED_APPS, **EXTRA}\nWRAPPER['apps'].append('blog')",
    );
    let cases = cases(&settings, "/installed_apps/cases");

    assert!(!cases.is_empty(), "{settings:#}");
    assert!(
        cases.iter().all(|case| case.get("dynamic").is_some()),
        "{settings:#}"
    );
    assert!(
        cases.iter().all(|case| case.get("known").is_none()),
        "{settings:#}"
    );
}

#[test]
fn tuple_mutation_is_not_treated_as_a_supported_list_mutation() {
    let settings = extract("INSTALLED_APPS = ('first',)\nINSTALLED_APPS.append('second')");
    let cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn failed_recognized_mutations_degrade_argument_aliases() {
    let settings = extract(
        "INSTALLED_APPS = []\nAPPS = INSTALLED_APPS\nHANDLER = factory()\nHANDLER.append(APPS)",
    );
    let cases = cases(&settings, "/installed_apps/cases");

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
        let settings = extract(source);
        let cases = cases(&settings, "/installed_apps/cases");

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
    let settings = extract("APPS = []\nINSTALLED_APPS = APPS\nINSTALLED_APPS.append('blog')");
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();

    assert_eq!(apps.len(), 1, "{settings:#}");
    assert_eq!(apps[0]["value"], "blog");
}

#[test]
fn chained_immutable_assignment_remains_exact() {
    let (_, evaluation) = evaluate_module("VALUE = ALIAS = '/static/'");

    for name in ["VALUE", "ALIAS"] {
        assert!(matches!(
            &bound_value(&evaluation, name).value.kind,
            PythonValueKindView::Str(value) if value == "/static/"
        ));
    }
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
fn tuple_installed_apps_are_accepted_like_lists() {
    let settings = extract("INSTALLED_APPS = ('first', 'second')");
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();

    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "first");
    assert_eq!(apps[1]["value"], "second");
}

#[test]
fn string_installed_apps_are_not_accepted_as_a_collection() {
    let settings = extract("INSTALLED_APPS = 'blog'");
    let case = &cases(&settings, "/installed_apps/cases")[0];

    assert!(
        case.get("known").is_none(),
        "a bare string is not a collection: {settings:#}"
    );
    assert!(case.get("malformed").is_some(), "{settings:#}");
}

#[test]
fn tuple_collection_shaped_template_settings_are_accepted() {
    let settings = extract(
        "TEMPLATES = ({'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ('/templates',), 'OPTIONS': {'context_processors': ('app.context.processor',), 'builtins': ('app.builtins',)}},)",
    );

    let backend = &cases(&settings, "/templates/cases")[0]["known"]["backends"][0];
    assert_eq!(backend["dirs"][0]["value"]["resolved"], "/templates");
}

#[test]
fn binary_tuple_plus_tuple_preserves_exact_installed_apps() {
    let settings = extract("INSTALLED_APPS = ('alpha',) + ('beta',)");
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();
    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "alpha");
    assert_eq!(apps[1]["value"], "beta");
}

#[test]
fn binary_string_plus_string_is_exact() {
    let (_, evaluation) = evaluate_module("VALUE = '/sta' + 'tic/'");

    assert!(matches!(
        &bound_value(&evaluation, "VALUE").value.kind,
        PythonValueKindView::Str(value) if value == "/static/"
    ));
}

#[test]
fn binary_cross_kind_list_and_tuple_is_unsupported() {
    for source in [
        "INSTALLED_APPS = ['alpha'] + ('beta',)",
        "INSTALLED_APPS = ('alpha',) + ['beta']",
    ] {
        let settings = extract(source);
        let cases = cases(&settings, "/installed_apps/cases");
        assert_eq!(cases.len(), 1, "{source}");
        assert!(cases[0].get("known").is_none(), "{source}");
        assert!(!cases[0].to_string().contains("alpha"), "{source}");
    }
}

#[test]
fn name_target_tuple_augmented_add_tuple_is_exact() {
    let settings = extract("INSTALLED_APPS = ('alpha',)\nINSTALLED_APPS += ('beta',)");
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();
    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "alpha");
    assert_eq!(apps[1]["value"], "beta");
}

#[test]
fn name_target_tuple_augmented_add_unknown_keeps_prefix() {
    let settings = extract("INSTALLED_APPS = ('alpha',)\nINSTALLED_APPS += EXTRA");
    let cases = cases(&settings, "/installed_apps/cases");
    assert_eq!(cases.len(), 1, "{settings:#}");
    let evidence = cases[0]["dynamic"]["apps"]["evidence"].as_array().unwrap();
    assert_eq!(evidence[0]["known"]["value"], "alpha");
    assert_eq!(evidence[1]["issue"]["kind"], "unknown_unpack");
}

#[test]
fn list_augmented_add_tuple_preserves_exact_order() {
    let settings = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS += ('beta',)");
    let apps = cases(&settings, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|app| app["value"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(apps, ["alpha", "beta"], "{settings:#}");
}

#[test]
fn list_augmented_add_bool_discards_prefix() {
    let settings = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS += True");
    let cases = cases(&settings, "/installed_apps/cases");
    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
    assert!(!cases[0].to_string().contains("alpha"), "{settings:#}");
}

#[test]
fn list_extend_tuple_is_exact_but_extend_bool_degrades() {
    let exact = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS.extend(('beta',))");
    let apps = cases(&exact, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();
    assert_eq!(apps.len(), 2, "{exact:#}");

    let degraded = extract("INSTALLED_APPS = ['alpha']\nINSTALLED_APPS.extend(True)");
    let cases = cases(&degraded, "/installed_apps/cases");
    assert!(cases[0].get("known").is_none(), "{degraded:#}");
    assert!(!cases[0].to_string().contains("alpha"), "{degraded:#}");
}

#[test]
fn starred_construction_follows_iterable_matrix() {
    let exact = extract("INSTALLED_APPS = [*('alpha', 'beta')]");
    let apps = cases(&exact, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();
    assert_eq!(apps.len(), 2, "{exact:#}");

    let imprecise = extract("INSTALLED_APPS = ['alpha', *'xy']");
    let imprecise_cases = cases(&imprecise, "/installed_apps/cases");
    assert!(imprecise_cases[0].get("known").is_none(), "{imprecise:#}");
    assert!(
        imprecise_cases[0].to_string().contains("unknown_unpack"),
        "{imprecise:#}"
    );

    let bool_source = extract("INSTALLED_APPS = ['alpha', *True]");
    let bool_cases = cases(&bool_source, "/installed_apps/cases");
    assert!(bool_cases[0].get("known").is_none(), "{bool_source:#}");
    assert!(
        !bool_cases[0].to_string().contains("alpha"),
        "{bool_source:#}"
    );
}

#[test]
fn starred_tuple_construction_follows_iterable_matrix() {
    let exact = extract("INSTALLED_APPS = (*('alpha', 'beta'),)");
    let apps = cases(&exact, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();
    assert_eq!(apps.len(), 2, "{exact:#}");

    for rhs in ["''", "{}", "EXTRA", "Path('/x')"] {
        let source = format!("from pathlib import Path\nINSTALLED_APPS = ('alpha', *{rhs})");
        let settings = extract(&source);
        let case = &cases(&settings, "/installed_apps/cases")[0];
        assert!(case.get("known").is_none(), "{source}: {settings:#}");
        assert!(case.to_string().contains("unknown_unpack"), "{source}");
    }

    let invalid = extract("INSTALLED_APPS = ('alpha', *True)");
    let case = &cases(&invalid, "/installed_apps/cases")[0];
    assert!(!case.to_string().contains("alpha"), "{invalid:#}");
}

#[test]
fn name_target_string_augmented_add_is_exact_or_degrades() {
    let (_, exact) = evaluate_module("VALUE = '/sta'\nVALUE += 'tic/'");
    assert!(matches!(
        &bound_value(&exact, "VALUE").value.kind,
        PythonValueKindView::Str(value) if value == "/static/"
    ));

    for source in [
        "VALUE = '/static/'\nVALUE += EXTRA",
        "from pathlib import Path\nVALUE = '/static/'\nVALUE += Path('/x')",
    ] {
        let (_, degraded) = evaluate_module(source);
        assert!(has_unknown_alternative(&degraded, "VALUE"), "{degraded:#?}");
    }
}

#[test]
fn name_target_tuple_augmented_add_incompatible_kinds_degrade() {
    for rhs in ["['beta']", "'beta'", "True", "{'beta': 1}", "Path('/x')"] {
        let source = format!(
            "from pathlib import Path\nINSTALLED_APPS = ('alpha',)\nINSTALLED_APPS += {rhs}"
        );
        let settings = extract(&source);
        let cases = cases(&settings, "/installed_apps/cases");
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
    for rhs in ["''", "{'k': 'v'}", "{}", "EXTRA", "Path('/x')"] {
        let source = format!(
            "from pathlib import Path\nINSTALLED_APPS = ['alpha']\nINSTALLED_APPS += {rhs}"
        );
        let settings = extract(&source);
        let cases = cases(&settings, "/installed_apps/cases");
        assert_eq!(cases.len(), 1, "{source}");
        let evidence = cases[0]["dynamic"]["apps"]["evidence"].as_array().unwrap();
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
    for rhs in ["'xy'", "''", "{'k': 'v'}", "{}", "EXTRA", "Path('/x')"] {
        let source = format!(
            "from pathlib import Path\nINSTALLED_APPS = ['alpha']\nINSTALLED_APPS.extend({rhs})"
        );
        let settings = extract(&source);
        let cases = cases(&settings, "/installed_apps/cases");
        assert_eq!(cases.len(), 1, "{source}");
        let evidence = cases[0]["dynamic"]["apps"]["evidence"].as_array().unwrap();
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
    let list = extract("INSTALLED_APPS = [*['alpha', 'beta']]");
    let apps = cases(&list, "/installed_apps/cases")[0]["known"]["apps"]
        .as_array()
        .unwrap();
    assert_eq!(apps.len(), 2, "{list:#}");

    for rhs in ["''", "{'k': 'v'}", "{}", "EXTRA", "Path('/x')"] {
        let source = format!("from pathlib import Path\nINSTALLED_APPS = ['alpha', *{rhs}]");
        let settings = extract(&source);
        let cases = cases(&settings, "/installed_apps/cases");
        assert!(cases[0].get("known").is_none(), "{source}: {settings:#}");
        assert!(
            cases[0].to_string().contains("unknown_unpack"),
            "{source}: {settings:#}"
        );
    }
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
    let (_, evaluation) = evaluate_module(
        "DIRS = []\nWRAPPER = {'DIRS': DIRS}\nLEFT = RIGHT = WRAPPER\nRIGHT['DIRS'].append('/templates')",
    );

    assert!(
        has_unknown_alternative(&evaluation, "DIRS"),
        "{evaluation:#?}"
    );
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
fn differing_branch_scalars_distribute_through_settings_collections() {
    let settings = extract_project(
        "if FLAG:\n    from one.values import ROOT, BASE_APPS\nelse:\n    from two.values import ROOT, BASE_APPS\nINSTALLED_APPS = BASE_APPS + ['local']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT]}]",
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

    let paths = cases(&settings, "/templates/cases")
        .iter()
        .map(|case| {
            case["known"]["backends"][0]["dirs"][0]["value"]["resolved"]
                .as_str()
                .unwrap()
        })
        .collect::<std::collections::BTreeSet<_>>();
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
    assert_eq!(cases(&settings, "/templates/cases").len(), 4);
}

#[test]
fn repeated_branch_selected_scalar_retains_two_feasible_configurations() {
    let repeated = std::iter::repeat_n("SHARED", 7)
        .collect::<Vec<_>>()
        .join(", ");
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
    .2;

    let cases = cases(&settings, "/templates/cases");
    assert_eq!(cases.len(), 2);
    assert!(cases.iter().all(|case| case.get("known").is_some()));
    let roots = cases
        .iter()
        .map(|case| {
            let paths = case["known"]["backends"][0]["dirs"].as_array().unwrap();
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

#[test]
fn typed_value_order_setting_lists_keep_cap_projection_for_reversed_input() {
    let at_limit = equal_top_level_list_alternatives(64, false);
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

    let overflowed = equal_top_level_list_alternatives(65, false);
    assert_eq!(
        overflowed,
        equal_top_level_list_alternatives(65, true),
        "top-level list projection must ignore equivalent reversed module installation"
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
        2,
        "remainder should retain the omitted path and overflow operation: {overflowed:#}"
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
            writeln!(source, "elif FLAG_{index}:").unwrap();
        }
        if index == 0 {
            source.push_str("    INSTALLED_APPS = False\n");
        } else {
            writeln!(source, "    INSTALLED_APPS = ['app_{index:02}']").unwrap();
        }
    }
    source.push_str("INSTALLED_APPS.append(@)\n");

    let settings = extract(&source);
    let setting_cases = cases(&settings, "/installed_apps/cases");

    assert_eq!(setting_cases.len(), 65, "{settings:#}");
    let malformed_cases = setting_cases
        .iter()
        .filter_map(|case| case.get("malformed"))
        .collect::<Vec<_>>();
    let [malformed] = malformed_cases.as_slice() else {
        panic!("expected one Malformed case: {settings:#}");
    };
    assert_eq!(
        malformed["apps"]["evidence"],
        json!([
            {
                "issue": {
                    "kind": "invalid_shape",
                    "spans": [expected_span(&source, "False")],
                }
            },
        ]),
        "the Malformed case must not absorb overflow or syntax evidence: {settings:#}"
    );

    let dynamic_cases = setting_cases
        .iter()
        .filter_map(|case| case.get("dynamic"))
        .collect::<Vec<_>>();
    let [remainder] = dynamic_cases.as_slice() else {
        panic!("expected one capped Dynamic remainder: {settings:#}");
    };
    assert_eq!(
        remainder["apps"]["evidence"],
        json!([
            {
                "issue": {
                    "kind": "dynamic_expression",
                    "spans": [Span::saturating_from_parts_usize(
                        0,
                        source.find("\nINSTALLED_APPS.append").unwrap(),
                    )],
                }
            },
            {
                "issue": {
                    "kind": "syntax_error",
                    "spans": [
                        expected_span(&source, "@"),
                        expected_span(&source, ")"),
                    ],
                }
            },
        ]),
        "{settings:#}"
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
        "if FLAG:\n    from one.base import TEMPLATES\nelse:\n    from two.base import TEMPLATES",
        &[
            (
                "one.base",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['first', 'second']}]",
            ),
            (
                "two.base",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['first', 'second']}]",
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
}

#[test]
fn equal_mixed_origin_path_lists_retain_each_original_configuration() {
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

    assert_eq!(template_dirs, expected);
}

#[test]
fn all_setting_families_distinguish_known_unset_dynamic_and_malformed() {
    let known = extract("INSTALLED_APPS = []\nTEMPLATES = []");
    let unset = extract("");
    let dynamic = extract("INSTALLED_APPS = unknown\nTEMPLATES = unknown");
    let malformed = extract("INSTALLED_APPS = False\nTEMPLATES = False");
    for (pointer, payload_field) in [
        ("/installed_apps/cases", "apps"),
        ("/templates/cases", "templates"),
    ] {
        assert!(
            cases(&known, pointer)[0].get("known").is_some(),
            "{pointer}"
        );
        assert_eq!(cases(&unset, pointer), [json!("unset")], "{pointer}");

        let dynamic_case = &cases(&dynamic, pointer)[0];
        let malformed_case = &cases(&malformed, pointer)[0];
        let dynamic_payload = dynamic_case["dynamic"].as_object().unwrap();
        let malformed_payload = malformed_case["malformed"].as_object().unwrap();
        assert_eq!(dynamic_case.as_object().unwrap().len(), 1, "{pointer}");
        assert_eq!(malformed_case.as_object().unwrap().len(), 1, "{pointer}");
        assert_eq!(
            dynamic_payload
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [payload_field],
            "{pointer}"
        );
        assert_eq!(
            malformed_payload
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [payload_field],
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
fn canonical_unknown_origins_merge_top_level_branch_spans() {
    let source = "if FLAG:\n    VALUE = first()\nelse:\n    VALUE = second()\n";
    let (_, evaluation) = evaluate_module(source);
    let PythonValueKindView::Unknown(unknown) = &bound_value(&evaluation, "VALUE").value.kind
    else {
        panic!("equal unknown branches should produce one unknown: {evaluation:#?}");
    };

    assert_eq!(unknown.cause, PythonUnknownCauseView::UnsupportedExpression);
    assert_eq!(
        unknown
            .origins
            .iter()
            .map(|origin| origin.span)
            .collect::<Vec<_>>(),
        [
            expected_span(source, "first()"),
            expected_span(source, "second()"),
        ]
    );
}

#[test]
fn canonical_unknown_origins_project_mapping_unpack_spans() {
    let source = "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {**first()}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {**second()}}}]\n";
    let settings = extract(source);
    let libraries = &cases(&settings, "/templates/cases")[0]["dynamic"]["templates"]["evidence"][0]
        ["backend"]["libraries"];
    let issues = libraries["issues"]
        .as_array()
        .expect("unknown mapping unpack should produce issues");
    let [issue] = issues.as_slice() else {
        panic!("equal unknown mapping branches should produce one issue: {settings:#}");
    };
    assert_eq!(issue["kind"], "unknown_unpack");
    assert_eq!(
        issue["spans"],
        serde_json::to_value([
            expected_span(source, "first()"),
            expected_span(source, "second()"),
        ])
        .unwrap()
    );
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
