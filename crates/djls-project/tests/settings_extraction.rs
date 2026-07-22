use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io;
use std::iter;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::TagSpecDef;
use djls_project::Db;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::PythonSourceModule;
use djls_project::SearchPaths;
use djls_project::TemplateName;
use djls_project::TemplateResolutionResult;
use djls_project::template_resolution;
use djls_project::testing::PythonBindingAlternativeView;
use djls_project::testing::PythonBoundValueView;
use djls_project::testing::PythonImportNameErrorView;
use djls_project::testing::PythonImportOutcomeView;
use djls_project::testing::PythonModuleEvaluationError;
use djls_project::testing::PythonModuleEvaluationView;
use djls_project::testing::PythonModuleView;
use djls_project::testing::PythonMutationOperationView;
use djls_project::testing::PythonMutationPathSegmentView;
use djls_project::testing::PythonPathIntrinsicView;
use djls_project::testing::PythonPathView;
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
use djls_source::CaseSensitivity;
use djls_source::ChangeEvent;
use djls_source::File;
use djls_source::FileReadErrorKind;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::Origin;
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
use serde_json::to_value;

fn extract_project(
    source: &str,
    modules: &[(&str, &str)],
) -> Result<(TestDatabase, Project, Value), Box<dyn std::error::Error>> {
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

fn binding_unknown_origin(source: &str, name: &str) -> Result<Origin, Box<dyn std::error::Error>> {
    let (db, project, _) = extract_project(source, &[])?;
    let file = settings_module_file(&db, project).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "test settings module should resolve to a source file",
        )
    })?;
    let evaluation = python_module_evaluation(&db, project, file)?;
    let binding = evaluation.binding(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("expected test binding `{name}` should exist"),
        )
    })?;
    let [PythonBindingAlternativeView::Bound(bound)] = binding.alternatives.as_slice() else {
        return Err(io::Error::other(format!("expected one bound alternative for {name}")).into());
    };
    let PythonValueKindView::Unknown(unknown) = &bound.value.kind else {
        return Err(io::Error::other(format!("expected unknown value for {name}")).into());
    };
    if unknown.cause != PythonUnknownCauseView::UnsupportedMutation {
        return Err(io::Error::other(format!(
            "expected unsupported mutation for {name}, got {:?}",
            unknown.cause
        ))
        .into());
    }
    Ok(unknown.origins.first().copied().ok_or_else(|| {
        io::Error::other(format!("unknown binding `{name}` should retain an origin"))
    })?)
}

fn python_project(db: &dyn Db) -> Project {
    python_project_with_paths(db, &[])
}

fn python_project_with_paths(db: &dyn Db, pythonpath: &[Utf8PathBuf]) -> Project {
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

fn expected_span(source: &str, needle: &str) -> Option<Span> {
    source
        .find(needle)
        .map(|start| Span::saturating_from_parts_usize(start, needle.len()))
}

/// Span of a binding target (`target`) located inside a specific import clause
/// (`clause`), disambiguating targets whose text also appears elsewhere in the
/// source (e.g. an alias `stale` distinct from an earlier `stale = ...`).
fn binding_target_span(source: &str, clause: &str, target: &str) -> Option<Span> {
    let clause_start = source.find(clause)?;
    let within = clause.find(target)?;
    Some(Span::saturating_from_parts_usize(
        clause_start + within,
        target.len(),
    ))
}

fn only<T>(values: &[T]) -> Option<&T> {
    let [value] = values else {
        return None;
    };
    Some(value)
}

fn only_bound(alternatives: &[PythonBindingAlternativeView]) -> Option<&PythonBoundValueView> {
    let [PythonBindingAlternativeView::Bound(bound)] = alternatives else {
        return None;
    };
    Some(bound)
}

fn list_items(kind: &PythonValueKindView) -> Option<&[PythonSequenceItemView]> {
    let PythonValueKindView::List(items) = kind else {
        return None;
    };
    Some(items)
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

fn is_alternative_limit_unknown(alternative: &PythonBindingAlternativeView) -> bool {
    matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
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
fn python_module_evaluation_follows_recursive_imports() {
    let db = TestDatabase::new();
    db.add_file("/project/base.py", "VALUE = 'from base'\n")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/settings.py",
        "from base import VALUE\nCOPY = VALUE\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let value = evaluation.binding("VALUE").expect("VALUE should be bound");
    let copy = evaluation.binding("COPY").expect("COPY should be bound");
    assert_eq!(value.alternatives.len(), 1);
    assert_eq!(copy.alternatives.len(), 1);
    assert_eq!(evaluation.dependency_files.len(), 2);
    assert!(evaluation
        .imports
        .iter()
        .any(|outcome| matches!(outcome, PythonImportOutcomeView::Resolved { file, .. } if *file == db.file(Utf8Path::new("/project/base.py")).expect("settings-extraction test file should exist"))));
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
fn python_binding_alternative_limit_has_exact_boundary_and_unknown_remainder() {
    let evaluate = |count| {
        let db = TestDatabase::new();
        db.add_file("/project/settings.py", branch_alternatives(count).as_str())
            .expect("settings-extraction test file should be added");
        let project = python_project(&db);
        let settings = db
            .file(Utf8Path::new("/project/settings.py"))
            .expect("settings-extraction test file should exist");
        python_module_evaluation(&db, project, settings)
            .expect("Python file should map to a module")
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
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
            value: PythonValueView {
                kind: PythonValueKindView::Str(value),
                origins,
            },
            binding_origins,
        }) if value == "set"
            && origins.as_slice() == [Origin::new(settings, expected_span(source, "'set'").expect("test source should contain expected text"))]
            && binding_origins.as_slice() == [Origin::new(settings, expected_span(source, "'set'").expect("test source should contain expected text"))]
    )));
}

#[test]
fn os_path_abspath_follows_import_and_assignment_aliases() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "import os as operating_system\nfrom os.path import abspath as path_abspath\nassigned_abspath = path_abspath\nROOT_ABS = operating_system.path.abspath(__file__)\nBASE_DIR = operating_system.path.dirname(operating_system.path.dirname(operating_system.path.abspath(__file__)))\nNORMALIZED_ABS = path_abspath('/project/one/../settings.py')\nASSIGNED_ABS = assigned_abspath(__file__)\nRELATIVE_ABS = path_abspath('relative')\nMISSING_ABS = path_abspath()\nKEYWORD_ABS = path_abspath(path=__file__)\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let assert_kind = |name: &str, expected: PythonValueKindView| {
        let binding = evaluation.binding(name).expect("binding should exist");
        let bound =
            only_bound(&binding.alternatives).expect("binding should have one bound alternative");
        assert_eq!(bound.value.kind, expected, "unexpected value for {name}");
    };
    for name in ["path_abspath", "assigned_abspath"] {
        assert_kind(
            name,
            PythonValueKindView::Path(PythonPathView::Intrinsic(
                PythonPathIntrinsicView::OsPathAbspathFunction,
            )),
        );
    }
    for name in ["ROOT_ABS", "NORMALIZED_ABS", "ASSIGNED_ABS"] {
        assert_kind(
            name,
            PythonValueKindView::Str("/project/settings.py".to_string()),
        );
    }
    assert_kind("BASE_DIR", PythonValueKindView::Str("/".to_string()));
    for name in ["RELATIVE_ABS", "MISSING_ABS", "KEYWORD_ABS"] {
        let binding = evaluation.binding(name).expect("binding should exist");
        let bound =
            only_bound(&binding.alternatives).expect("binding should have one bound alternative");
        assert!(matches!(bound.value.kind, PythonValueKindView::Unknown(_)));
    }
}

#[test]
fn python_path_intrinsics_follow_import_and_assignment_aliases() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from pathlib import Path as P\nimport os as operating_system\nfrom os.path import join as path_join, dirname as path_dirname\nstringify = str\nMODULE_FILE = __file__\nROOT = P(__file__).parent\nRESOLVED = P(__file__).resolve()\nNORMALIZED = P(__file__).parent.joinpath('..').resolve()\nTEMPLATES_DIR = operating_system.path.join(ROOT, 'templates')\nSTATIC_DIR = path_join(ROOT, 'static')\nPARENT = path_dirname(TEMPLATES_DIR)\nEMPTY_PARENT = path_dirname('')\nTRAILING_PARENT = path_dirname('/project/')\nROOT_PARENT = path_dirname('/')\nSTATIC_TEXT = stringify(STATIC_DIR)\nRELATIVE_PATH = P('relative')\nINVALID_METHOD = TEMPLATES_DIR.parent\nINVALID_DIVISION = TEMPLATES_DIR / 'nested'\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let assert_kind = |name: &str, expected: PythonValueKindView| {
        let binding = evaluation.binding(name).expect("binding should exist");
        let bound =
            only_bound(&binding.alternatives).expect("binding should have one bound alternative");
        assert_eq!(bound.value.kind, expected, "unexpected value for {name}");
    };

    assert_kind(
        "P",
        PythonValueKindView::Path(PythonPathView::Intrinsic(
            PythonPathIntrinsicView::PathlibPathType,
        )),
    );
    assert_kind(
        "operating_system",
        PythonValueKindView::Path(PythonPathView::Intrinsic(PythonPathIntrinsicView::OsModule)),
    );
    assert_kind(
        "path_join",
        PythonValueKindView::Path(PythonPathView::Intrinsic(
            PythonPathIntrinsicView::OsPathJoinFunction,
        )),
    );
    assert_kind(
        "path_dirname",
        PythonValueKindView::Path(PythonPathView::Intrinsic(
            PythonPathIntrinsicView::OsPathDirnameFunction,
        )),
    );
    assert_kind(
        "stringify",
        PythonValueKindView::Path(PythonPathView::Intrinsic(
            PythonPathIntrinsicView::BuiltinStrType,
        )),
    );
    assert_kind(
        "MODULE_FILE",
        PythonValueKindView::Str("/project/settings.py".to_string()),
    );
    assert_kind(
        "ROOT",
        PythonValueKindView::Path(PythonPathView::Object("/project".into())),
    );
    assert_kind(
        "RESOLVED",
        PythonValueKindView::Path(PythonPathView::Object("/project/settings.py".into())),
    );
    assert_kind(
        "NORMALIZED",
        PythonValueKindView::Path(PythonPathView::Object("/".into())),
    );
    assert_kind(
        "TEMPLATES_DIR",
        PythonValueKindView::Str("/project/templates".to_string()),
    );
    assert_kind(
        "STATIC_DIR",
        PythonValueKindView::Str("/project/static".to_string()),
    );
    assert_kind("PARENT", PythonValueKindView::Str("/project".to_string()));
    assert_kind("EMPTY_PARENT", PythonValueKindView::Str(String::new()));
    assert_kind(
        "TRAILING_PARENT",
        PythonValueKindView::Str("/project".to_string()),
    );
    assert_kind("ROOT_PARENT", PythonValueKindView::Str("/".to_string()));
    assert_kind(
        "STATIC_TEXT",
        PythonValueKindView::Str("/project/static".to_string()),
    );
    for name in ["RELATIVE_PATH", "INVALID_METHOD", "INVALID_DIVISION"] {
        let binding = evaluation.binding(name).expect("binding should exist");
        let bound =
            only_bound(&binding.alternatives).expect("binding should have one bound alternative");
        assert!(matches!(bound.value.kind, PythonValueKindView::Unknown(_)));
    }
    assert!(
        evaluation
            .imports
            .iter()
            .all(|outcome| !matches!(outcome, PythonImportOutcomeView::NotFound { .. }))
    );
    assert!(
        evaluation
            .imports
            .iter()
            .all(|outcome| matches!(outcome, PythonImportOutcomeView::SkippedExternal { .. }))
    );
}

#[test]
fn python_path_intrinsics_respect_shadowing_and_branch_constraints() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from pathlib import Path\nimport os\nif FLAG:\n    Path = custom_path\nif OTHER:\n    os = custom_os\nif THIRD:\n    str = custom_str\nPATH_VALUE = Path(__file__)\nOS_VALUE = os.path.join('/project', 'templates')\nSTR_VALUE = str('/project/templates')\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["PATH_VALUE", "OS_VALUE", "STR_VALUE"] {
        let alternatives = &evaluation
            .binding(name)
            .expect("expected test binding should exist")
            .alternatives;
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(unknown),
                        ..
                    },
                    ..
                }) if unknown.cause == PythonUnknownCauseView::UnsupportedExpression
            )),
            "{name} should retain the shadowed path"
        );
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Path(_) | PythonValueKindView::Str(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} should retain the intrinsic path"
        );
    }

    let unimported = extract_project("VALUE = Path(__file__)\n", &[])
        .expect("settings extraction project should build");
    let file = settings_module_file(&unimported.0, unimported.1)
        .expect("test settings module should resolve to a source file");
    let evaluation = python_module_evaluation(&unimported.0, unimported.1, file)
        .expect("Python file should map to a module");
    let bound = only_bound(
        &evaluation
            .binding("VALUE")
            .expect("test settings module should resolve to a source file")
            .alternatives,
    )
    .expect("VALUE should have one bound alternative");
    assert!(matches!(bound.value.kind, PythonValueKindView::Unknown(_)));
}

#[test]
fn unsupported_outer_calls_do_not_contaminate_nested_path_constructors() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from pathlib import Path\nif FLAG:\n    stringify = str\nelse:\n    stringify = dynamic\nVALUE = stringify(Path(__file__))\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let value = &evaluation
        .binding("VALUE")
        .expect("expected test binding should exist")
        .alternatives;
    assert!(value.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Str(path),
                ..
            },
            ..
        }) if path == "/project/settings.py"
    )));
    assert!(value.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(_),
                ..
            },
            ..
        })
    )));
    assert!(matches!(
        evaluation
            .binding("Path")
            .expect("expected test binding should exist")
            .alternatives
            .as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Path(PythonPathView::Intrinsic(
                    PythonPathIntrinsicView::PathlibPathType
                )),
                ..
            },
            ..
        })]
    ));
}

#[test]
fn open_star_imports_can_shadow_implicit_path_names() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from pathlib import Path\nfrom dynamic import *\nTEXT = str(__file__)\nFILE = Path(__file__)\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["TEXT", "FILE"] {
        let alternatives = &evaluation
            .binding(name)
            .expect("expected test binding should exist")
            .alternatives;
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView { kind, .. },
                    ..
                }) if (name == "TEXT" && matches!(kind, PythonValueKindView::Str(_)))
                    || (name == "FILE" && matches!(kind, PythonValueKindView::Path(_)))
            )),
            "{name} should retain the intrinsic possibility: {alternatives:#?}"
        );
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} should retain star-import shadowing"
        );
    }
}

#[test]
fn exact_file_assignment_after_open_star_import_shadows_namespace_uncertainty() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from dynamic import *\nOPEN = __file__\n__file__ = '/override.py'\nAFTER = __file__\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let open = &evaluation
        .binding("OPEN")
        .expect("expected test binding should exist")
        .alternatives;
    assert!(open.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Str(path),
                ..
            },
            ..
        }) if path == "/project/settings.py"
    )));
    assert!(open.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(_),
                ..
            },
            ..
        })
    )));

    let after = &evaluation
        .binding("AFTER")
        .expect("expected test binding should exist")
        .alternatives;
    assert!(matches!(
        after.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Str(path),
                ..
            },
            ..
        })] if path == "/override.py"
    ));
}

#[test]
fn conditional_file_assignment_replaces_only_its_feasible_unbound_case() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from dynamic import *\nif FLAG:\n    __file__ = '/override.py'\nVALUE = __file__\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let alternatives = &evaluation
        .binding("VALUE")
        .expect("expected test binding should exist")
        .alternatives;

    for expected in ["/override.py", "/project/settings.py"] {
        assert!(alternatives.iter().any(|alternative| matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::Str(path),
                    ..
                },
                ..
            }) if path == expected
        )));
    }
    assert!(alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(_),
                ..
            },
            ..
        })
    )));
}

#[test]
fn path_intrinsic_attribute_writes_invalidate_aliasing_owners() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "import os\npath_alias = os.path\npath_alias.join = replacement\nimport os as fresh_os\nVALUE = os.path.join('/project', 'templates')\nFRESH_VALUE = fresh_os.path.join('/project', 'static')\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["os", "path_alias", "fresh_os", "VALUE", "FRESH_VALUE"] {
        let alternatives = &evaluation
            .binding(name)
            .expect("expected test binding should exist")
            .alternatives;
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} should be invalidated by the aliased write"
        );
        assert!(
            alternatives.iter().all(|alternative| !matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Path(_) | PythonValueKindView::Str(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} must not retain an exact path result"
        );
    }
}

#[test]
fn intrinsic_namespace_contamination_survives_reimports() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "import pathlib\nimport os\nimport builtins\npathlib.Path = custom_path\nbuiltins.str = custom_str\n(os.path if FLAG else other).join = replacement\nimport pathlib as fresh_pathlib\nimport os as fresh_os\nfrom builtins import str as fresh_str\nPATH_VALUE = fresh_pathlib.Path(__file__).parent\nOS_VALUE = fresh_os.path.join('/project', 'templates')\nSTR_VALUE = fresh_str(__file__)\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in [
        "fresh_pathlib",
        "fresh_os",
        "fresh_str",
        "PATH_VALUE",
        "OS_VALUE",
        "STR_VALUE",
    ] {
        let alternatives = &evaluation
            .binding(name)
            .expect("expected test binding should exist")
            .alternatives;
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} should remain contaminated after re-import"
        );
        assert!(
            alternatives.iter().all(|alternative| !matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Path(_) | PythonValueKindView::Str(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} must not regain exact intrinsic behavior"
        );
    }
}

#[test]
fn intrinsic_namespace_contamination_propagates_across_project_imports() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/helper.py",
        "import os\nif FLAG:\n    os.path.join = replacement\n",
    )
    .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/consumer.py",
        "import os\nVALUE = os.path.join('/project', 'templates')\n",
    )
    .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/settings.py",
        "import os\nimport helper\nimport consumer\nVALUE = os.path.join('/project', 'templates')\nIMPORTED_VALUE = consumer.VALUE\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["VALUE", "IMPORTED_VALUE"] {
        let alternatives = &evaluation
            .binding(name)
            .expect("expected test binding should exist")
            .alternatives;
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} should retain the imported contamination"
        );
        assert!(
            alternatives.iter().all(|alternative| !matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Path(_) | PythonValueKindView::Str(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} must not regain exact intrinsic behavior"
        );
    }
}

#[test]
fn unsupported_calls_persist_path_namespace_contamination() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "def mutate(path):\n    pass\nimport os\n_ = mutate(os.path)\nimport os as fresh_os\nVALUE = fresh_os.path.join('/project', 'templates')\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["fresh_os", "VALUE"] {
        let alternatives = &evaluation
            .binding(name)
            .expect("expected test binding should exist")
            .alternatives;
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} should retain unsupported-call contamination"
        );
        assert!(
            alternatives.iter().all(|alternative| !matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Path(_) | PythonValueKindView::Str(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} must not regain exact intrinsic behavior"
        );
    }
}

#[test]
fn project_modules_named_like_stdlib_path_helpers_are_not_intrinsics() {
    let db = TestDatabase::new();
    db.add_file("/project/pathlib.py", "Path = custom\n")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/settings.py",
        "from pathlib import Path\nVALUE = Path(__file__)\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let bound = only_bound(
        &evaluation
            .binding("VALUE")
            .expect("expected test binding should exist")
            .alternatives,
    )
    .expect("VALUE should have one bound alternative");
    assert!(matches!(bound.value.kind, PythonValueKindView::Unknown(_)));
}

#[test]
fn known_external_module_outcomes_do_not_depend_on_selected_members() {
    for source in [
        "from pathlib import PurePath\n",
        "from pathlib import PurePath, Path\n",
        "from pathlib import *\n",
    ] {
        let (db, project, _) =
            extract_project(source, &[]).expect("settings extraction project should build");
        let file = settings_module_file(&db, project)
            .expect("test settings module should resolve to a source file");
        let evaluation = python_module_evaluation(&db, project, file)
            .expect("Python file should map to a module");

        assert_eq!(evaluation.imports.len(), 1, "{source}");
        assert!(
            matches!(
                &evaluation.imports[0],
                PythonImportOutcomeView::SkippedExternal { module, .. }
                    if module.as_str() == "pathlib"
            ),
            "{source}: {:?}",
            evaluation.imports
        );
        if source.contains("PurePath") {
            let alternatives = &evaluation
                .binding("PurePath")
                .expect("expected JSON value should be a string")
                .alternatives;
            assert!(
                alternatives.iter().any(|alternative| matches!(
                    alternative,
                    PythonBindingAlternativeView::Bound(PythonBoundValueView {
                        value: PythonValueView {
                            kind: PythonValueKindView::Unknown(unknown),
                            ..
                        },
                        ..
                    }) if matches!(
                        &unknown.cause,
                        PythonUnknownCauseView::SkippedExternal(module)
                            if module.as_str() == "pathlib"
                    )
                )),
                "{source}"
            );
        } else {
            assert!(evaluation.namespace_open(), "{source}");
        }
    }
}

#[test]
fn partial_project_path_helper_chains_do_not_become_stdlib_intrinsics() {
    let db = TestDatabase::new();
    db.add_file("/project/os/__init__.py", "")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/settings.py",
        "import os.path as os_path\nfrom os.path import join\nDIRECT = os_path.join('/project', 'templates')\nNAMED = join('/project', 'static')\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["os_path", "join", "DIRECT", "NAMED"] {
        let binding = evaluation.binding(name).expect("binding should exist");
        assert!(
            binding.alternatives.iter().all(|alternative| !matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Path(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} must not become an intrinsic path value"
        );
    }
    assert!(
        evaluation
            .imports
            .iter()
            .any(|outcome| matches!(outcome, PythonImportOutcomeView::NotFound { .. }))
    );
}

#[test]
fn path_operations_preserve_conditionally_unbound_alternatives() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from pathlib import Path\nimport os\nBASE = Path(__file__).parent\nif FLAG:\n    SEGMENT = 'templates'\nDIVIDED = BASE / SEGMENT\nMETHOD = BASE.joinpath(SEGMENT)\nSTRING = os.path.join(str(BASE), SEGMENT)\n",
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["DIVIDED", "METHOD", "STRING"] {
        let alternatives = &evaluation
            .binding(name)
            .expect("expected test binding should exist")
            .alternatives;
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Path(_) | PythonValueKindView::Str(_),
                        ..
                    },
                    ..
                })
            )),
            "{name} should retain the bound path"
        );
        assert!(
            alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(unknown),
                        ..
                    },
                    ..
                }) if unknown.cause == PythonUnknownCauseView::UnsupportedExpression
            )),
            "{name} should retain the unbound fallthrough"
        );
    }
}

#[test]
fn closed_unsupported_literals_are_concrete_unsupported_values() {
    let source = "NONE = None\nNUMBER = 1\nNEGATIVE = -1\nBYTES = b'x'\nELLIPSIS = ...\nSET = {1}\nVALUES = [None, 1, b'x', ..., {1}]\n";
    let (db, project, _) =
        extract_project(source, &[]).expect("settings extraction project should build");
    let file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let evaluation =
        python_module_evaluation(&db, project, file).expect("Python file should map to a module");

    for name in ["NONE", "NUMBER", "NEGATIVE", "BYTES", "ELLIPSIS", "SET"] {
        let binding = evaluation
            .binding(name)
            .expect("test settings module should resolve to a source file");
        let bound = only_bound(&binding.alternatives)
            .expect("literal binding should have one bound alternative");
        assert_eq!(bound.value.kind, PythonValueKindView::UnsupportedLiteral);
    }

    let values = only_bound(
        &evaluation
            .binding("VALUES")
            .expect("expected test binding should exist")
            .alternatives,
    )
    .expect("VALUES should have one bound alternative");
    let items = list_items(&values.value.kind).expect("VALUES should remain a list");
    assert_eq!(items.len(), 5);
    assert!(items.iter().all(|item| matches!(
        item,
        PythonSequenceItemView::Value(PythonValueView {
            kind: PythonValueKindView::UnsupportedLiteral,
            ..
        })
    )));
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
fn python_binding_normalizes_nested_unknowns_and_merges_their_evidence() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "if condition:\n    VALUE = [dynamic()]\nelse:\n    VALUE = [other_dynamic()]\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let binding = evaluation.binding("VALUE").expect("VALUE should be bound");
    let bound = only_bound(&binding.alternatives)
        .expect("nested unknowns should normalize into one bound alternative");
    let items = list_items(&bound.value.kind).expect("VALUE should remain a list");
    let unknown = match only(items) {
        Some(PythonSequenceItemView::UnknownElement(unknown)) => Some(unknown),
        Some(PythonSequenceItemView::Value(_) | PythonSequenceItemView::UnknownUnpack(_))
        | None => None,
    }
    .expect("the list should contain one typed unknown element");
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
    ).expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
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
    db.add_file("/project/pkg/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/pkg/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::NotFound { origin, module }]
            if *origin == Origin::new(
                settings,
                expected_span(source, "from . import VALUE").expect("test source should contain expected text"),
            ) && module.as_str() == "pkg.VALUE"
    ));
    let binding = evaluation
        .binding("VALUE")
        .expect("failed relative import should replace the prior binding");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if matches!(&unknown.cause, PythonUnknownCauseView::MissingImportMember { module, member }
            if module.as_str() == "pkg" && member == "VALUE")
    ));
}

#[test]
fn relative_import_cannot_escape_the_importer_package() {
    let db = TestDatabase::new();
    let source = "from ..missing import VALUE\n";
    db.add_file("/project/pkg/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/pkg/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::InvalidImport { origin, reason }]
            if *origin == Origin::new(
                settings,
                expected_span(source, source.trim_end()).expect("test source should contain expected text"),
            ) && *reason == PythonImportNameErrorView::TooManyDots
    ));
}

#[test]
fn relative_import_uses_the_inbound_module_identity() {
    let db = TestDatabase::new();
    let source = "from .sibling import VALUE\n";
    db.add_file("/project/lib/pkg/settings.py", source)
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/pkg/sibling.py",
        "VALUE = 'inbound module identity'\n",
    )
    .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/lib/pkg/sibling.py",
        "VALUE = 'canonical file identity'\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/project/lib")]);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.settings").expect("test Python module name should be valid"),
    )
    .expect("pkg.settings should resolve through the nested search path");
    let sibling = db
        .file(Utf8Path::new("/project/pkg/sibling.py"))
        .expect("settings-extraction test file should exist");

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
            value: PythonValueView {
                kind: PythonValueKindView::Str(value),
                ..
            },
            ..
        })] if value == "inbound module identity"
    ));

    let canonical = python_module_evaluation(&db, project, module.file())
        .expect("resolved Python source file should map back to its module");
    let canonical_binding = canonical
        .binding("VALUE")
        .expect("canonical file identity should also be evaluated");
    assert!(matches!(
        canonical_binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
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
    db.add_file("/project/pkg/__init__.py", "from .base import VALUE\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/base.py", "VALUE = 'package value'\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.__init__").expect("test Python module name should be valid"),
    )
    .expect("pkg.__init__ should resolve as a file-module alias");

    let evaluation = python_module_evaluation_for_module(&db, project, module);
    let binding = evaluation
        .binding("VALUE")
        .expect("the package-relative import should bind VALUE");

    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
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
    let (db, project, settings) = extract_project(source, &[("plugin", "PRESENT = 'known'\n")])
        .expect("settings extraction project should build");
    let settings_file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let plugin_file = db
        .file(Utf8Path::new("/project/settings/plugin.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings_file)
        .expect("Python file should map to a module");

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
    let import_origin = Origin::new(
        settings_file,
        expected_span(source, "MISSING as INSTALLED_APPS")
            .expect("test source should contain expected text"),
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
fn python_module_evaluation_keeps_typed_import_and_namespace_outcomes() {
    let db = TestDatabase::new();
    let source = "from missing_named import VALUE\nfrom missing_star import *\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let not_found = evaluation
        .imports
        .iter()
        .map(|outcome| match outcome {
            PythonImportOutcomeView::NotFound { origin, module } => {
                Some((origin.file, origin.span, module.as_str()))
            }
            PythonImportOutcomeView::Resolved { .. }
            | PythonImportOutcomeView::InvalidImport { .. }
            | PythonImportOutcomeView::SkippedExternal { .. }
            | PythonImportOutcomeView::Unreadable { .. }
            | PythonImportOutcomeView::SyntaxErrors { .. }
            | PythonImportOutcomeView::Cycle { .. } => None,
        })
        .collect::<Option<Vec<_>>>()
        .expect("evaluation should contain only typed not-found imports");
    assert_eq!(
        not_found,
        [
            (
                settings,
                expected_span(source, "from missing_named import VALUE")
                    .expect("test source should contain expected text"),
                "missing_named"
            ),
            (
                settings,
                expected_span(source, "from missing_star import *")
                    .expect("test source should contain expected text"),
                "missing_star"
            ),
        ]
    );
    let value = evaluation.binding("VALUE").expect("VALUE should be bound");
    assert!(value.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
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
    )
    .expect("settings extraction project should build");
    let settings_file =
        settings_module_file(&db, project).expect("expected JSON value should be a string");
    let plugin_file = db
        .file(Utf8Path::new("/project/settings/plugin.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings_file)
        .expect("Python file should map to a module");
    let binding = evaluation
        .binding("INSTALLED_APPS")
        .expect("the named import should retain member and namespace uncertainty");

    assert!(
        !binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    );
    let import_origin = Origin::new(
        settings_file,
        expected_span(source, "INSTALLED_APPS").expect("test source should contain expected text"),
    );
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
    let (db, project, settings) = extract_project(
        source,
        &[(
            "plugin",
            "if ENABLED:\n    INSTALLED_APPS = ['imported']\n    from missing import *\n",
        )],
    )
    .expect("settings extraction project should build");
    let settings_file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let plugin_file = db
        .file(Utf8Path::new("/project/settings/plugin.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings_file)
        .expect("Python file should map to a module");
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
fn star_import_translates_every_typed_namespace_cause_to_the_import_site() {
    let source = "VALUE = 'local'\nfrom plugin import *\n";
    let (db, project, _) = extract_project(
        source,
        &[(
            "plugin",
            "if ENABLED:\n    from missing import *\nelse:\n    from ...invalid import *\n",
        )],
    )
    .expect("settings extraction project should build");
    let settings_file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let evaluation = python_module_evaluation(&db, project, settings_file)
        .expect("Python file should map to a module");
    let binding = evaluation
        .binding("VALUE")
        .expect("VALUE should remain bound");
    let unknowns = binding
        .alternatives
        .iter()
        .filter_map(|alternative| match alternative {
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value:
                    PythonValueView {
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
        unknown.cause
            == PythonUnknownCauseView::InvalidImport(PythonImportNameErrorView::TooManyDots)
    }));
    assert!(unknowns.iter().all(|unknown| {
        unknown.origins.as_slice()
            == [Origin::new(
                settings_file,
                expected_span(source, "from plugin import *")
                    .expect("test source should contain expected text"),
            )]
    }));
}

#[test]
fn deterministic_false_while_executes_only_else_body() {
    let source = "while False:\n    from missing_body import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['else']\n";
    let (db, project, settings) =
        extract_project(source, &[]).expect("settings extraction project should build");
    assert_eq!(
        cases(&settings, "/installed_apps/cases")
            .expect("settings JSON pointer should identify an array")[0]["known"]["apps"][0]["value"],
        "else"
    );

    let file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let evaluation =
        python_module_evaluation(&db, project, file).expect("Python file should map to a module");
    assert!(
        evaluation.imports.is_empty(),
        "the unreachable while body must contribute no import outcome"
    );
}

#[test]
fn ambiguous_while_degrades_writes_and_retains_branch_effects() {
    let source = "INSTALLED_APPS = []\nwhile FLAG:\n    INSTALLED_APPS.append('loop')\nelse:\n    from plugin import VALUE\n";
    let (db, project, settings) = extract_project(source, &[("plugin", "VALUE = '/static/'")])
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
    let file = settings_module_file(&db, project).expect("expected JSON field should be an array");
    let plugin = db
        .file(Utf8Path::new("/project/settings/plugin.py"))
        .expect("settings-extraction test file should exist");
    let evaluation =
        python_module_evaluation(&db, project, file).expect("Python file should map to a module");
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
                    == Origin::new(
                        file,
                        expected_span(source, "INSTALLED_APPS.append('loop')").expect("test source should contain expected text"),
                    )
    ));
}

#[test]
fn ambiguous_branch_annotation_only_name_is_absent() {
    let db = TestDatabase::new();
    db.add_file("/project/settings.py", "if FLAG:\n    VALUE: str\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(evaluation.binding("VALUE").is_none());
}

#[test]
fn ambiguous_branch_skips_nested_deterministically_dead_write() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "if FLAG:\n    if False:\n        VALUE = 'dead'\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(evaluation.binding("VALUE").is_none());
}

#[test]
fn ambiguous_branch_same_value_reassignment_preserves_origins() {
    let db = TestDatabase::new();
    let source = "VALUE = 'same'\nif FLAG:\n    VALUE = 'same'\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let binding = evaluation.binding("VALUE").expect("VALUE should be bound");
    let bound = only_bound(&binding.alternatives)
        .expect("the equal values should normalize into one bound alternative");
    let expected_origins = source
        .match_indices("'same'")
        .map(|(start, value)| {
            Origin::new(
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let binding = evaluation.binding("VALUE").expect("VALUE should be bound");
    let bound = only_bound(&binding.alternatives)
        .expect("the restored value should normalize into one bound alternative");
    let expected_origins = source
        .match_indices("'base'")
        .map(|(start, value)| {
            Origin::new(
                settings,
                Span::saturating_from_parts_usize(start, value.len()),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(bound.value.origins, expected_origins);
    assert_eq!(bound.binding_origins, expected_origins);
    assert!(bound.value.origins.iter().all(|origin| {
        origin.span
            != expected_span(source, "'temporary'")
                .expect("test source should contain expected text")
    }));
}

#[test]
fn ambiguous_branch_append_remove_retains_mutation_evidence() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nif FLAG:\n    VALUES.append('item')\n    VALUES.remove('item')\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let bound = only_bound(&binding.alternatives)
        .expect("the restored list should normalize into one bound alternative");
    let items = list_items(&bound.value.kind).expect("VALUES should remain a list");

    assert!(items.is_empty());
    assert_eq!(bound.value.origins.len(), 3);
    assert_eq!(evaluation.mutations.len(), 2);
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.binding == "VALUES"
            && mutation.operation == PythonMutationOperationView::Append
            && mutation.origin.span
                == expected_span(source, "VALUES.append('item')")
                    .expect("test source should contain expected text")
    }));
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.binding == "VALUES"
            && mutation.operation == PythonMutationOperationView::Remove
            && mutation.origin.span
                == expected_span(source, "VALUES.remove('item')")
                    .expect("test source should contain expected text")
    }));
}

#[test]
fn ambiguous_branch_reassignment_clears_prior_mutation_evidence() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nif FLAG:\n    VALUES.append('stale')\n    VALUES = []\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let bound = only_bound(&binding.alternatives)
        .expect("the reassigned lists should normalize into one bound alternative");

    assert!(evaluation.mutations.is_empty());
    assert_eq!(bound.value.origins.len(), 2);
    assert!(bound.value.origins.iter().all(|origin| {
        origin.span
            != expected_span(source, "VALUES.append('stale')")
                .expect("test source should contain expected text")
    }));
}

fn evaluate_module(
    source: &str,
) -> Result<(File, PythonModuleEvaluationView), Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    db.add_file("/project/settings.py", source)?;
    let project = python_project(&db);
    let file = db.file(Utf8Path::new("/project/settings.py"))?;
    let evaluation = python_module_evaluation(&db, project, file)?;
    Ok((file, evaluation))
}

fn bound_value<'a>(
    evaluation: &'a PythonModuleEvaluationView,
    name: &str,
) -> Option<&'a PythonBoundValueView> {
    evaluation
        .binding(name)
        .and_then(|binding| only_bound(&binding.alternatives))
}

fn evaluate_module_with(
    files: &[(&str, &str)],
    source: &str,
) -> Result<(File, PythonModuleEvaluationView), Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    for (path, content) in files {
        db.add_file(path, content)?;
    }
    db.add_file("/project/settings.py", source)?;
    let project = python_project(&db);
    let file = db.file(Utf8Path::new("/project/settings.py"))?;
    let evaluation = python_module_evaluation(&db, project, file)?;
    Ok((file, evaluation))
}

fn bound_module<'a>(
    evaluation: &'a PythonModuleEvaluationView,
    name: &str,
) -> Option<&'a PythonModuleView> {
    let PythonValueKindView::Module(id) = &bound_value(evaluation, name)?.value.kind else {
        return None;
    };
    Some(id)
}

fn import_module_names(evaluation: &PythonModuleEvaluationView) -> Result<Vec<String>, io::Error> {
    evaluation
        .imports
        .iter()
        .map(|outcome| match outcome {
            PythonImportOutcomeView::Resolved {
                imported_module, ..
            } => Ok(format!("resolved:{}", imported_module.as_str())),
            PythonImportOutcomeView::NotFound { module, .. } => {
                Ok(format!("notfound:{}", module.as_str()))
            }
            PythonImportOutcomeView::SkippedExternal { module, .. } => {
                Ok(format!("external:{}", module.as_str()))
            }
            PythonImportOutcomeView::Cycle {
                imported_module, ..
            } => Ok(format!("cycle:{}", imported_module.as_str())),
            other @ (PythonImportOutcomeView::InvalidImport { .. }
            | PythonImportOutcomeView::Unreadable { .. }
            | PythonImportOutcomeView::SyntaxErrors { .. }) => Err(io::Error::other(format!(
                "unexpected import outcome: {other:?}"
            ))),
        })
        .collect()
}

fn binding_strings<'a>(
    evaluation: &'a PythonModuleEvaluationView,
    name: &str,
) -> BTreeSet<&'a str> {
    evaluation
        .binding(name)
        .into_iter()
        .flat_map(|binding| &binding.alternatives)
        .filter_map(|alternative| match alternative {
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value:
                    PythonValueView {
                        kind: PythonValueKindView::Str(value),
                        ..
                    },
                ..
            }) => Some(value.as_str()),
            PythonBindingAlternativeView::Bound(_) | PythonBindingAlternativeView::Unbound => None,
        })
        .collect()
}

fn binding_has_unbound(evaluation: &PythonModuleEvaluationView, name: &str) -> bool {
    evaluation.binding(name).is_some_and(|binding| {
        binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound)
    })
}

#[test]
fn ordinary_import_binds_typed_not_found_in_source_order() {
    // Replaces the pre-Phase-3 control: unresolved ordinary imports now bind
    // typed import-not-found unknowns and record canonical not-found outcomes,
    // one per clause, in source order, without erasing prior clause effects.
    let source = concat!(
        "stale = 'old'\n",
        "import package.child\n",
        "import other as stale, third as alias\n",
    );
    let (file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

    for (name, clause, missing, target) in [
        ("package", "package.child", "package.child", "package"),
        ("stale", "other as stale", "other", "stale"),
        ("alias", "third as alias", "third", "alias"),
    ] {
        let bound = bound_value(&evaluation, name)
            .expect("imported name should have one bound alternative");
        let unknown = match &bound.value.kind {
            PythonValueKindView::Unknown(unknown) => Some(unknown),
            PythonValueKindView::Str(_)
            | PythonValueKindView::Bool(_)
            | PythonValueKindView::Path(_)
            | PythonValueKindView::UnsupportedLiteral
            | PythonValueKindView::List(_)
            | PythonValueKindView::Tuple(_)
            | PythonValueKindView::Dict(_)
            | PythonValueKindView::Module(_) => None,
        }
        .expect("imported name should bind a typed import-not-found unknown");
        assert!(
            matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == missing),
            "{name} should be ImportNotFound({missing}), got {:?}",
            unknown.cause,
        );
        // The binding origin is the exact local binding target (the root
        // segment for an unaliased dotted clause, the alias identifier for an
        // aliased clause), not the whole import clause.
        assert_eq!(
            bound.binding_origins,
            [Origin::new(
                file,
                binding_target_span(source, clause, target)
                    .expect("import clause should contain expected binding target")
            )]
        );
    }
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["notfound:package.child", "notfound:other", "notfound:third"]
    );
    // The unresolved root import never overwrites its own segment with a value
    // and the only dependency remains the evaluated file itself.
    assert_eq!(evaluation.dependency_files, [file]);
}

#[test]
fn ordinary_import_binds_source_module_and_records_edge() {
    let (file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "VALUE = 'pkg'\n")],
        "import pkg\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg"]
    );
    assert!(
        evaluation
            .dependency_files
            .contains(&Origin::new(file, Span::new(0, 0)).file)
    );
}

#[test]
fn ordinary_import_alias_binds_leaf_and_evaluates_parent() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "ROOT = 'root'\n"),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg.sub as s\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "s").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
    assert!(
        evaluation.binding("pkg").is_none(),
        "an aliased import binds only the alias"
    );
    // Both the parent package and the leaf are evaluated as new parent effects.
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.sub"]
    );
}

#[test]
fn ordinary_dotted_import_binds_root_and_attaches_loaded_child() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg.sub\nCHILD = pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    // The loaded child `sub` is attached under `pkg` and readable through the
    // module-member projection.
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.sub"]
    );
}

#[test]
fn ordinary_import_of_namespace_parent_binds_namespace_and_loads_child() {
    // `nspkg` has no __init__.py, so it is a namespace package; `nspkg.mod` is a
    // source module underneath it.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/nspkg/mod.py", "LEAF = 'leaf'\n")],
        "import nspkg.mod\nCHILD = nspkg.mod\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "nspkg").expect("binding should contain one module value"),
        &PythonModuleView::Namespace(
            PythonModuleName::parse("nspkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("nspkg.mod").expect("test Python module name should be valid")
        )
    );
    // A namespace parent produces no file/edge; only the source child does.
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:nspkg.mod"]
    );
}

#[test]
fn ordinary_import_of_external_module_binds_handle_and_skips_body() {
    let (_file, evaluation) = evaluate_module_with(
        &[(
            "/project/.venv/lib/python3.12/site-packages/ext/__init__.py",
            "VALUE = 'external'\n",
        )],
        "import ext\nATTR = ext.VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "ext").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("ext").expect("test Python module name should be valid")
        )
    );
    // Exactly one skipped-external outcome for the requested leaf and no
    // external dependency file.
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["external:ext"]
    );
    // Reading an external module's attribute never evaluates its body; it
    // surfaces the skipped-external open cause.
    let attr = evaluation.binding("ATTR").expect("ATTR should be bound");
    assert!(
        attr.alternatives.iter().any(|alternative| matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::Unknown(unknown),
                    ..
                },
                ..
            }) if matches!(&unknown.cause, PythonUnknownCauseView::SkippedExternal(module) if module.as_str() == "ext")
        )),
        "reading an external module attribute surfaces SkippedExternal: {:?}",
        attr.alternatives,
    );
    // No project value ever leaks from the external body.
    assert!(
        !attr.alternatives.iter().any(|alternative| matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::Str(_),
                    ..
                },
                ..
            })
        )),
        "the external body must not be evaluated",
    );
}

#[test]
fn ordinary_import_preserves_prior_clause_effects_on_later_failure() {
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/good/__init__.py", "")],
        "import good\nimport bad\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "good").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("good").expect("test Python module name should be valid")
        )
    );
    let bad = bound_value(&evaluation, "bad").expect("binding should have one bound alternative");
    assert!(matches!(
        &bad.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "bad")
    ));
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:good", "notfound:bad"]
    );
}

#[test]
fn ordinary_import_in_false_branch_creates_no_effects() {
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "")],
        "if False:\n    import pkg\nMARK = 'kept'\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert!(evaluation.binding("pkg").is_none());
    assert!(evaluation.imports.is_empty());
}

#[test]
fn ordinary_import_two_file_cycle_binds_handle_and_records_cycle_edge() {
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "import b\nVALUE = 'a'\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/b.py", "import a\nVALUE = 'b'\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("a").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    // The direct import binds the loaded module handle even across the cycle,
    // and the local value survives.
    assert_eq!(
        bound_module(&evaluation, "b").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("b").expect("test Python module name should be valid")
        )
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(&value.value.kind, PythonValueKindView::Str(text) if text == "a"));
    assert!(
        evaluation
            .imports
            .iter()
            .any(|outcome| matches!(outcome, PythonImportOutcomeView::Cycle { .. })),
        "a two-file direct-import cycle records a cycle edge",
    );
}

#[test]
fn ordinary_dotted_import_of_package_leaf_attaches_package_child() {
    // The leaf `pkg.sub` is itself a regular package (`sub/__init__.py`), so a
    // package-init leaf attaches under its parent exactly like a file-module
    // leaf.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/sub/__init__.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg.sub\nCHILD = pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.sub"]
    );
}

#[test]
fn ordinary_import_of_namespace_terminal_binds_namespace_without_edges() {
    // `ns` has no `__init__.py` and no requested child, so it is a namespace
    // terminal: it binds a namespace handle and records no file or edge.
    let (_file, evaluation) =
        evaluate_module_with(&[("/project/ns/placeholder.py", "")], "import ns\n")
            .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "ns").expect("binding should contain one module value"),
        &PythonModuleView::Namespace(
            PythonModuleName::parse("ns").expect("test Python module name should be valid")
        )
    );
    assert!(
        evaluation.imports.is_empty(),
        "a namespace terminal produces no file/edge",
    );
}

#[test]
fn ordinary_dotted_import_missing_leaf_preserves_parent_edge() {
    // The parent package `pkg` resolves and evaluates; the missing leaf is a
    // typed not-found whose failure binds the root as unknown, but the parent's
    // edge and dependency survive.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "VALUE = 'pkg'\n")],
        "import pkg.missing\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "notfound:pkg.missing"]
    );
    let bound = bound_value(&evaluation, "pkg").expect("binding should have one bound alternative");
    assert!(matches!(
        &bound.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "pkg.missing")
    ));
}

#[test]
fn ordinary_dotted_import_missing_intermediate_preserves_root_edge() {
    // A missing intermediate component stops resolution at the missing name; the
    // resolved root prefix `pkg` still records its edge before the not-found.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "")],
        "import pkg.missing.leaf\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "notfound:pkg.missing.leaf"]
    );
}

#[test]
fn ordinary_dotted_import_recovers_parent_syntax_and_continues() {
    // The parent `__init__.py` has recoverable syntax errors: its edge carries a
    // syntax status, the parent handle is still available, and the dotted chain
    // continues to the leaf.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "VALUE = 'ok'\nbroken(\n"),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg.sub\nCHILD = pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::SyntaxErrors { imported_module, .. }
                if imported_module.as_str() == "pkg"
        )),
        "the parent package records a recovered syntax outcome: {:?}",
        evaluation.imports,
    );
    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Resolved { imported_module, .. }
                if imported_module.as_str() == "pkg.sub"
        )),
        "the leaf still resolves after recovering the parent: {:?}",
        evaluation.imports,
    );
    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
}

#[test]
fn member_read_of_unimported_child_is_module_attribute_unknown() {
    // A submodule that was never imported is not attached to its package, so a
    // member read observes non-attachment as a typed module-attribute unknown
    // rather than the child's value.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg\nCHILD = pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    let child =
        bound_value(&evaluation, "CHILD").expect("binding should have one bound alternative");
    assert!(
        matches!(
            &child.value.kind,
            PythonValueKindView::Unknown(unknown)
                if matches!(&unknown.cause, PythonUnknownCauseView::ModuleAttribute { module, member }
                    if module.as_str() == "pkg" && member == "sub")
        ),
        "an unimported child is not attached; the read is a module-attribute unknown, got {:?}",
        child.value.kind,
    );
}

#[test]
fn ordinary_dotted_import_records_root_to_leaf_order_under_reverse_registration() {
    // Files registered leaf-before-root still produce edges/dependencies in
    // first-seen root-to-leaf traversal order.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
        ],
        "import pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    let resolved: Vec<(String, String)> = evaluation
        .imports
        .iter()
        .map(|outcome| match outcome {
            PythonImportOutcomeView::Resolved {
                importer_module,
                imported_module,
                ..
            } => Some((
                importer_module.as_str().to_string(),
                imported_module.as_str().to_string(),
            )),
            PythonImportOutcomeView::InvalidImport { .. }
            | PythonImportOutcomeView::NotFound { .. }
            | PythonImportOutcomeView::SkippedExternal { .. }
            | PythonImportOutcomeView::Unreadable { .. }
            | PythonImportOutcomeView::SyntaxErrors { .. }
            | PythonImportOutcomeView::Cycle { .. } => None,
        })
        .collect::<Option<_>>()
        .expect("evaluation should contain only resolved imports");
    assert_eq!(
        resolved,
        vec![
            ("settings".to_string(), "pkg".to_string()),
            ("settings".to_string(), "pkg.sub".to_string()),
        ],
    );
}

#[test]
fn ambiguous_dotted_import_preserves_module_and_unbound_alternatives() {
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "")],
        "if condition:\n    import pkg\nMARK = 'kept'\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    let binding = evaluation.binding("pkg").expect("pkg should be tracked");
    assert!(
        binding.alternatives.iter().any(|alternative| matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::Module(PythonModuleView::Source(module)),
                    ..
                },
                ..
            }) if module.as_str() == "pkg"
        )),
        "the taken branch binds the module handle: {:?}",
        binding.alternatives,
    );
    assert!(
        binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound),
        "the skipped branch leaves pkg unbound: {:?}",
        binding.alternatives,
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg"]
    );
}

#[test]
fn deterministically_true_dotted_import_binds_module_unconditionally() {
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "")],
        "if True:\n    import pkg\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg"]
    );
}

#[test]
fn named_from_import_loads_exact_package_child_with_alias_origins() {
    let source = "from pkg import child as alias\n";
    let (file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/child.py", "VALUE = 'child'\n"),
        ],
        source,
    )
    .expect("multi-file Python evaluation fixture should build");
    assert_eq!(
        bound_module(&evaluation, "alias").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.child").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.child"]
    );
    let alias_origin = Origin::new(
        file,
        expected_span(source, "child as alias").expect("test source should contain expected text"),
    );
    assert_eq!(
        bound_value(&evaluation, "alias")
            .expect("binding should have one bound alternative")
            .binding_origins,
        [alias_origin]
    );
    let statement_origin = Origin::new(
        file,
        expected_span(source, "from pkg import child as alias")
            .expect("test source should contain expected text"),
    );
    assert!(matches!(
        &evaluation.imports[1],
        PythonImportOutcomeView::Resolved { origin, imported_module, .. }
            if *origin == statement_origin && imported_module.as_str() == "pkg.child"
    ));
    assert_eq!(evaluation.dependency_files.len(), 3);
}

#[test]
fn named_from_import_existing_private_member_shadows_child() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "_child = 'member'\n"),
            ("/project/pkg/_child.py", "VALUE = 'child'\n"),
        ],
        "from pkg import _child\n",
    )
    .expect("multi-file Python evaluation fixture should build");
    assert!(
        matches!(&bound_value(&evaluation, "_child").expect("binding should have one bound alternative").value.kind, PythonValueKindView::Str(value) if value == "member")
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg"]
    );
}

#[test]
fn named_from_import_conditional_member_attaches_child_only_when_absent() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "if FLAG:\n    child = 'member'\n",
            ),
            ("/project/pkg/child.py", "import pkg.side\n"),
            ("/project/pkg/side.py", ""),
        ],
        "from pkg import child\nimport pkg\nATTR = pkg.child\nSIDE = pkg.side\n",
    )
    .expect("multi-file Python evaluation fixture should build");
    assert_eq!(
        &import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes")[..2],
        ["resolved:pkg", "resolved:pkg.child"],
    );
    assert_eq!(evaluation.dependency_files.len(), 4);
    for name in ["child", "ATTR"] {
        let binding = evaluation
            .binding(name)
            .expect("expected test binding should exist");
        assert!(binding.alternatives.iter().any(|alternative| matches!(alternative, PythonBindingAlternativeView::Bound(PythonBoundValueView { value: PythonValueView { kind: PythonValueKindView::Str(value), .. }, .. }) if value == "member")));
        assert!(binding.alternatives.iter().any(|alternative| matches!(alternative, PythonBindingAlternativeView::Bound(PythonBoundValueView { value: PythonValueView { kind: PythonValueKindView::Module(PythonModuleView::Source(module)), .. }, .. }) if module.as_str() == "pkg.child")));
    }
    let side = evaluation.binding("SIDE").expect("SIDE should be bound");
    assert!(side.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Module(PythonModuleView::Source(module)),
                ..
            },
            ..
        }) if module.as_str() == "pkg.side"
    )));
    assert!(side.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::ModuleAttribute { module, member }
            if module.as_str() == "pkg" && member == "side")
    )));
}

#[test]
fn named_child_fallback_preserves_member_mutation_namespace_and_syntax_evidence() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "if FLAG:\n    child = []\n    child.append('member')\nif BROKEN:\n    from clean import *\n    broken(]\nfrom missing import *\n",
            ),
            ("/project/pkg/child.py", ""),
            ("/project/clean.py", ""),
        ],
        "from pkg import child\n",
    ).expect("multi-file Python evaluation fixture should build");

    let child = evaluation
        .binding("child")
        .expect("expected test binding should exist");
    assert!(child.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::List(_),
                ..
            },
            ..
        })
    )));
    assert!(child.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Module(PythonModuleView::Source(module)),
                ..
            },
            ..
        }) if module.as_str() == "pkg.child"
    )));
    assert!(child.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module)
            if module.as_str() == "missing")
    )));
    assert!(child.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(unknown.cause, PythonUnknownCauseView::SyntaxErrors(_))
    )));
    assert!(
        evaluation
            .mutations
            .iter()
            .any(|mutation| mutation.binding == "child")
    );
    let package_index = evaluation
        .imports
        .iter()
        .position(|outcome| {
            matches!(
                outcome,
                PythonImportOutcomeView::SyntaxErrors { imported_module, .. }
                    if imported_module.as_str() == "pkg"
            )
        })
        .expect("the recovered package edge should be recorded");
    let child_index = evaluation
        .imports
        .iter()
        .position(|outcome| {
            matches!(
                outcome,
                PythonImportOutcomeView::Resolved { imported_module, .. }
                    if imported_module.as_str() == "pkg.child"
            )
        })
        .expect("the fallback child edge should be recorded");
    assert!(package_index < child_index);
    assert_eq!(evaluation.dependency_files.len(), 4);
}

#[test]
fn named_from_import_loads_namespace_child_and_types_missing_child() {
    let (_file, success) = evaluate_module_with(
        &[("/project/ns/child.py", "")],
        "from ns import child\nimport ns as parent\nATTR = parent.child\n",
    )
    .expect("multi-file Python evaluation fixture should build");
    assert_eq!(
        bound_module(&success, "child").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("ns.child").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        import_module_names(&success).expect("import outcomes should have supported test shapes"),
        ["resolved:ns.child"]
    );
    assert_eq!(success.dependency_files.len(), 2);
    assert_eq!(
        bound_module(&success, "ATTR").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("ns.child").expect("test Python module name should be valid")
        )
    );

    let (_file, missing) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "")],
        "from pkg import absent\nimport pkg\nATTR = pkg.absent\n",
    )
    .expect("multi-file Python evaluation fixture should build");
    assert_eq!(
        import_module_names(&missing).expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "notfound:pkg.absent", "resolved:pkg"]
    );
    assert_eq!(missing.dependency_files.len(), 2);
    assert!(
        matches!(&bound_value(&missing, "absent").expect("binding should have one bound alternative").value.kind, PythonValueKindView::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCauseView::MissingImportMember { module, member } if module.as_str() == "pkg" && member == "absent"))
    );
    assert!(matches!(
        &bound_value(&missing, "ATTR").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ModuleAttribute { module, member }
                if module.as_str() == "pkg" && member == "absent")
    ));
}

#[test]
fn named_from_import_external_package_child_keeps_identity_open_without_dependencies() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/.venv/lib/python3.12/site-packages/external/__init__.py",
                "PARENT = 'not evaluated'\n",
            ),
            (
                "/project/.venv/lib/python3.12/site-packages/external/child.py",
                "VALUE = 'not evaluated'\n",
            ),
        ],
        "from external import child\nimport external as parent\nATTR = child.VALUE\nATTACHED = parent.child\n",
    ).expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        &import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes")[..2],
        ["external:external", "external:external.child"]
    );
    assert_eq!(
        evaluation.dependency_files.len(),
        1,
        "external source files must not become dependencies",
    );
    let child = evaluation.binding("child").expect("child should be bound");
    assert!(child.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Module(PythonModuleView::Source(module)),
                ..
            },
            ..
        }) if module.as_str() == "external.child"
    )));
    let attached = evaluation
        .binding("ATTACHED")
        .expect("the loaded child should remain attached to its parent");
    assert!(attached.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Module(PythonModuleView::Source(module)),
                ..
            },
            ..
        }) if module.as_str() == "external.child"
    )));
    assert!(!attached.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(unknown.cause, PythonUnknownCauseView::ModuleAttribute { .. })
    )));

    let attribute = evaluation.binding("ATTR").expect("ATTR should be bound");
    assert!(attribute.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::SkippedExternal(module)
            if module.as_str() == "external.child")
    )));
}

#[test]
fn named_from_import_unreadable_child_records_edge_without_attachment() {
    let (db, settings, evaluation) = evaluate_unreadable_module(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/child.py", "VALUE = 'hidden'\n"),
        ],
        "/project/pkg/child.py",
        "from pkg import child\nimport pkg\nATTR = pkg.child\n",
    )
    .expect("unreadable-module fixture should build");
    let package = path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
        .expect("test path should map to a source file");
    let child_file = path_to_file(&db, Utf8Path::new("/project/pkg/child.py"))
        .expect("test path should map to a source file");

    assert!(matches!(
        &evaluation.imports[0],
        PythonImportOutcomeView::Resolved { imported_module, .. }
            if imported_module.as_str() == "pkg"
    ));
    assert!(matches!(
        &evaluation.imports[1],
        PythonImportOutcomeView::Unreadable { imported_module, .. }
            if imported_module.as_str() == "pkg.child"
    ));
    assert_eq!(
        &evaluation.dependency_files[..3],
        [settings, package, child_file],
    );
    assert!(matches!(
        &bound_value(&evaluation, "child").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(unknown.cause, PythonUnknownCauseView::Unreadable(_))
    ));
    assert!(matches!(
        &bound_value(&evaluation, "ATTR").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ModuleAttribute { module, member }
                if module.as_str() == "pkg" && member == "child")
    ));
}

#[test]
fn named_from_import_child_cycle_records_recovery_and_binds_handle() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            (
                "/project/pkg/child.py",
                "import settings\nVALUE = 'partial'\n",
            ),
        ],
        "from pkg import child\nATTR = child.VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["cycle:settings", "resolved:pkg", "resolved:pkg.child"]
    );
    assert_eq!(evaluation.dependency_files.len(), 3);
    assert_eq!(
        bound_module(&evaluation, "child").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.child").expect("test Python module name should be valid")
        )
    );
    assert!(matches!(
        &bound_value(&evaluation, "ATTR").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Str(value) if value == "partial"
    ));
}

#[test]
fn named_from_import_cycle_seed_parent_still_loads_exact_child() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/pkg/__init__.py",
        "from . import child\nATTR = child.VALUE\n",
    )
    .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/child.py", "VALUE = 'child'\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let package = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let package_file = package.file();
    let child_file = db
        .file(Utf8Path::new("/project/pkg/child.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation_for_module(&db, project, package);

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["cycle:pkg", "resolved:pkg.child"]
    );
    assert_eq!(evaluation.dependency_files, [package_file, child_file]);
    let child = evaluation.binding("child").expect("child should be bound");
    assert!(child.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(unknown.cause, PythonUnknownCauseView::Cycle)
    )));
}

#[test]
fn from_dotted_import_named_records_parent_effects_and_selects_member() {
    // A dotted `from pkg.sub import CHILD` loads the side-effecting parent
    // `pkg/__init__.py` (new parent effect) and the leaf `pkg/sub.py`, then
    // selects the named member unchanged.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/sub.py", "CHILD = 'child'\n"),
        ],
        "from pkg.sub import CHILD\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.sub"]
    );
    let child =
        bound_value(&evaluation, "CHILD").expect("binding should have one bound alternative");
    assert!(matches!(&child.value.kind, PythonValueKindView::Str(text) if text == "child"));
}

#[test]
fn from_dotted_import_star_records_parent_effects_and_binds_members() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/sub.py", "CHILD = 'child'\n"),
        ],
        "from pkg.sub import *\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.sub"]
    );
    let child =
        bound_value(&evaluation, "CHILD").expect("binding should have one bound alternative");
    assert!(matches!(&child.value.kind, PythonValueKindView::Str(text) if text == "child"));
    // The parent's own name is not pulled in by a star import of the leaf.
    assert!(evaluation.binding("PARENT").is_none());
}

#[test]
fn star_import_without_all_selects_only_intrinsic_public_names_and_never_scans_children() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "PUBLIC = 'public'\n_PRIVATE = 'private'\n",
            ),
            ("/project/pkg/child.py", "VALUE = 'child'\n"),
        ],
        "PUBLIC = 'stale public'\n_PRIVATE = 'stale private'\nfrom pkg import *\nimport pkg\nCHILD = pkg.child\n",
    ).expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        binding_strings(&evaluation, "PUBLIC"),
        ["public"].into_iter().collect()
    );
    assert_eq!(
        binding_strings(&evaluation, "_PRIVATE"),
        ["stale private"].into_iter().collect()
    );
    assert!(matches!(
        &bound_value(&evaluation, "CHILD").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ModuleAttribute { module, member }
                if module.as_str() == "pkg" && member == "child")
    ));
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg"]
    );
}

#[test]
fn exact_all_selects_private_names_in_order_dedupes_and_loads_listed_children() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "__all__ = ['_PRIVATE', 'second', 'first', 'second']\n_PRIVATE = 'private'\nUNLISTED = 'hidden'\n",
            ),
            ("/project/pkg/first.py", "VALUE = 'first'\n"),
            (
                "/project/pkg/second/__init__.py",
                "VALUE = 'second package'\n",
            ),
            ("/project/pkg/unlisted.py", "VALUE = 'unlisted'\n"),
        ],
        "from pkg import *\n",
    ).expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        binding_strings(&evaluation, "_PRIVATE"),
        ["private"].into_iter().collect()
    );
    assert_eq!(
        bound_module(&evaluation, "second").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.second").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "first").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.first").expect("test Python module name should be valid")
        )
    );
    assert!(evaluation.binding("UNLISTED").is_none());
    assert!(evaluation.binding("unlisted").is_none());
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.second", "resolved:pkg.first"]
    );
}

#[test]
fn exact_all_non_child_spellings_stay_missing_without_descendant_lookup() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "__all__ = ['bad-name', 'child.grandchild']\n",
            ),
            ("/project/pkg/child/__init__.py", ""),
            ("/project/pkg/child/grandchild.py", ""),
        ],
        "from pkg import *\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg"]
    );
    for member in ["bad-name", "child.grandchild"] {
        assert!(matches!(
            &bound_value(&evaluation, member).expect("binding should have one bound alternative").value.kind,
            PythonValueKindView::Unknown(unknown)
                if matches!(&unknown.cause, PythonUnknownCauseView::MissingImportMember {
                    module,
                    member: missing,
                } if module.as_str() == "pkg" && missing == member)
        ));
    }
}

#[test]
fn exact_tuple_all_selects_only_its_named_member() {
    let (_file, evaluation) = evaluate_module_with(
        &[(
            "/project/plugin.py",
            "__all__ = ('SELECTED',)\nSELECTED = 'selected'\nOMITTED = 'omitted'\n",
        )],
        "from plugin import *\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        binding_strings(&evaluation, "SELECTED"),
        ["selected"].into_iter().collect()
    );
    assert!(evaluation.binding("OMITTED").is_none());
}

#[test]
fn exact_all_branch_alternatives_keep_common_exports_exact_and_omitted_stale_values() {
    let (_file, evaluation) = evaluate_module_with(
        &[(
            "/project/plugin.py",
            "COMMON = 'imported common'\nLEFT = 'imported left'\nRIGHT = 'imported right'\nif FLAG:\n    __all__ = ['COMMON', 'LEFT']\nelse:\n    __all__ = ['COMMON', 'RIGHT']\n",
        )],
        "COMMON = 'stale common'\nLEFT = 'stale left'\nRIGHT = 'stale right'\nUNLISTED = 'stale unlisted'\nfrom plugin import *\n",
    ).expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        binding_strings(&evaluation, "COMMON"),
        ["imported common"].into_iter().collect()
    );
    assert_eq!(
        binding_strings(&evaluation, "LEFT"),
        ["imported left", "stale left"].into_iter().collect()
    );
    assert_eq!(
        binding_strings(&evaluation, "RIGHT"),
        ["imported right", "stale right"].into_iter().collect()
    );
    assert_eq!(
        binding_strings(&evaluation, "UNLISTED"),
        ["stale unlisted"].into_iter().collect()
    );
    assert!(!binding_has_unbound(&evaluation, "COMMON"));
}

#[test]
fn exact_all_conditional_member_preserves_prior_binding_and_mutation_path() {
    let plugin = "if FLAG:\n    VALUE = []\n    VALUE.append('imported')\n__all__ = ['VALUE']\n";
    let source = "VALUE = []\nVALUE.append('local')\nfrom plugin import *\n";
    let (file, evaluation) = evaluate_module_with(&[("/project/plugin.py", plugin)], source)
        .expect("multi-file Python evaluation fixture should build");

    let binding = evaluation
        .binding("VALUE")
        .expect("expected test binding should exist");
    assert!(
        binding.alternatives.iter().all(|alternative| matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::List(_),
                    ..
                },
                ..
            })
        )),
        "exact selection must use the caller value when the imported member is unbound: {binding:#?}",
    );
    assert_eq!(evaluation.mutations.len(), 2, "{evaluation:#?}");
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.origin.file == file
            && mutation.origin.span
                == expected_span(source, "VALUE.append('local')")
                    .expect("test source should contain expected text")
    }));
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.origin.file != file
            && mutation.origin.span
                == expected_span(plugin, "VALUE.append('imported')")
                    .expect("test source should contain expected text")
    }));
}

#[test]
fn conditional_exact_and_absent_all_keep_selection_and_mutations_correlated() {
    let plugin = "VALUE = []\nVALUE.append('imported')\nif FLAG:\n    __all__ = []\n";
    let source = "VALUE = []\nVALUE.append('local')\nfrom plugin import *\n";
    let (file, evaluation) = evaluate_module_with(&[("/project/plugin.py", plugin)], source)
        .expect("multi-file Python evaluation fixture should build");

    let binding = evaluation
        .binding("VALUE")
        .expect("expected test binding should exist");
    assert_eq!(
        binding
            .alternatives
            .iter()
            .filter(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::List(_),
                        ..
                    },
                    ..
                })
            ))
            .count(),
        2,
        "the absent path imports VALUE while the exact-empty path preserves the caller: {binding:#?}",
    );
    assert_eq!(evaluation.mutations.len(), 2, "{evaluation:#?}");
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.origin.file == file
            && mutation.origin.span
                == expected_span(source, "VALUE.append('local')")
                    .expect("test source should contain expected text")
    }));
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.origin.file != file
            && mutation.origin.span
                == expected_span(plugin, "VALUE.append('imported')")
                    .expect("test source should contain expected text")
    }));
}

#[test]
fn exact_all_cycle_member_preserves_caller_binding_and_mutation_path() {
    let source = "VALUE = []\nVALUE.append('local')\nfrom a import *\n";
    let (file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/a.py",
                "from b import VALUE\n__all__ = ['VALUE']\n",
            ),
            ("/project/b.py", "from a import VALUE\n"),
        ],
        source,
    )
    .expect("multi-file Python evaluation fixture should build");

    let binding = evaluation
        .binding("VALUE")
        .expect("expected test binding should exist");
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::List(_),
                ..
            },
            ..
        })
    )));
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if unknown.cause == PythonUnknownCauseView::Cycle
    )));
    assert!(evaluation.mutations.iter().any(|mutation| {
        mutation.origin.file == file
            && mutation.origin.span
                == expected_span(source, "VALUE.append('local')")
                    .expect("test source should contain expected text")
    }));
}

#[test]
fn exact_all_copies_only_mutations_from_selected_branch_paths() {
    let plugin = "if FLAG:\n    VALUE = []\n    VALUE.append('same')\n    __all__ = ['VALUE']\nelse:\n    VALUE = []\n    VALUE.append('same')\n    __all__ = []\n";
    let (_file, evaluation) =
        evaluate_module_with(&[("/project/plugin.py", plugin)], "from plugin import *\n")
            .expect("multi-file Python evaluation fixture should build");

    assert_eq!(evaluation.mutations.len(), 1, "{evaluation:#?}");
    assert_eq!(
        evaluation.mutations[0].origin.span,
        expected_span(plugin, "VALUE.append('same')")
            .expect("test source should contain expected text"),
    );
}

#[test]
fn exact_all_branch_child_effects_follow_arm_and_list_source_order() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "if FLAG:\n    __all__ = ['z_child', 'm_child']\nelse:\n    __all__ = ['a_child']\n",
            ),
            ("/project/pkg/z_child.py", ""),
            ("/project/pkg/m_child.py", ""),
            ("/project/pkg/a_child.py", ""),
        ],
        "from pkg import *\n",
    ).expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        [
            "resolved:pkg",
            "resolved:pkg.z_child",
            "resolved:pkg.m_child",
            "resolved:pkg.a_child",
        ]
    );
}

#[test]
fn exact_all_opposite_branch_orders_load_once_in_deterministic_arm_order() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "if FLAG:\n    __all__ = ['z_child', 'a_child']\nelse:\n    __all__ = ['a_child', 'z_child']\n",
            ),
            ("/project/pkg/z_child.py", ""),
            ("/project/pkg/a_child.py", ""),
        ],
        "from pkg import *\n",
    ).expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        [
            "resolved:pkg",
            "resolved:pkg.z_child",
            "resolved:pkg.a_child"
        ],
    );
}

#[test]
fn exact_all_attaches_selected_child_and_its_effects_only_on_selected_paths() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            (
                "/project/pkg/__init__.py",
                "if FLAG:\n    __all__ = ['child']\nelse:\n    __all__ = []\n",
            ),
            ("/project/pkg/child.py", "import pkg.side\n"),
            ("/project/pkg/side.py", ""),
        ],
        "from pkg import *\nimport pkg\nCHILD = pkg.child\nSIDE = pkg.side\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    for (name, module) in [
        ("child", "pkg.child"),
        ("CHILD", "pkg.child"),
        ("SIDE", "pkg.side"),
    ] {
        let binding = evaluation
            .binding(name)
            .expect("selected path should be represented");
        assert!(
            binding.alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Module(PythonModuleView::Source(found)),
                        ..
                    },
                    ..
                }) if found.as_str() == module
            )),
            "{name}: {binding:#?}"
        );
    }
    assert!(binding_has_unbound(&evaluation, "child"));
    for name in ["CHILD", "SIDE"] {
        assert!(
            evaluation
                .binding(name)
                .expect("expected JSON value should be a string")
                .alternatives
                .iter()
                .any(|alternative| matches!(
                    alternative,
                    PythonBindingAlternativeView::Bound(PythonBoundValueView {
                        value: PythonValueView { kind: PythonValueKindView::Unknown(unknown), .. },
                        ..
                    }) if matches!(unknown.cause, PythonUnknownCauseView::ModuleAttribute { .. })
                )),
            "{name} should preserve the omitted-path coordinate"
        );
    }
}

#[test]
fn selected_and_dynamic_star_paths_preserve_an_existing_child_coordinate() {
    for all in [
        "if FLAG:\n    __all__ = ['child']\nelse:\n    __all__ = []\n",
        "__all__ = NAMES\n",
    ] {
        let (_file, evaluation) = evaluate_module_with(
            &[
                ("/project/pkg/__init__.py", all),
                ("/project/pkg/child.py", ""),
            ],
            "import pkg.child\nfrom pkg import *\nATTR = pkg.child\n",
        )
        .expect("multi-file Python evaluation fixture should build");

        let attr = evaluation
            .binding("ATTR")
            .expect("ATTR should be represented");
        assert!(
            attr.alternatives.iter().any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Module(PythonModuleView::Source(module)),
                        ..
                    },
                    ..
                }) if module.as_str() == "pkg.child"
            )),
            "{all}: {evaluation:#?}"
        );
        assert!(
            attr.alternatives.iter().all(|alternative| !matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(unknown),
                        ..
                    },
                    ..
                }) if matches!(unknown.cause, PythonUnknownCauseView::ModuleAttribute { .. })
            )),
            "the pre-existing child coordinate must survive: {all}: {evaluation:#?}"
        );
    }
}

#[test]
fn dynamic_all_shapes_preserve_imported_and_prior_possibilities_and_open_namespace() {
    for all in [
        "['VALUE', dynamic()]",
        "['VALUE', *EXTRA]",
        "'VALUE'",
        "True",
    ] {
        let source = "VALUE = 'local'\nfrom plugin import *\n";
        let plugin = format!("VALUE = 'imported'\n__all__ = {all}\n");
        let (file, evaluation) =
            evaluate_module_with(&[("/project/plugin.py", plugin.as_str())], source)
                .expect("multi-file Python evaluation fixture should build");

        assert_eq!(
            binding_strings(&evaluation, "VALUE"),
            ["imported", "local"].into_iter().collect(),
            "{all}: {evaluation:#?}"
        );
        assert!(
            evaluation
                .binding("VALUE")
                .expect("expected JSON value should be a string")
                .alternatives
                .iter()
                .any(|alternative| matches!(
                    alternative,
                    PythonBindingAlternativeView::Bound(PythonBoundValueView {
                        value: PythonValueView {
                            kind: PythonValueKindView::Unknown(_),
                            ..
                        },
                        ..
                    })
                )),
            "{all}: dynamic selection must not guarantee an exact export"
        );
        assert!(
            matches!(
                evaluation.namespace_unknowns.as_slice(),
                [unknown]
                    if unknown.cause == PythonUnknownCauseView::UnsupportedExpression
                        && unknown.origins.as_slice() == [Origin::new(
                            file,
                            expected_span(source, "from plugin import *").expect("test source should contain expected text"),
                        )]
            ),
            "{all}: {evaluation:#?}"
        );
    }
}

#[test]
fn attached_all_member_is_classified_through_the_shared_projection() {
    let source = "PUBLIC = 'prior'\nimport pkg.__all__\nfrom pkg import *\n";
    let (file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PUBLIC = 'imported'\n"),
            ("/project/pkg/__all__.py", ""),
        ],
        source,
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        binding_strings(&evaluation, "PUBLIC"),
        ["imported", "prior"].into_iter().collect(),
    );
    let public = evaluation
        .binding("PUBLIC")
        .expect("expected test binding should exist");
    assert!(public.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        }) if matches!(unknown.cause, PythonUnknownCauseView::UnsupportedExpression)
    )));
    assert!(matches!(
        evaluation.namespace_unknowns.as_slice(),
        [unknown]
            if matches!(unknown.cause, PythonUnknownCauseView::UnsupportedExpression)
                && unknown.origins.as_slice()
                    == [Origin::new(file, expected_span(source, "from pkg import *").expect("test source should contain expected text"))]
    ));
}

#[test]
fn dynamic_all_does_not_scan_a_listable_package_child() {
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "__all__ = NAMES\n"),
            ("/project/pkg/child.py", "VALUE = 'child'\n"),
        ],
        "from pkg import *\nimport pkg\nCHILD = pkg.child\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg"]
    );
    assert!(matches!(
        &bound_value(&evaluation, "CHILD").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(unknown.cause, PythonUnknownCauseView::ModuleAttribute { .. })
    ));
    assert!(evaluation.namespace_open());
}

#[test]
fn exact_all_closes_an_otherwise_open_source_namespace() {
    let source = "VALUE = 'local'\nfrom plugin import *\n";
    let (_file, evaluation) = evaluate_module_with(
        &[(
            "/project/plugin.py",
            "VALUE = 'imported'\nfrom missing import *\n__all__ = ['VALUE']\n",
        )],
        source,
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        binding_strings(&evaluation, "VALUE"),
        ["imported"].into_iter().collect()
    );
    assert!(!evaluation.namespace_open(), "{evaluation:#?}");
    assert!(!binding_has_unbound(&evaluation, "VALUE"));
}

#[test]
fn syntax_uncertainty_in_all_is_dynamic_and_rebased_to_the_star_site() {
    let source = "VALUE = 'local'\nfrom plugin import *\n";
    let (file, evaluation) = evaluate_module_with(
        &[(
            "/project/plugin.py",
            "VALUE = 'imported'\n__all__ = [\n    'VALUE',\n    @\n]\n",
        )],
        source,
    )
    .expect("multi-file Python evaluation fixture should build");

    assert!(binding_strings(&evaluation, "VALUE").contains("local"));
    assert!(
        evaluation.namespace_unknowns.iter().any(|unknown| {
            matches!(unknown.cause, PythonUnknownCauseView::SyntaxErrors(_))
                && unknown.origins.as_slice()
                    == [Origin::new(
                        file,
                        expected_span(source, "from plugin import *")
                            .expect("test source should contain expected text"),
                    )]
        }),
        "{evaluation:#?}"
    );
}

#[test]
fn cycle_uncertainty_in_all_preserves_stale_bindings_and_opens_the_namespace() {
    let source = "VALUE = 'local'\nfrom a import *\n";
    let (db, project, _) = extract_project(
        source,
        &[
            (
                "a",
                "VALUE = 'imported'\n__all__ = ['VALUE']\nfrom b import *\n",
            ),
            ("b", "from a import *\n"),
        ],
    )
    .expect("settings extraction project should build");
    let file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let evaluation =
        python_module_evaluation(&db, project, file).expect("Python file should map to a module");

    assert_eq!(
        binding_strings(&evaluation, "VALUE"),
        ["local"].into_iter().collect()
    );
    assert!(
        evaluation.namespace_unknowns.iter().any(|unknown| {
            unknown.cause == PythonUnknownCauseView::Cycle
                && unknown.origins.as_slice()
                    == [Origin::new(
                        file,
                        expected_span(source, "from a import *")
                            .expect("test source should contain expected text"),
                    )]
        }),
        "{evaluation:#?}"
    );
}

#[test]
fn unreadable_star_source_preserves_stale_bindings_and_opens_the_namespace() {
    let source = "VALUE = 'local'\nfrom unreadable import *\n";
    let (_db, file, evaluation) = evaluate_unreadable_module(
        &[("/project/unreadable.py", "VALUE = 'hidden'\n")],
        "/project/unreadable.py",
        source,
    )
    .expect("unreadable-module fixture should build");

    assert_eq!(
        binding_strings(&evaluation, "VALUE"),
        ["local"].into_iter().collect()
    );
    assert!(
        evaluation
            .binding("VALUE")
            .expect("expected test binding should exist")
            .alternatives
            .iter()
            .any(|alternative| matches!(
                alternative,
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: PythonValueView {
                        kind: PythonValueKindView::Unknown(unknown),
                        ..
                    },
                    ..
                }) if matches!(unknown.cause, PythonUnknownCauseView::Unreadable(_))
                    && unknown.origins.as_slice()
                        == [Origin::new(file, expected_span(source, "from unreadable import *").expect("test source should contain expected text"))]
            ))
    );
    assert!(matches!(
        evaluation.namespace_unknowns.as_slice(),
        [unknown] if matches!(unknown.cause, PythonUnknownCauseView::Unreadable(_))
    ));
}

#[test]
fn star_import_copies_mutations_only_for_exact_or_absent_selected_names() {
    let (_file, exact) = evaluate_module_with(
        &[(
            "/project/plugin.py",
            "SELECTED = []\nSELECTED.append('selected')\nOMITTED = []\nOMITTED.append('omitted')\n__all__ = ['SELECTED']\n",
        )],
        "from plugin import *\n",
    ).expect("multi-file Python evaluation fixture should build");
    assert_eq!(
        exact
            .mutations
            .iter()
            .map(|mutation| mutation.binding.as_str())
            .collect::<Vec<_>>(),
        ["SELECTED"]
    );

    let (caller_file, absent) = evaluate_module_with(
        &[(
            "/project/plugin.py",
            "PUBLIC = []\nPUBLIC.append('public')\n_PRIVATE = []\n_PRIVATE.append('private')\n",
        )],
        "PUBLIC = []\nPUBLIC.append('caller')\nfrom plugin import *\n",
    )
    .expect("multi-file Python evaluation fixture should build");
    assert_eq!(
        absent
            .mutations
            .iter()
            .map(|mutation| mutation.binding.as_str())
            .collect::<Vec<_>>(),
        ["PUBLIC"]
    );
    assert_ne!(
        absent.mutations[0].origin.file, caller_file,
        "definite absent-__all__ overwrite must discard the caller mutation",
    );

    let (_file, dynamic) = evaluate_module_with(
        &[(
            "/project/plugin.py",
            "VALUE = []\nVALUE.append('dynamic')\n__all__ = NAMES\n",
        )],
        "from plugin import *\n",
    )
    .expect("multi-file Python evaluation fixture should build");
    assert!(dynamic.mutations.is_empty());
}

#[test]
fn ordinary_import_of_project_namespace_prefix_to_external_suffix() {
    // `ns` is a namespace spanning a first-party portion and a site-packages
    // portion; `ns.sub` resolves only under the external portion. The evaluated
    // namespace prefix survives, exactly one skipped-external outcome is
    // recorded for the requested leaf, and the requested root handle is bound.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/ns/placeholder.py", ""),
            (
                "/project/.venv/lib/python3.12/site-packages/ns/sub.py",
                "VALUE = 'external'\n",
            ),
        ],
        "import ns.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["external:ns.sub"]
    );
    assert_eq!(
        bound_module(&evaluation, "ns").expect("binding should contain one module value"),
        &PythonModuleView::Namespace(
            PythonModuleName::parse("ns").expect("test Python module name should be valid")
        )
    );
}

#[test]
fn ordinary_dotted_import_self_cycle_binds_root_and_records_leaf_cycle() {
    // `pkg.sub` imports itself through its dotted path: the parent `pkg`
    // resolves, the leaf is a cycle seed, the requested root handle is still
    // reachable, and the post-import local value survives.
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "import pkg.sub\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/sub.py", "import pkg.sub\nLEAF = 'sub'\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Resolved { imported_module, .. }
                if imported_module.as_str() == "pkg"
        )),
        "the parent package resolves as a prefix: {:?}",
        evaluation.imports,
    );
    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Cycle { imported_module, .. }
                if imported_module.as_str() == "pkg.sub"
        )),
        "the dotted leaf is a cycle edge: {:?}",
        evaluation.imports,
    );
    // The selected component for an unaliased dotted import is the root, which
    // was reached, so the root handle is bound.
    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    let value =
        bound_value(&evaluation, "LEAF").expect("binding should have one bound alternative");
    assert!(matches!(&value.value.kind, PythonValueKindView::Str(text) if text == "sub"));
}

#[test]
fn from_dotted_import_self_cycle_records_leaf_cycle_and_retains_local_value() {
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/pkg/sub.py",
        "from pkg.sub import LEAF\nLEAF = 'sub'\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Resolved { imported_module, .. }
                if imported_module.as_str() == "pkg"
        )),
        "the parent package resolves as a prefix: {:?}",
        evaluation.imports,
    );
    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Cycle { imported_module, .. }
                if imported_module.as_str() == "pkg.sub"
        )),
        "the dotted from-import source leaf is a cycle edge: {:?}",
        evaluation.imports,
    );
    // The later unconditional assignment dominates the cycle-degraded member.
    let value =
        bound_value(&evaluation, "LEAF").expect("binding should have one bound alternative");
    assert!(matches!(&value.value.kind, PythonValueKindView::Str(text) if text == "sub"));
}

/// Evaluate `/project/settings.py` against a filesystem where exactly one file
/// (`unreadable`) fails to read. Reuses the shared `ReadFailingFileSystem` so
/// direct/dotted unreadable positions can be probed at root, intermediate, and
/// leaf components without a bespoke filesystem per test.
fn evaluate_unreadable_module(
    files: &[(&str, &str)],
    unreadable: &str,
    source: &str,
) -> Result<(OsTestDatabase, File, PythonModuleEvaluationView), Box<dyn std::error::Error>> {
    let mut inner = InMemoryFileSystem::new();
    for (path, content) in files {
        inner.add_file((*path).into(), (*content).to_string());
    }
    inner.add_file("/project/settings.py".into(), source.to_string());
    let fs = ReadFailingFileSystem {
        inner,
        unreadable: unreadable.into(),
    };
    let db = OsTestDatabase::with_file_system(Arc::new(fs));
    let project = python_project(&db);
    let settings = path_to_file(&db, Utf8Path::new("/project/settings.py"))?;
    let evaluation = python_module_evaluation(&db, project, settings)?;
    Ok((db, settings, evaluation))
}

#[test]
fn ordinary_import_unreadable_root_binds_typed_unreadable_unknown() {
    // A direct root import whose only component fails to read records exactly one
    // typed unreadable edge (never a not-found) and binds a typed unreadable
    // unknown for the root name.
    let (db, settings, evaluation) = evaluate_unreadable_module(
        &[("/project/unreadable.py", "VALUE = 'hidden'\n")],
        "/project/unreadable.py",
        "import unreadable\n",
    )
    .expect("unreadable-module fixture should build");
    let unreadable = path_to_file(&db, Utf8Path::new("/project/unreadable.py"))
        .expect("unreadable fixture should still be discoverable");

    let (origin, file, from_module, to_module, error) = match only(&evaluation.imports) {
        Some(PythonImportOutcomeView::Unreadable {
            origin,
            file,
            importer_module,
            imported_module,
            error,
        }) => Some((origin, file, importer_module, imported_module, error)),
        _ => None,
    }
    .expect("expected one unreadable outcome");
    assert_eq!(origin.file, settings);
    assert_eq!(*file, unreadable);
    assert_eq!(from_module.as_str(), "settings");
    assert_eq!(to_module.as_str(), "unreadable");
    assert_eq!(
        error.kind,
        FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)
    );

    let bound =
        bound_value(&evaluation, "unreadable").expect("binding should have one bound alternative");
    assert!(matches!(
        &bound.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::Unreadable(error)
                if error.kind == FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied))
    ));
    // The unreadable component's file is retained as a dependency so a later
    // readable revision re-triggers evaluation.
    assert_eq!(evaluation.dependency_files, [settings, unreadable]);
}

#[test]
fn ordinary_dotted_import_unreadable_leaf_leaves_failed_child_unattached() {
    // The parent `pkg` resolves and records its edge; the unreadable leaf records
    // an unreadable edge (never a not-found). The successful `import pkg` keeps
    // `pkg` a module handle, the aliased failing clause binds only its alias, and
    // the failed leaf is never attached, so a parent member read surfaces a typed
    // module-attribute unknown.
    let (db, _settings, evaluation) = evaluate_unreadable_module(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "/project/pkg/sub.py",
        "import pkg\nimport pkg.sub as s\nCHILD = pkg.sub\n",
    )
    .expect("unreadable-module fixture should build");
    let pkg_init = path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
        .expect("test path should map to a source file");
    let pkg_sub = path_to_file(&db, Utf8Path::new("/project/pkg/sub.py"))
        .expect("test path should map to a source file");

    assert!(
        matches!(
            evaluation.imports.as_slice(),
            [
                PythonImportOutcomeView::Resolved { imported_module: a, file: fa, .. },
                PythonImportOutcomeView::Resolved { imported_module: b, file: fb, .. },
                PythonImportOutcomeView::Unreadable { imported_module: c, file: fc, error, .. },
            ] if a.as_str() == "pkg"
                && b.as_str() == "pkg"
                && c.as_str() == "pkg.sub"
                && *fa == pkg_init
                && *fb == pkg_init
                && *fc == pkg_sub
                && error.kind == FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)
        ),
        "{:?}",
        evaluation.imports,
    );

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    let alias = bound_value(&evaluation, "s").expect("binding should have one bound alternative");
    assert!(matches!(
        &alias.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::Unreadable(_))
    ));

    let child =
        bound_value(&evaluation, "CHILD").expect("binding should have one bound alternative");
    assert!(
        matches!(
            &child.value.kind,
            PythonValueKindView::Unknown(unknown)
                if matches!(&unknown.cause, PythonUnknownCauseView::ModuleAttribute { module, member }
                    if module.as_str() == "pkg" && member == "sub")
        ),
        "the unreadable leaf must not be attached under pkg, got {:?}",
        child.value.kind,
    );
}

#[test]
fn ordinary_dotted_import_unreadable_intermediate_preserves_root_edge() {
    // A dotted chain whose intermediate component fails to read stops at the
    // intermediate: the resolved root prefix `pkg` still records its edge before
    // the unreadable outcome for `pkg.mid`, and the whole statement fails so the
    // root binds a typed unreadable unknown.
    let (db, _settings, evaluation) = evaluate_unreadable_module(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/mid/__init__.py", "MID = 'mid'\n"),
            ("/project/pkg/mid/leaf.py", "LEAF = 'leaf'\n"),
        ],
        "/project/pkg/mid/__init__.py",
        "import pkg.mid.leaf\n",
    )
    .expect("unreadable-module fixture should build");
    let pkg_init = path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
        .expect("test path should map to a source file");
    let mid_init = path_to_file(&db, Utf8Path::new("/project/pkg/mid/__init__.py"))
        .expect("test path should map to a source file");

    assert!(
        matches!(
            evaluation.imports.as_slice(),
            [
                PythonImportOutcomeView::Resolved { imported_module: a, file: fa, .. },
                PythonImportOutcomeView::Unreadable { imported_module: b, file: fb, .. },
            ] if a.as_str() == "pkg"
                && *fa == pkg_init
                && b.as_str() == "pkg.mid"
                && *fb == mid_init
        ),
        "{:?}",
        evaluation.imports,
    );
    let bound = bound_value(&evaluation, "pkg").expect("binding should have one bound alternative");
    assert!(matches!(
        &bound.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::Unreadable(_))
    ));
}

#[test]
fn from_dotted_import_not_found_records_parent_prefix_and_fails_selection() {
    // The resolved source prefix `pkg` records its edge before the not-found
    // terminal; the named member is failed through the not-found policy.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "PARENT = 'parent'\n")],
        "from pkg.missing import VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "notfound:pkg.missing"]
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(
        &value.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module)
                if module.as_str() == "pkg.missing")
    ));
}

#[test]
fn from_dotted_import_unreadable_leaf_records_parent_prefix_and_fails_selection() {
    let (db, _settings, evaluation) = evaluate_unreadable_module(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/sub.py", "VALUE = 'v'\n"),
        ],
        "/project/pkg/sub.py",
        "from pkg.sub import VALUE\n",
    )
    .expect("unreadable-module fixture should build");
    let pkg_init = path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
        .expect("expected JSON value should be a string");
    let pkg_sub = path_to_file(&db, Utf8Path::new("/project/pkg/sub.py"))
        .expect("test path should map to a source file");

    assert!(
        matches!(
            evaluation.imports.as_slice(),
            [
                PythonImportOutcomeView::Resolved { imported_module: a, file: fa, .. },
                PythonImportOutcomeView::Unreadable { imported_module: b, file: fb, .. },
            ] if a.as_str() == "pkg"
                && *fa == pkg_init
                && b.as_str() == "pkg.sub"
                && *fb == pkg_sub
        ),
        "{:?}",
        evaluation.imports,
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(
        &value.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::Unreadable(_))
    ));
}

#[test]
fn from_dotted_import_syntax_leaf_records_syntax_edge_and_selects_member() {
    // A recoverable syntax error in the source leaf records a syntax edge but the
    // leaf still resolves, so the named member selection is unchanged.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            (
                "/project/pkg/sub.py",
                "CHILD = 'child'\nif FLAG:\n    OTHER = 'o'\n    broken(\n",
            ),
        ],
        "from pkg.sub import CHILD\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert!(evaluation.imports.iter().any(|outcome| matches!(
        outcome,
        PythonImportOutcomeView::Resolved { imported_module, .. }
            if imported_module.as_str() == "pkg"
    )));
    assert!(evaluation.imports.iter().any(|outcome| matches!(
        outcome,
        PythonImportOutcomeView::SyntaxErrors { imported_module, .. }
            if imported_module.as_str() == "pkg.sub"
    )));
    let child =
        bound_value(&evaluation, "CHILD").expect("binding should have one bound alternative");
    assert!(matches!(&child.value.kind, PythonValueKindView::Str(text) if text == "child"));
}

#[test]
fn from_dotted_import_external_suffix_skips_and_fails_selection() {
    // A namespace prefix `ns` spans into a site-packages `ns.sub`; the requested
    // leaf is external, so exactly one skipped-external outcome is recorded and
    // the named member is failed through the skipped-external policy.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/ns/placeholder.py", ""),
            (
                "/project/.venv/lib/python3.12/site-packages/ns/sub.py",
                "VALUE = 'ext'\n",
            ),
        ],
        "from ns.sub import VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["external:ns.sub"]
    );
    let value = evaluation.binding("VALUE").expect("VALUE should be bound");
    assert!(value.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView { kind: PythonValueKindView::Unknown(unknown), .. },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::SkippedExternal(module)
            if module.as_str() == "ns.sub")
    )));
    assert!(value.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView { kind: PythonValueKindView::Unknown(unknown), .. },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::MissingImportMember { module, member }
            if module.as_str() == "ns.sub" && member == "VALUE")
    )));
}

#[test]
fn from_dotted_import_namespace_leaf_records_source_prefix_and_not_found() {
    // The source parent `pkg` records its edge; the namespace leaf `pkg.ns` has
    // no loadable source module, so selection fails through the not-found policy.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/ns/mod.py", "X = 'x'\n"),
        ],
        "from pkg.ns import VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "notfound:pkg.ns.VALUE"]
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(
        &value.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::MissingImportMember { module, member }
                if module.as_str() == "pkg.ns" && member == "VALUE")
    ));
}

#[test]
fn ordinary_import_single_alias_binds_module_and_omits_source_name() {
    // A successful single-component aliased import binds only the alias to the
    // resolved source module handle; the source name itself is never bound.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/a/__init__.py", "VALUE = 'a'\n")],
        "import a as x\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "x").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("a").expect("test Python module name should be valid")
        )
    );
    assert!(
        evaluation.binding("a").is_none(),
        "an aliased import binds only the alias"
    );
    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:a"]
    );
}

#[test]
fn ordinary_dotted_import_recovers_leaf_syntax_and_attaches_child() {
    // The leaf `pkg/sub.py` has recoverable syntax errors: it records a syntax
    // edge but still resolves, so the root binds and the loaded child attaches
    // and is readable through the parent's module-member projection.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\nbroken(\n"),
        ],
        "import pkg.sub\nCHILD = pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Resolved { imported_module, .. }
                if imported_module.as_str() == "pkg"
        )),
        "the parent package resolves: {:?}",
        evaluation.imports,
    );
    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::SyntaxErrors { imported_module, .. }
                if imported_module.as_str() == "pkg.sub"
        )),
        "the leaf records a recovered syntax outcome: {:?}",
        evaluation.imports,
    );
    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
}

#[test]
fn from_dotted_import_unreadable_intermediate_records_root_prefix_and_fails_selection() {
    // The intermediate `pkg.mid` fails to read: the resolved root prefix `pkg`
    // records its edge before the unreadable outcome for `pkg.mid`, and the named
    // member is failed through the unreadable policy.
    let (db, _settings, evaluation) = evaluate_unreadable_module(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/mid/__init__.py", "MID = 'mid'\n"),
            ("/project/pkg/mid/leaf.py", "VALUE = 'v'\n"),
        ],
        "/project/pkg/mid/__init__.py",
        "from pkg.mid.leaf import VALUE\n",
    )
    .expect("unreadable-module fixture should build");
    let pkg_init = path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
        .expect("test path should map to a source file");
    let mid_init = path_to_file(&db, Utf8Path::new("/project/pkg/mid/__init__.py"))
        .expect("test path should map to a source file");

    assert!(
        matches!(
            evaluation.imports.as_slice(),
            [
                PythonImportOutcomeView::Resolved { imported_module: a, file: fa, .. },
                PythonImportOutcomeView::Unreadable { imported_module: b, file: fb, .. },
            ] if a.as_str() == "pkg"
                && *fa == pkg_init
                && b.as_str() == "pkg.mid"
                && *fb == mid_init
        ),
        "{:?}",
        evaluation.imports,
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(
        &value.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::Unreadable(_))
    ));
}

#[test]
fn from_dotted_import_not_found_intermediate_records_root_prefix_and_fails_selection() {
    // A missing intermediate component stops resolution at the missing name; the
    // resolved root prefix `pkg` still records its edge before the not-found, and
    // the named member is failed through the not-found policy.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "PARENT = 'parent'\n")],
        "from pkg.missing.leaf import VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "notfound:pkg.missing.leaf"]
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(
        &value.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module)
                if module.as_str() == "pkg.missing.leaf")
    ));
}

#[test]
fn from_dotted_import_namespace_parent_selects_source_child_member() {
    // The parent `nspkg` is a namespace package (no __init__.py) and produces no
    // edge; the source child `nspkg.mod` resolves and the named member is
    // selected unchanged.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/nspkg/mod.py", "VALUE = 'child'\n")],
        "from nspkg.mod import VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:nspkg.mod"]
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(&value.value.kind, PythonValueKindView::Str(text) if text == "child"));
}

#[test]
fn member_read_of_intrinsic_binding_after_bare_import_selects_value() {
    // Reading a member that is one of the imported module's own (intrinsic)
    // bindings after a bare `import pkg` selects that value unchanged.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/pkg/__init__.py", "VALUE = 'pkg'\n")],
        "import pkg\nATTR = pkg.VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    let attr = bound_value(&evaluation, "ATTR").expect("binding should have one bound alternative");
    assert!(matches!(&attr.value.kind, PythonValueKindView::Str(text) if text == "pkg"));
}

#[test]
fn member_read_of_parent_init_binding_after_dotted_root_import_selects_value() {
    // An unaliased `import pkg.sub` binds the root `pkg`; reading the parent
    // package's own `__init__.py` binding through the root selects that value.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg.sub\nATTR = pkg.PARENT\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    let attr = bound_value(&evaluation, "ATTR").expect("binding should have one bound alternative");
    assert!(matches!(&attr.value.kind, PythonValueKindView::Str(text) if text == "parent"));
}

#[test]
fn member_read_of_leaf_binding_through_dotted_alias_selects_value() {
    // `import pkg.sub as s` binds the alias to the leaf module; reading the leaf's
    // own binding through the alias selects that value.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg.sub as s\nATTR = s.LEAF\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        bound_module(&evaluation, "s").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
    let attr = bound_value(&evaluation, "ATTR").expect("binding should have one bound alternative");
    assert!(matches!(&attr.value.kind, PythonValueKindView::Str(text) if text == "leaf"));
}

#[test]
fn ordinary_dotted_import_source_parent_binds_namespace_leaf_child() {
    // A source parent `pkg` reaches a namespace leaf `pkg.ns`. The namespace leaf
    // is not a definite failure, so the unaliased import binds the resolved root
    // handle and the namespace child is attached and readable.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ("/project/pkg/ns/mod.py", "X = 'x'\n"),
        ],
        "import pkg.ns\nCHILD = pkg.ns\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg"]
    );
    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Namespace(
            PythonModuleName::parse("pkg.ns").expect("test Python module name should be valid")
        )
    );
}

#[test]
fn ordinary_dotted_import_namespace_intermediate_reaches_source_leaf() {
    // Three components: source root `pkg`, namespace intermediate `pkg.ns`, and
    // source leaf `pkg.ns.leaf`. The namespace intermediate produces no edge, but
    // the chain continues to the source leaf and both source edges are recorded.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/ns/leaf.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg.ns.leaf\nCHILD = pkg.ns.leaf\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.ns.leaf"]
    );
    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.ns.leaf")
                .expect("test Python module name should be valid")
        )
    );
}

#[test]
fn ordinary_import_binds_module_from_extra_root() {
    // An ordinary direct import resolves through an extra (project-code) root and
    // binds the source module handle, recording the extra-root file as a resolved
    // edge and dependency.
    let db = TestDatabase::new();
    db.add_file("/vendor/shared.py", "VALUE = 'extra'\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/settings.py", "import shared\n")
        .expect("settings-extraction test file should be added");
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor")]);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let shared = db
        .file(Utf8Path::new("/vendor/shared.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert_eq!(
        bound_module(&evaluation, "shared").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("shared").expect("test Python module name should be valid")
        )
    );
    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::Resolved { file, imported_module, .. }]
            if *file == shared && imported_module.as_str() == "shared"
    ));
    assert_eq!(evaluation.dependency_files, [settings, shared]);
}

#[test]
fn ordinary_dotted_import_records_exact_prefix_dependencies_and_surviving_child_value() {
    // The prefix chain records dependency files in first-seen root-to-leaf order
    // and the loaded child's own body survives, readable through the parent's
    // module-member projection.
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "PARENT = 'parent'\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/sub.py", "LEAF = 'leaf'\n")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/settings.py",
        "import pkg.sub\nCHILD = pkg.sub\nVIA = pkg.sub.LEAF\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let pkg_init = db
        .file(Utf8Path::new("/project/pkg/__init__.py"))
        .expect("settings-extraction test file should exist");
    let pkg_sub = db
        .file(Utf8Path::new("/project/pkg/sub.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:pkg", "resolved:pkg.sub"]
    );
    assert_eq!(evaluation.dependency_files, [settings, pkg_init, pkg_sub]);
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
    let via = bound_value(&evaluation, "VIA").expect("binding should have one bound alternative");
    assert!(matches!(&via.value.kind, PythonValueKindView::Str(text) if text == "leaf"));
}

#[test]
fn ordinary_dotted_import_records_exact_outcome_sequence_and_identities() {
    // The full outcome sequence for a two-component dotted chain: both components
    // share the clause origin, carry their own files and imported identities, and
    // the dependency array is the first-seen root-to-leaf order.
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "PARENT = 'parent'\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/sub.py", "LEAF = 'leaf'\n")
        .expect("settings-extraction test file should be added");
    let source = "import pkg.sub\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let pkg_init = db
        .file(Utf8Path::new("/project/pkg/__init__.py"))
        .expect("settings-extraction test file should exist");
    let pkg_sub = db
        .file(Utf8Path::new("/project/pkg/sub.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let clause = Origin::new(
        settings,
        expected_span(source, "pkg.sub").expect("test source should contain expected text"),
    );
    let settings_module =
        PythonModuleName::parse("settings").expect("test Python module name should be valid");
    assert_eq!(
        evaluation.imports,
        vec![
            PythonImportOutcomeView::Resolved {
                origin: clause,
                file: pkg_init,
                importer_module: settings_module.clone(),
                imported_module: PythonModuleName::parse("pkg")
                    .expect("test Python module name should be valid"),
            },
            PythonImportOutcomeView::Resolved {
                origin: clause,
                file: pkg_sub,
                importer_module: settings_module,
                imported_module: PythonModuleName::parse("pkg.sub")
                    .expect("test Python module name should be valid"),
            },
        ]
    );
    assert_eq!(evaluation.dependency_files, [settings, pkg_init, pkg_sub]);
}

#[test]
fn successful_direct_imports_bind_exact_target_origins() {
    // Each successful direct form binds its local name at the exact binding
    // target: the root form at the root segment, the alias form at the alias
    // identifier, and the unaliased dotted form at the root segment.
    let db = TestDatabase::new();
    db.add_file("/project/root.py", "R = 'r'\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/__init__.py", "")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/sub.py", "S = 's'\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/dpkg/__init__.py", "")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/dpkg/leaf.py", "L = 'l'\n")
        .expect("settings-extraction test file should be added");
    let source = "import root\nimport pkg.sub as salias\nimport dpkg.leaf\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert_eq!(
        bound_value(&evaluation, "root")
            .expect("binding should have one bound alternative")
            .binding_origins,
        [Origin::new(
            settings,
            binding_target_span(source, "import root", "root")
                .expect("import clause should contain expected binding target")
        )]
    );
    assert_eq!(
        bound_value(&evaluation, "salias")
            .expect("binding should have one bound alternative")
            .binding_origins,
        [Origin::new(
            settings,
            binding_target_span(source, "as salias", "salias")
                .expect("import clause should contain expected binding target")
        )]
    );
    assert_eq!(
        bound_value(&evaluation, "dpkg")
            .expect("binding should have one bound alternative")
            .binding_origins,
        [Origin::new(
            settings,
            binding_target_span(source, "import dpkg.leaf", "dpkg")
                .expect("import clause should contain expected binding target")
        )]
    );
}

#[test]
fn ordinary_import_binds_multiple_clauses_in_source_order() {
    // A multi-clause single statement of all-successful imports binds each module
    // handle and records edges in left-to-right source order.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/alpha/__init__.py", ""),
            ("/project/beta/__init__.py", ""),
        ],
        "import alpha, beta\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["resolved:alpha", "resolved:beta"]
    );
    assert_eq!(
        bound_module(&evaluation, "alpha").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("alpha").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "beta").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("beta").expect("test Python module name should be valid")
        )
    );
}

#[test]
fn ordinary_import_mixed_clauses_preserve_source_order_after_failure() {
    // A single statement mixing a failing then a succeeding clause preserves
    // source order: the earlier failure does not suppress the later success.
    let (_file, evaluation) = evaluate_module_with(
        &[("/project/alpha/__init__.py", "")],
        "import missing, alpha\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["notfound:missing", "resolved:alpha"]
    );
    let missing =
        bound_value(&evaluation, "missing").expect("binding should have one bound alternative");
    assert!(matches!(
        &missing.value.kind,
        PythonValueKindView::Unknown(unknown)
            if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module)
                if module.as_str() == "missing")
    ));
    assert_eq!(
        bound_module(&evaluation, "alpha").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("alpha").expect("test Python module name should be valid")
        )
    );
}

#[test]
fn ordinary_dotted_import_external_suffix_attaches_open_child() {
    // A namespace root `ns` with an external suffix `ns.sub` binds the namespace
    // root, attaches the external suffix child, and preserves the suffix's
    // skipped-external open cause on member reads.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/ns/placeholder.py", ""),
            (
                "/project/.venv/lib/python3.12/site-packages/ns/sub.py",
                "VALUE = 'ext'\n",
            ),
        ],
        "import ns.sub\nCHILD = ns.sub\nATTR = ns.sub.VALUE\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    assert_eq!(
        import_module_names(&evaluation)
            .expect("import outcomes should have supported test shapes"),
        ["external:ns.sub"]
    );
    assert_eq!(
        bound_module(&evaluation, "ns").expect("binding should contain one module value"),
        &PythonModuleView::Namespace(
            PythonModuleName::parse("ns").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "CHILD").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("ns.sub").expect("test Python module name should be valid")
        )
    );
    let attr = evaluation.binding("ATTR").expect("ATTR should be bound");
    assert!(
        attr.alternatives.iter().any(|alternative| matches!(
            alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::Unknown(unknown),
                    ..
                },
                ..
            }) if matches!(&unknown.cause, PythonUnknownCauseView::SkippedExternal(module)
                if module.as_str() == "ns.sub")
        )),
        "reading an external suffix attribute surfaces SkippedExternal: {:?}",
        attr.alternatives,
    );
}

#[test]
fn ordinary_dotted_import_self_cycle_attaches_open_child() {
    // A dotted self-cycle attaches the cyclic child under the resolved root, so a
    // parent member read of the child is a module handle, but attribute reads of
    // the cyclic child surface the cycle open cause. The post-import local value
    // survives.
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "import pkg.sub\n")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/pkg/sub.py",
        "import pkg.sub\nLEAF = 'sub'\nSELF = pkg.sub\nCYC = pkg.sub.LEAF\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    assert!(evaluation.imports.iter().any(|outcome| matches!(
        outcome,
        PythonImportOutcomeView::Resolved { imported_module, .. }
            if imported_module.as_str() == "pkg"
    )));
    assert!(evaluation.imports.iter().any(|outcome| matches!(
        outcome,
        PythonImportOutcomeView::Cycle { imported_module, .. }
            if imported_module.as_str() == "pkg.sub"
    )));
    assert_eq!(
        bound_module(&evaluation, "pkg").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg").expect("test Python module name should be valid")
        )
    );
    assert_eq!(
        bound_module(&evaluation, "SELF").expect("binding should contain one module value"),
        &PythonModuleView::Source(
            PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid")
        )
    );
    let cyc = bound_value(&evaluation, "CYC").expect("binding should have one bound alternative");
    assert!(
        matches!(
            &cyc.value.kind,
            PythonValueKindView::Unknown(unknown)
                if matches!(&unknown.cause, PythonUnknownCauseView::Cycle)
        ),
        "reading a cyclic child attribute surfaces the cycle open cause, got {:?}",
        cyc.value.kind,
    );
    let leaf = bound_value(&evaluation, "LEAF").expect("binding should have one bound alternative");
    assert!(matches!(&leaf.value.kind, PythonValueKindView::Str(text) if text == "sub"));
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

fn string_list_items(value: &PythonValueView) -> Option<Vec<String>> {
    let (PythonValueKindView::List(items) | PythonValueKindView::Tuple(items)) = &value.kind else {
        return None;
    };
    Some(
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
            .collect(),
    )
}

fn nested_list_at_index0(value: &PythonValueView) -> Option<&PythonValueView> {
    let PythonValueKindView::Tuple(items) = &value.kind else {
        return None;
    };
    match only(items)? {
        PythonSequenceItemView::Value(nested) => Some(nested),
        PythonSequenceItemView::UnknownElement(_) | PythonSequenceItemView::UnknownUnpack(_) => {
            None
        }
    }
}

#[test]
fn tuple_index_augmented_add_mutates_nested_list_transactionally() {
    // ROOT is an immutable tuple, but tuple indexing reaches the nested mutable
    // list, so `ROOT[0] += [...]` mutates that list in place while the tuple
    // structure is preserved.
    let source = "ROOT = ([],)\nROOT[0] += ['a']\n";
    let (file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");
    let bound =
        bound_value(&evaluation, "ROOT").expect("binding should have one bound alternative");
    let nested = nested_list_at_index0(&bound.value).expect("ROOT should contain one nested value");

    assert_eq!(
        string_list_items(nested).expect("value should be a list or tuple"),
        vec!["str:a"]
    );
    // Ancestor provenance: the operation origin is recorded up the path onto
    // the root tuple value.
    let op_origin = Origin::new(
        file,
        expected_span(source, "ROOT[0] += ['a']")
            .expect("test source should contain expected text"),
    );
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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");
    let bound =
        bound_value(&evaluation, "ROOT").expect("binding should have one bound alternative");
    let nested = nested_list_at_index0(&bound.value).expect("ROOT should contain one nested value");

    assert_eq!(
        string_list_items(nested).expect("value should be a list or tuple"),
        vec!["str:a"]
    );
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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

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
    let (file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");
    let bound =
        bound_value(&evaluation, "VALUES").expect("binding should have one bound alternative");

    assert_eq!(
        string_list_items(&bound.value).expect("value should be a list or tuple"),
        vec!["str:a", "str:b"]
    );
    assert_eq!(
        bound.binding_origins,
        vec![Origin::new(
            file,
            expected_span(source, "[]").expect("test source should contain expected text")
        )],
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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

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
    let (_file, augmented) = evaluate_module("VALUES = []\nVALUES += True\n")
        .expect("Python module evaluation fixture should build");
    assert!(has_unknown_alternative(&augmented, "VALUES"));
    assert!(
        augmented
            .mutations
            .iter()
            .any(|mutation| mutation.binding == "VALUES"
                && mutation.operation == PythonMutationOperationView::Extend),
        "name-target `+= bool` records an attempted extend fact",
    );

    let (_file, extended) = evaluate_module("FLAG = True\nVALUES = []\nVALUES.extend(FLAG)\n")
        .expect("Python module evaluation fixture should build");
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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

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
    let flag = bound_value(&evaluation, "FLAG").expect("binding should have one bound alternative");
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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

    let src = bound_value(&evaluation, "SRC").expect("binding should have one bound alternative");
    assert_eq!(
        string_list_items(&src.value).expect("value should be a list or tuple"),
        vec!["str:x"]
    );
    let values =
        bound_value(&evaluation, "VALUES").expect("binding should have one bound alternative");
    assert_eq!(
        string_list_items(&values.value).expect("value should be a list or tuple"),
        vec!["str:x"]
    );
}

#[test]
fn failed_extend_alias_rhs_degradation_takes_precedence_over_preservation() {
    // The RHS `ALIAS` aliases the failing dictionary receiver `ROOT`. Because
    // it is selected by the mutation-failure alias set, its degradation takes
    // precedence over RHS-only preservation.
    let source = "ROOT = {}\nALIAS = ROOT\nROOT.extend(ALIAS)\n";
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");

    for name in ["ROOT", "ALIAS"] {
        let binding = evaluation
            .binding(name)
            .expect("mutated alias should remain bound");
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
        let (_file, evaluation) =
            evaluate_module(&source).expect("Python module evaluation fixture should build");
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
    let (_file, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");
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
        let (_file, evaluation) =
            evaluate_module(source).expect("Python module evaluation fixture should build");
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
        let (_file, evaluation) =
            evaluate_module(&source).expect("Python module evaluation fixture should build");
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let bound = only_bound(&binding.alternatives)
        .expect("the empty lists should normalize into one bound alternative");

    assert_eq!(bound.value.origins.len(), 2);
    assert!(matches!(
        evaluation.mutations.as_slice(),
        [mutation]
            if mutation.binding == "VALUES"
                && mutation.operation == PythonMutationOperationView::Extend
                && mutation.origin.span == expected_span(source, "VALUES.extend([])").expect("test source should contain expected text")
    ));
}

#[test]
fn ambiguous_branch_mutations_remain_uncorrelated_may_have_evidence() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nif FLAG:\n    VALUES.append('item')\nelse:\n    VALUES.extend([])\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let mut list_lengths = binding
        .alternatives
        .iter()
        .map(|alternative| match alternative {
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value:
                    PythonValueView {
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    for name in ["ITEM", "VALUE", "FALLBACK"] {
        let binding = evaluation
            .binding(name)
            .expect("matched name should participate in the match join");
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

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
            .expect("test source should contain expected text")
    );
    assert_eq!(
        evaluation.mutations[1].origin.span,
        expected_span(source, "Z_VALUES.append('z')")
            .expect("test source should contain expected text")
    );
}

#[test]
fn python_module_evaluation_keeps_failed_star_import_from_loop_body() {
    let db = TestDatabase::new();
    let source = "for item in ITEMS:\n    from missing_star import *\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::NotFound { origin, module }]
            if origin.file == settings
                && origin.span == expected_span(source, "from missing_star import *").expect("test source should contain expected text")
                && module.as_str() == "missing_star"
    ));
    assert!(matches!(
        evaluation.namespace_unknowns.as_slice(),
        [unknown]
            if matches!(&unknown.cause, PythonUnknownCauseView::ImportNotFound(module) if module.as_str() == "missing_star")
                && unknown.origins.as_slice() == [Origin::new(
                    settings,
                    expected_span(source, "from missing_star import *").expect("test source should contain expected text"),
                )]
    ));
}

#[test]
fn python_module_evaluation_reports_invalid_import_with_typed_cause() {
    let db = TestDatabase::new();
    let source = "from ...missing import VALUE\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::InvalidImport { origin, reason }]
            if origin.file == settings
                && origin.span == expected_span(source, source.trim_end()).expect("test source should contain expected text")
                && *reason == PythonImportNameErrorView::TooManyDots
    ));
    let binding = evaluation
        .binding("VALUE")
        .expect("failed named import should bind unknown");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::InvalidImport(PythonImportNameErrorView::TooManyDots)
            && unknown.origins.as_slice() == [Origin::new(
                settings,
                expected_span(source, source.trim_end()).expect("test source should contain expected text"),
            )]
    ));
}

#[test]
fn python_module_evaluation_follows_named_and_star_imports_from_extra_roots() {
    for source in ["from shared import VALUE\n", "from shared import *\n"] {
        let db = TestDatabase::new();
        db.add_file("/vendor/shared.py", "VALUE = 'extra'\n")
            .expect("settings-extraction test file should be added");
        db.add_file("/project/settings.py", source)
            .expect("settings-extraction test file should be added");
        let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor")]);
        let settings = db
            .file(Utf8Path::new("/project/settings.py"))
            .expect("settings-extraction test file should exist");
        let shared = db
            .file(Utf8Path::new("/vendor/shared.py"))
            .expect("settings-extraction test file should exist");
        let evaluation = python_module_evaluation(&db, project, settings)
            .expect("Python file should map to a module");

        assert!(matches!(
            evaluation.imports.as_slice(),
            [PythonImportOutcomeView::Resolved { file, .. }] if *file == shared
        ));
        assert_eq!(evaluation.dependency_files, [settings, shared]);
        assert!(matches!(
            evaluation.binding("VALUE").expect("expected test binding should exist").alternatives.as_slice(),
            [PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
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
    db.add_file("/vendor/site-packages/external.py", "VALUE = 'external'\n")
        .expect("settings-extraction test file should be added");
    let source = "from external import VALUE\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor/site-packages")]);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(matches!(
        evaluation.imports.as_slice(),
        [PythonImportOutcomeView::SkippedExternal { origin, module }]
            if origin.file == settings && module.as_str() == "external"
    ));
    let binding = evaluation
        .binding("VALUE")
        .expect("skipped import should bind unknown");
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView { kind: PythonValueKindView::Unknown(unknown), .. },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::SkippedExternal(module) if module.as_str() == "external")
    )));
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView { kind: PythonValueKindView::Unknown(unknown), .. },
            ..
        }) if matches!(&unknown.cause, PythonUnknownCauseView::MissingImportMember { module, member }
            if module.as_str() == "external" && member == "VALUE")
    )));
}

#[test]
fn python_module_evaluation_skips_external_star_import() {
    let db = TestDatabase::new();
    db.add_file("/vendor/site-packages/external.py", "VALUE = 'external'\n")
        .expect("settings-extraction test file should be added");
    let source = "from external import *\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor/site-packages")]);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

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
        db.add_file("/vendor/site-packages/project.pth", "/editable-package\n")
            .expect("settings-extraction test file should be added");
        db.add_file("/editable-package/external.py", "VALUE = 'external'\n")
            .expect("settings-extraction test file should be added");
        db.add_file("/project/settings.py", source)
            .expect("settings-extraction test file should be added");
        let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/vendor/site-packages")]);
        let settings = db
            .file(Utf8Path::new("/project/settings.py"))
            .expect("settings-extraction test file should exist");
        let evaluation = python_module_evaluation(&db, project, settings)
            .expect("Python file should map to a module");

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
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let (origin, file, error) = match only(&evaluation.imports) {
        Some(PythonImportOutcomeView::Unreadable {
            origin,
            file,
            error,
            ..
        }) => Some((origin, file, error)),
        _ => None,
    }
    .expect("expected one typed unreadable import outcome");
    assert_eq!(origin.file, settings);
    assert_eq!(*file, unreadable);
    assert_eq!(error.path, Utf8Path::new("/project/unreadable.py"));
    assert_eq!(
        error.kind,
        FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)
    );
    let binding = evaluation
        .binding("VALUE")
        .expect("unreadable import should bind unknown");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if matches!(&unknown.cause, PythonUnknownCauseView::Unreadable(error)
            if error.path == Utf8Path::new("/project/unreadable.py")
                && error.kind == FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied))
    ));
}

#[test]
fn python_module_evaluation_reports_import_syntax_errors() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/broken.py",
        "if FLAG:\n    VALUE = 'known'\n    broken(\n",
    )
    .expect("settings-extraction test file should be added");
    let broken = db
        .file(Utf8Path::new("/project/broken.py"))
        .expect("settings-extraction test file should exist");
    let source = "from broken import VALUE\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let (origin, file, errors) = match only(&evaluation.imports) {
        Some(PythonImportOutcomeView::SyntaxErrors {
            origin,
            file,
            errors,
            ..
        }) => Some((origin, file, errors)),
        _ => None,
    }
    .expect("syntax failure should have one typed import outcome");
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
                value: PythonValueView {
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let mutation = only(&evaluation.mutations).expect("append should record one mutation");
    assert_eq!(mutation.binding, "VALUES");
    assert_eq!(mutation.operation, PythonMutationOperationView::Append);
    assert!(mutation.path.is_empty());
    assert_eq!(
        mutation.origin,
        Origin::new(
            settings,
            expected_span(source, "VALUES.append('added')")
                .expect("test source should contain expected text")
        )
    );
    let binding = evaluation
        .binding("VALUES")
        .expect("VALUES should be bound");
    let bound =
        only_bound(&binding.alternatives).expect("VALUES should have one bound alternative");
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let [append, augmented_add]: &[_; 2] = evaluation
        .mutations
        .as_slice()
        .try_into()
        .expect("the nested mutations should produce two durable facts");
    let expected_path = [
        PythonMutationPathSegmentView::Index(0),
        PythonMutationPathSegmentView::Key("DIRS".to_string()),
    ];
    assert_eq!(append.binding, "TEMPLATES");
    assert_eq!(append.path, expected_path);
    assert_eq!(append.operation, PythonMutationOperationView::Append);
    assert_eq!(
        append.origin,
        Origin::new(
            settings,
            expected_span(source, "TEMPLATES[0]['DIRS'].append('/one')")
                .expect("test source should contain expected text"),
        )
    );
    assert_eq!(augmented_add.binding, "TEMPLATES");
    assert_eq!(augmented_add.path, expected_path);
    assert_eq!(augmented_add.operation, PythonMutationOperationView::Extend);
    assert_eq!(
        augmented_add.origin,
        Origin::new(
            settings,
            expected_span(source, "TEMPLATES[0]['DIRS'] += ['/two']")
                .expect("test source should contain expected text"),
        )
    );
}

#[test]
fn python_module_evaluation_discards_facts_after_unsupported_mutation() {
    let db = TestDatabase::new();
    let source = "VALUES = []\nVALUES.append('stale')\nVALUES.clear()\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    assert!(evaluation.mutations.is_empty());
    let binding = evaluation
        .binding("VALUES")
        .expect("the invalidated binding should remain observable");
    assert!(matches!(
        binding.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                ..
            },
            ..
        })] if unknown.cause == PythonUnknownCauseView::UnsupportedMutation
            && unknown.origins.as_slice() == [Origin::new(
                settings,
                expected_span(source, "VALUES.clear()").expect("test source should contain expected text"),
            )]
    ));
}

#[test]
fn python_module_evaluation_records_nested_string_augmented_add_as_iterable_extension() {
    let db = TestDatabase::new();
    let source = "TEMPLATES = [{'DIRS': []}]\nTEMPLATES[0]['DIRS'].append('/one')\nTEMPLATES[0]['DIRS'] += 'invalid'\n";
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let [append, augmented_add]: &[_; 2] = evaluation
        .mutations
        .as_slice()
        .try_into()
        .expect("both attempted mutations should remain observable");
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
                value: PythonValueView {
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
    db.add_file("/project/settings.py", source)
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");
    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");

    let mutation = only(&evaluation.mutations).expect("the loop body mutation should be retained");
    assert_eq!(mutation.binding, "VALUES");
    assert_eq!(mutation.operation, PythonMutationOperationView::Append);
    assert!(mutation.path.is_empty());
    assert_eq!(
        mutation.origin,
        Origin::new(
            settings,
            expected_span(source, "VALUES.append(item)")
                .expect("test source should contain expected text")
        ),
    );
}

fn cycle_products(
    query_a_first: bool,
) -> Result<(PythonModuleEvaluationView, PythonModuleEvaluationView), Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "from b import B\nA = B\nAFTER_A = 'a'\n")?;
    db.add_file("/project/b.py", "from a import A\nB = A\nAFTER_B = 'b'\n")?;
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"))?;
    let b = db.file(Utf8Path::new("/project/b.py"))?;

    if query_a_first {
        let _ = python_module_evaluation(&db, project, a)?;
    } else {
        let _ = python_module_evaluation(&db, project, b)?;
    }
    Ok((
        python_module_evaluation(&db, project, a)?,
        python_module_evaluation(&db, project, b)?,
    ))
}

#[test]
fn python_module_cycle_products_are_entry_order_independent() {
    let a_first = cycle_products(true).expect("Python cycle evaluation fixture should build");
    let b_first = cycle_products(false).expect("Python cycle evaluation fixture should build");

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
    db.add_file("/project/lib/pkg/a.py", "from .b import B\nA = B\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/lib/pkg/b.py", "from lib.pkg.a import A\nB = A\n")
        .expect("settings-extraction test file should be added");
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/project/lib")]);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.a").expect("test Python module name should be valid"),
    )
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
            PythonImportOutcomeView::Resolved { .. }
            | PythonImportOutcomeView::InvalidImport { .. }
            | PythonImportOutcomeView::NotFound { .. }
            | PythonImportOutcomeView::SkippedExternal { .. }
            | PythonImportOutcomeView::Unreadable { .. }
            | PythonImportOutcomeView::SyntaxErrors { .. } => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(cycles, [("lib.pkg.a", "lib.pkg.b")]);
}

#[test]
fn branch_constraints_distinguish_overlapping_root_module_identities() {
    let db = TestDatabase::new();
    db.add_file(
        "/project/settings.py",
        "from lib.pkg.branch import VALUE as ROOT_VALUE\nfrom pkg.branch import VALUE as NESTED_VALUE\nPAIR = (ROOT_VALUE, NESTED_VALUE)\n",
    )
    .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/lib/pkg/branch.py",
        "if FLAG:\n    VALUE = 'left'\nelse:\n    VALUE = 'right'\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project_with_paths(&db, &[Utf8PathBuf::from("/project/lib")]);
    let settings = db
        .file(Utf8Path::new("/project/settings.py"))
        .expect("settings-extraction test file should exist");

    let evaluation = python_module_evaluation(&db, project, settings)
        .expect("Python file should map to a module");
    let pairs = evaluation
        .binding("PAIR")
        .expect("PAIR binding should exist")
        .alternatives
        .iter()
        .filter_map(|alternative| {
            let PythonBindingAlternativeView::Bound(bound) = alternative else {
                return None;
            };
            string_list_items(&bound.value)
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(
        pairs,
        [
            vec!["str:left".to_string(), "str:left".to_string()],
            vec!["str:left".to_string(), "str:right".to_string()],
            vec!["str:right".to_string(), "str:left".to_string()],
            vec!["str:right".to_string(), "str:right".to_string()],
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn typed_module_order_disjoint_import_cycles_are_root_order_independent() {
    let cycle_edges = |settings_source| {
        let db = TestDatabase::new();
        db.add_file("/project/settings.py", settings_source)
            .expect("settings-extraction test file should be added");
        db.add_file("/project/a.py", "from b import B\nA = B\n")
            .expect("settings-extraction test file should be added");
        db.add_file("/project/b.py", "from a import A\nB = A\n")
            .expect("settings-extraction test file should be added");
        db.add_file("/project/x.py", "from y import Y\nX = Y\n")
            .expect("settings-extraction test file should be added");
        db.add_file("/project/y.py", "from x import X\nY = X\n")
            .expect("settings-extraction test file should be added");
        let project = python_project(&db);
        let settings = db
            .file(Utf8Path::new("/project/settings.py"))
            .expect("settings-extraction test file should exist");

        python_module_evaluation(&db, project, settings)
            .expect("Python file should map to a module")
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
                PythonImportOutcomeView::Resolved { .. }
                | PythonImportOutcomeView::InvalidImport { .. }
                | PythonImportOutcomeView::NotFound { .. }
                | PythonImportOutcomeView::SkippedExternal { .. }
                | PythonImportOutcomeView::Unreadable { .. }
                | PythonImportOutcomeView::SyntaxErrors { .. } => None,
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
    db.add_file("/project/a.py", "from b import B\nA = B\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/b.py", "from a import A\nB = A\nbroken(\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let a = db
        .file(Utf8Path::new("/project/a.py"))
        .expect("settings-extraction test file should exist");

    let evaluation =
        python_module_evaluation(&db, project, a).expect("Python file should map to a module");
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
            PythonImportOutcomeView::Resolved { .. }
            | PythonImportOutcomeView::InvalidImport { .. }
            | PythonImportOutcomeView::NotFound { .. }
            | PythonImportOutcomeView::SkippedExternal { .. }
            | PythonImportOutcomeView::Unreadable { .. }
            | PythonImportOutcomeView::SyntaxErrors { .. }
            | PythonImportOutcomeView::Cycle { .. } => None,
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
    let (a, b) = cycle_products(true).expect("Python cycle evaluation fixture should build");

    for (evaluation, cycle_name, stable_name, stable_value) in
        [(&a, "A", "AFTER_A", "a"), (&b, "B", "AFTER_B", "b")]
    {
        let cycle = evaluation
            .binding(cycle_name)
            .expect("cyclic value should be represented");
        assert!(matches!(
            cycle.alternatives.as_slice(),
            [PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
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
                value: PythonValueView {
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
    )
    .expect("settings extraction project should build");
    let settings_file = settings_module_file(&db, project)
        .expect("test settings module should resolve to a source file");
    let evaluation = python_module_evaluation(&db, project, settings_file)
        .expect("Python file should map to a module");
    let expected_origins = ["from a import *", "from b import *"].map(|statement| {
        Origin::new(
            settings_file,
            expected_span(source, statement).expect("test source should contain expected text"),
        )
    });
    let unknown = only(&evaluation.namespace_unknowns)
        .expect("cycle causes should merge into one plural unknown");
    assert_eq!(unknown.cause, PythonUnknownCauseView::Cycle);
    assert_eq!(unknown.origins, expected_origins);

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
    let evaluation = python_module_evaluation(&db, project, settings_file)
        .expect("Python file should map to a module");
    let binding = evaluation
        .binding("INSTALLED_APPS")
        .expect("the star import should copy the cyclic setting binding");
    assert!(
        binding
            .alternatives
            .contains(&PythonBindingAlternativeView::Unbound),
        "dynamic star selection cannot guarantee the cyclic name is exported",
    );
    assert!(binding.alternatives.iter().any(|alternative| matches!(
        alternative,
        PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
                kind: PythonValueKindView::Unknown(unknown),
                origins,
            },
            binding_origins,
        }) if unknown.cause == PythonUnknownCauseView::Cycle
            && unknown.origins.as_slice() == [import_origin]
            && origins.as_slice() == [import_origin]
            && binding_origins.as_slice() == [import_origin]
    )));

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

fn external_star_cycle_product(
    query_a_first: bool,
) -> Result<(PythonModuleEvaluationView, File), Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "from b import *\n")?;
    db.add_file("/project/b.py", "from a import *\n")?;
    db.add_file("/project/external.py", "from a import *\n")?;
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"))?;
    let b = db.file(Utf8Path::new("/project/b.py"))?;
    let external = db.file(Utf8Path::new("/project/external.py"))?;

    if query_a_first {
        let _ = python_module_evaluation(&db, project, a)?;
    } else {
        let _ = python_module_evaluation(&db, project, b)?;
    }
    Ok((python_module_evaluation(&db, project, external)?, external))
}

#[test]
fn external_star_import_of_cycle_is_entry_order_independent() {
    let (a_first, a_external) =
        external_star_cycle_product(true).expect("external star-cycle fixture should build");
    let (b_first, b_external) =
        external_star_cycle_product(false).expect("external star-cycle fixture should build");

    assert_eq!(a_first, b_first);
    for (evaluation, external) in [(&a_first, a_external), (&b_first, b_external)] {
        assert!(matches!(
            evaluation.namespace_unknowns.as_slice(),
            [unknown]
                if unknown.cause == PythonUnknownCauseView::Cycle
                    && unknown.origins.as_slice() == [Origin::new(
                        external,
                        expected_span("from a import *\n", "from a import *").expect("test source should contain expected text"),
                    )]
        ));
    }
}

#[test]
fn python_module_cycle_preserves_stable_side_dependencies() {
    let db = TestDatabase::new();
    db.add_file("/project/side.py", "SIDE = 'stable'\n")
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
    let side = db
        .file(Utf8Path::new("/project/side.py"))
        .expect("settings-extraction test file should exist");
    let evaluation =
        python_module_evaluation(&db, project, a).expect("Python file should map to a module");

    assert!(evaluation.dependency_files.contains(&side));
    assert!(evaluation.imports.iter().any(
        |outcome| matches!(outcome, PythonImportOutcomeView::Resolved { file, .. } if *file == side)
    ));
}

#[test]
fn python_module_cycle_keeps_local_mutations_without_copying_dynamic_star_mutations() {
    fn products(
        query_a_first: bool,
    ) -> Result<(PythonModuleEvaluationView, PythonModuleEvaluationView), Box<dyn std::error::Error>>
    {
        let db = TestDatabase::new();
        db.add_file(
            "/project/a.py",
            "from b import *\nVALUES_A = []\nVALUES_A.append('a')\n",
        )?;
        db.add_file(
            "/project/b.py",
            "from a import *\nVALUES_B = []\nVALUES_B.append('b')\n",
        )?;
        let project = python_project(&db);
        let a = db.file(Utf8Path::new("/project/a.py"))?;
        let b = db.file(Utf8Path::new("/project/b.py"))?;
        if query_a_first {
            let _ = python_module_evaluation(&db, project, a)?;
        } else {
            let _ = python_module_evaluation(&db, project, b)?;
        }
        Ok((
            python_module_evaluation(&db, project, a)?,
            python_module_evaluation(&db, project, b)?,
        ))
    }

    let a_first = products(true).expect("first cycle fixture should map each Python file");
    let b_first = products(false).expect("second cycle fixture should map each Python file");
    assert_eq!(a_first, b_first);
    for (evaluation, local, imported) in [
        (&a_first.0, "VALUES_A", "VALUES_B"),
        (&a_first.1, "VALUES_B", "VALUES_A"),
    ] {
        assert!(
            evaluation
                .mutations
                .iter()
                .any(|mutation| mutation.binding == local)
        );
        assert!(
            evaluation
                .mutations
                .iter()
                .all(|mutation| mutation.binding != imported),
            "a dynamic cycle alternative cannot select an imported mutation",
        );
    }
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
fn python_module_long_cycle_stays_within_the_internal_iteration_guard() {
    const MEMBERS: usize = 20;

    let db = TestDatabase::new();
    for index in 0..MEMBERS {
        let next = (index + 1) % MEMBERS;
        db.add_file(
            format!("/project/member_{index}.py").as_str(),
            format!("from member_{next} import VALUE_{next}\nVALUE_{index} = '{index}'\n").as_str(),
        )
        .expect("settings-extraction test file should be added");
    }
    let project = python_project(&db);
    let root = db
        .file(Utf8Path::new("/project/member_0.py"))
        .expect("settings-extraction test file should exist");
    let evaluation =
        python_module_evaluation(&db, project, root).expect("Python file should map to a module");

    let value = evaluation
        .binding("VALUE_0")
        .expect("root assignment should be retained");
    assert!(matches!(
        value.alternatives.as_slice(),
        [PythonBindingAlternativeView::Bound(PythonBoundValueView {
            value: PythonValueView {
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

// Phase 4.1 branch/loop/cycle coverage.
//
// Several Phase 4.1 bullets are already covered by Phase 3 tests and are reused
// here rather than duplicated:
//   * two-file direct cycle -> `ordinary_import_two_file_cycle_binds_handle_and_records_cycle_edge`
//   * dotted self direct import -> `ordinary_dotted_import_self_cycle_binds_root_and_records_leaf_cycle`
//     and `ordinary_dotted_import_self_cycle_attaches_open_child`
//   * post-cycle exact locals -> `python_module_cycle_widens_cyclic_values_but_keeps_post_cycle_assignments`
//   * stable side dependencies -> `python_module_cycle_preserves_stable_side_dependencies`
//   * long-cycle guard -> `python_module_long_cycle_stays_within_the_internal_iteration_guard`
//   * from-import opposite entry order -> `python_module_cycle_products_are_entry_order_independent`
// The tests below add the genuinely missing cases: child-coordinate branch
// correlation, zero-iteration loop uncertainty, non-dotted self import, mixed
// direct/from cycle, parent-package component cycle, dotted-chain cycle,
// ordinary-import opposite entry order, and unrelated child-coordinate survival.

fn child_attribute_alternative_causes(
    binding: &djls_project::testing::PythonBindingView,
) -> Vec<PythonUnknownCauseView> {
    binding
        .alternatives
        .iter()
        .filter_map(|alternative| match alternative {
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value:
                    PythonValueView {
                        kind: PythonValueKindView::Unknown(unknown),
                        ..
                    },
                ..
            }) => Some(unknown.cause.clone()),
            PythonBindingAlternativeView::Bound(_) | PythonBindingAlternativeView::Unbound => None,
        })
        .collect()
}

fn binding_has_module(
    binding: &djls_project::testing::PythonBindingView,
    module_name: &str,
) -> bool {
    binding.alternatives.iter().any(|alternative| {
        matches!(alternative,
            PythonBindingAlternativeView::Bound(PythonBoundValueView {
                value: PythonValueView {
                    kind: PythonValueKindView::Module(PythonModuleView::Source(module)),
                    ..
                },
                ..
            }) if module.as_str() == module_name)
    })
}

#[test]
fn ordinary_dotted_import_branch_attachment_correlates_present_and_absent_child() {
    // The parent `pkg` is always imported; the child `pkg.sub` is attached only
    // on the taken branch. Reading `pkg.sub` afterward correlates the present
    // module handle with the absent-branch module-attribute unknown.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg\nif condition:\n    import pkg.sub\nCHILD = pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    let child = evaluation
        .binding("CHILD")
        .expect("CHILD should be tracked");
    assert!(
        binding_has_module(child, "pkg.sub"),
        "the taken branch attaches the child handle: {:?}",
        child.alternatives,
    );
    assert!(
        child_attribute_alternative_causes(child).iter().any(
            |cause| matches!(cause, PythonUnknownCauseView::ModuleAttribute { module, member }
                if module.as_str() == "pkg" && member == "sub")
        ),
        "the skipped branch leaves the child absent as a module-attribute unknown: {:?}",
        child.alternatives,
    );
}

#[test]
fn ordinary_dotted_import_zero_iteration_loop_leaves_child_uncertain() {
    // A dotted child imported only inside a loop body has a zero-iteration
    // baseline where the child is never attached, so reading it afterward retains
    // an uncertain (absent) module-attribute alternative alongside the attached
    // handle from the iterating path.
    let (_file, evaluation) = evaluate_module_with(
        &[
            ("/project/pkg/__init__.py", ""),
            ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
        ],
        "import pkg\nfor item in ITEMS:\n    import pkg.sub\nCHILD = pkg.sub\n",
    )
    .expect("multi-file Python evaluation fixture should build");

    let child = evaluation
        .binding("CHILD")
        .expect("CHILD should be tracked");
    let causes = child_attribute_alternative_causes(child);
    // The coordinate the loop body attaches degrades to `UnsupportedExpression`
    // because the loop may run zero times, so no stable module handle survives.
    assert!(
        !binding_has_module(child, "pkg.sub"),
        "the zero-iteration loop must not present a certain child handle: {:?}",
        child.alternatives,
    );
    assert!(
        causes
            .iter()
            .any(|cause| matches!(cause, PythonUnknownCauseView::UnsupportedExpression)),
        "the loop-changed child coordinate degrades to UnsupportedExpression: {:?}",
        child.alternatives,
    );
    assert!(
        causes.iter().any(
            |cause| matches!(cause, PythonUnknownCauseView::ModuleAttribute { module, member }
                if module.as_str() == "pkg" && member == "sub")
        ),
        "the zero-iteration baseline keeps the child absent as a module-attribute unknown: {:?}",
        child.alternatives,
    );
}

#[test]
fn ordinary_import_non_dotted_self_cycle_binds_handle_and_records_cycle_edge() {
    // A module that imports itself with a bare `import a` records a self cycle
    // edge and still retains its post-import local value.
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "import a\nVALUE = 'a'\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("a").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Cycle { imported_module, .. }
                if imported_module.as_str() == "a"
        )),
        "a bare self import records a self cycle edge: {:?}",
        evaluation.imports,
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(&value.value.kind, PythonValueKindView::Str(text) if text == "a"));
}

#[test]
fn mixed_direct_and_from_cycle_records_cycle_on_both_forms() {
    // `a` reaches `b` through a bare `import b`; `b` reaches back through
    // `from a import ...`. Both source forms enter the shared chain lifecycle and
    // record cycle edges regardless of which module is evaluated first.
    let evaluate = |root: &str| {
        let db = TestDatabase::new();
        db.add_file("/project/a.py", "import b\nVALUE_A = 'a'\n")
            .expect("settings-extraction test file should be added");
        db.add_file(
            "/project/b.py",
            "from a import VALUE_A\nVALUE_B = VALUE_A\n",
        )
        .expect("settings-extraction test file should be added");
        let project = python_project(&db);
        let module = PythonSourceModule::resolve(
            &db,
            project,
            PythonModuleName::parse(root).expect("test Python module name should be valid"),
        )
        .expect("test Python module should resolve");
        python_module_evaluation_for_module(&db, project, module)
    };

    let from_a = evaluate("a");
    let from_b = evaluate("b");

    let edges = |evaluation: &PythonModuleEvaluationView| {
        evaluation
            .imports
            .iter()
            .filter_map(|outcome| match outcome {
                PythonImportOutcomeView::Cycle {
                    importer_module,
                    imported_module,
                    ..
                }
                | PythonImportOutcomeView::Resolved {
                    importer_module,
                    imported_module,
                    ..
                } => Some((
                    importer_module.as_str().to_string(),
                    imported_module.as_str().to_string(),
                )),
                PythonImportOutcomeView::InvalidImport { .. }
                | PythonImportOutcomeView::NotFound { .. }
                | PythonImportOutcomeView::SkippedExternal { .. }
                | PythonImportOutcomeView::Unreadable { .. }
                | PythonImportOutcomeView::SyntaxErrors { .. } => None,
            })
            .collect::<BTreeSet<_>>()
    };

    // Both cycle participants record the same canonical mixed-form edge set with
    // exactly one cycle edge, independent of which module is evaluated.
    for evaluation in [&from_a, &from_b] {
        assert_eq!(
            evaluation
                .imports
                .iter()
                .filter(|outcome| matches!(outcome, PythonImportOutcomeView::Cycle { .. }))
                .count(),
            1,
        );
    }
    assert_eq!(edges(&from_a), edges(&from_b));
    let recorded = edges(&from_a);
    assert!(
        recorded.contains(&("a".to_string(), "b".to_string())),
        "the direct import edge a->b is recorded: {recorded:?}",
    );
    assert!(
        recorded.contains(&("b".to_string(), "a".to_string())),
        "the from-import edge b->a is recorded: {recorded:?}",
    );
    // The direct import still binds the module handle across the cycle.
    assert!(
        binding_has_module(from_a.binding("b").expect("b should be bound"), "b"),
        "the direct import binds the module handle across the cycle: {:?}",
        from_a.binding("b"),
    );
}

#[test]
fn parent_package_component_cycle_records_the_parent_edge() {
    // Evaluating `pkg.sub` imports the parent package `pkg`, whose `__init__`
    // reads back into `pkg.sub`, closing a cycle through the parent component.
    // The recorded edges must include the parent `pkg` component.
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "from pkg.sub import X\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/sub.py", "import pkg\nX = 'x'\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("pkg.sub").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Cycle { imported_module, .. } | PythonImportOutcomeView::Resolved { imported_module, .. }
                if imported_module.as_str() == "pkg"
        )),
        "the parent package component appears in the recorded edges: {:?}",
        evaluation.imports,
    );
    assert!(
        evaluation
            .imports
            .iter()
            .any(|outcome| matches!(outcome, PythonImportOutcomeView::Cycle { .. })),
        "the parent-package cycle records a cycle edge: {:?}",
        evaluation.imports,
    );
    let value = bound_value(&evaluation, "X").expect("binding should have one bound alternative");
    assert!(matches!(&value.value.kind, PythonValueKindView::Str(text) if text == "x"));
}

#[test]
fn dotted_chain_cycle_records_leaf_cycle_and_resolves_prefix() {
    // A three-deep dotted chain `a.b.c` importing itself resolves the `a` and
    // `a.b` prefix components and records the leaf `a.b.c` as a cycle edge.
    let db = TestDatabase::new();
    db.add_file("/project/a/__init__.py", "")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/a/b/__init__.py", "")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/a/b/c.py", "import a.b.c\nVALUE = 'c'\n")
        .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("a.b.c").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    for prefix in ["a", "a.b"] {
        assert!(
            evaluation.imports.iter().any(|outcome| matches!(
                outcome,
                PythonImportOutcomeView::Resolved { imported_module, .. }
                    if imported_module.as_str() == prefix
            )),
            "the prefix component {prefix} resolves: {:?}",
            evaluation.imports,
        );
    }
    assert!(
        evaluation.imports.iter().any(|outcome| matches!(
            outcome,
            PythonImportOutcomeView::Cycle { imported_module, .. }
                if imported_module.as_str() == "a.b.c"
        )),
        "the dotted leaf is a cycle edge: {:?}",
        evaluation.imports,
    );
    let value =
        bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative");
    assert!(matches!(&value.value.kind, PythonValueKindView::Str(text) if text == "c"));
}

fn direct_cycle_products(
    query_a_first: bool,
) -> Result<(PythonModuleEvaluationView, PythonModuleEvaluationView), Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    db.add_file("/project/a.py", "import b\nVALUE_A = 'a'\n")?;
    db.add_file("/project/b.py", "import a\nVALUE_B = 'b'\n")?;
    let project = python_project(&db);
    let a = db.file(Utf8Path::new("/project/a.py"))?;
    let b = db.file(Utf8Path::new("/project/b.py"))?;

    if query_a_first {
        let _ = python_module_evaluation(&db, project, a)?;
    } else {
        let _ = python_module_evaluation(&db, project, b)?;
    }
    Ok((
        python_module_evaluation(&db, project, a)?,
        python_module_evaluation(&db, project, b)?,
    ))
}

#[test]
fn ordinary_import_cycle_products_are_entry_order_independent() {
    let a_first =
        direct_cycle_products(true).expect("direct cycle evaluation fixture should build");
    let b_first =
        direct_cycle_products(false).expect("direct cycle evaluation fixture should build");

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
fn ordinary_import_cycle_widening_preserves_unrelated_child_coordinate() {
    // `root` attaches two children under `pkg`: the stable `pkg.stable` and the
    // cyclic `pkg.cyc` (which imports `root`). Cycle widening degrades only the
    // cyclic coordinate; the unrelated stable coordinate survives as a handle.
    let db = TestDatabase::new();
    db.add_file("/project/pkg/__init__.py", "")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/stable.py", "LEAF = 'stable'\n")
        .expect("settings-extraction test file should be added");
    db.add_file("/project/pkg/cyc.py", "import root\nLEAF = 'cyc'\n")
        .expect("settings-extraction test file should be added");
    db.add_file(
        "/project/root.py",
        "import pkg.stable\nimport pkg.cyc\nSTABLE = pkg.stable\nCYC = pkg.cyc\n",
    )
    .expect("settings-extraction test file should be added");
    let project = python_project(&db);
    let module = PythonSourceModule::resolve(
        &db,
        project,
        PythonModuleName::parse("root").expect("test Python module name should be valid"),
    )
    .expect("test Python module should resolve");
    let evaluation = python_module_evaluation_for_module(&db, project, module);

    let stable = evaluation
        .binding("STABLE")
        .expect("STABLE should be bound");
    assert!(
        binding_has_module(stable, "pkg.stable"),
        "the unrelated stable child coordinate survives widening: {:?}",
        stable.alternatives,
    );
    assert!(
        evaluation
            .imports
            .iter()
            .any(|outcome| matches!(outcome, PythonImportOutcomeView::Cycle { .. })),
        "the cyclic child records a cycle edge: {:?}",
        evaluation.imports,
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
fn handler_if_uses_stable_arms_across_try_prefixes() {
    let source = "try:\n    B = True\n    MARKER = 1\nexcept Exception:\n    if A:\n        VALUE = 'a'\n    elif B:\n        VALUE = 'b'\n    else:\n        VALUE = 'c'\n";
    let (_file, evaluation) =
        evaluate_module(source).expect("Python evaluation fixture should build");
    let values = evaluation
        .binding("VALUE")
        .expect("handler binding should exist")
        .alternatives
        .iter()
        .filter_map(|alternative| {
            let PythonBindingAlternativeView::Bound(bound) = alternative else {
                return None;
            };
            let PythonValueKindView::Str(value) = &bound.value.kind else {
                return None;
            };
            Some(value.as_str())
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(values, ["a", "b", "c"].into_iter().collect());
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

    let origin = binding_unknown_origin(source, "INSTALLED_APPS")
        .expect("unknown binding origin should be extracted");
    assert_eq!(
        &source[origin.span.start_usize()..origin.span.end_usize()],
        "INSTALLED_APPS.clear()",
    );
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
fn simple_mutable_alias_mutation_keeps_source_setting_conservative() {
    let settings = extract("INSTALLED_APPS = []\nAPPS = INSTALLED_APPS\nAPPS.append('blog')")
        .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
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
fn nested_augmented_assignment_invalidates_mutable_aliases() {
    let settings = extract(
        "INSTALLED_APPS = []\nWRAPPER = {'apps': INSTALLED_APPS}\nWRAPPER['apps'] += ['blog']",
    )
    .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn repeated_nested_aliases_in_the_mutated_root_become_conservative() {
    let settings = extract(
        "DIRS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': DIRS},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': DIRS},\n]\nTEMPLATES[0]['DIRS'].append('/shared')",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/templates/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
}

#[test]
fn mutating_multi_case_bindings_invalidates_their_aliases() {
    let settings = extract(
        "if FLAG:\n    BASE = ['first']\nelse:\n    BASE = ['second']\nINSTALLED_APPS = BASE\nBASE.append('blog')",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

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
fn uncertain_dictionary_overrides_invalidate_possible_mutation_aliases() {
    let settings = extract(
        "INSTALLED_APPS = []\nEXTRA = make_mapping()\nWRAPPER = {'apps': INSTALLED_APPS, **EXTRA}\nWRAPPER['apps'].append('blog')",
    ).expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

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
    let settings = extract("INSTALLED_APPS = ('first',)\nINSTALLED_APPS.append('second')")
        .expect("Django settings extraction should succeed");
    let cases = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array");

    assert_eq!(cases.len(), 1, "{settings:#}");
    assert!(cases[0].get("dynamic").is_some(), "{settings:#}");
    assert!(cases[0].get("known").is_none(), "{settings:#}");
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
fn chained_immutable_assignment_remains_exact() {
    let (_, evaluation) = evaluate_module("VALUE = ALIAS = '/static/'")
        .expect("Python module evaluation fixture should build");

    for name in ["VALUE", "ALIAS"] {
        assert!(matches!(
            &bound_value(&evaluation, name).expect("binding should have one bound alternative").value.kind,
            PythonValueKindView::Str(value) if value == "/static/"
        ));
    }
}

#[test]
fn chained_immutable_tuple_assignment_remains_exact() {
    let settings = extract("INSTALLED_APPS = APPS = ('first', 'second')")
        .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "first");
    assert_eq!(apps[1]["value"], "second");
}

#[test]
fn tuple_installed_apps_are_accepted_like_lists() {
    let settings = extract("INSTALLED_APPS = ('first', 'second')")
        .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");

    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "first");
    assert_eq!(apps[1]["value"], "second");
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
fn binary_tuple_plus_tuple_preserves_exact_installed_apps() {
    let settings = extract("INSTALLED_APPS = ('alpha',) + ('beta',)")
        .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "alpha");
    assert_eq!(apps[1]["value"], "beta");
}

#[test]
fn binary_string_plus_string_is_exact() {
    let (_, evaluation) = evaluate_module("VALUE = '/sta' + 'tic/'")
        .expect("Python module evaluation fixture should build");

    assert!(matches!(
        &bound_value(&evaluation, "VALUE").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Str(value) if value == "/static/"
    ));
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
fn name_target_tuple_augmented_add_tuple_is_exact() {
    let settings = extract("INSTALLED_APPS = ('alpha',)\nINSTALLED_APPS += ('beta',)")
        .expect("Django settings extraction should succeed");
    let apps = cases(&settings, "/installed_apps/cases")
        .expect("settings JSON pointer should identify an array")[0]["known"]["apps"]
        .as_array()
        .expect("expected JSON field should be an array");
    assert_eq!(apps.len(), 2, "{settings:#}");
    assert_eq!(apps[0]["value"], "alpha");
    assert_eq!(apps[1]["value"], "beta");
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
fn name_target_string_augmented_add_is_exact_or_degrades() {
    let (_, exact) = evaluate_module("VALUE = '/sta'\nVALUE += 'tic/'")
        .expect("Python module evaluation fixture should build");
    assert!(matches!(
        &bound_value(&exact, "VALUE").expect("binding should have one bound alternative").value.kind,
        PythonValueKindView::Str(value) if value == "/static/"
    ));

    for source in [
        "VALUE = '/static/'\nVALUE += EXTRA",
        "from pathlib import Path\nVALUE = '/static/'\nVALUE += Path(__file__)",
    ] {
        let (_, degraded) =
            evaluate_module(source).expect("Python module evaluation fixture should build");
        assert!(has_unknown_alternative(&degraded, "VALUE"), "{degraded:#?}");
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
fn chained_container_assignment_invalidates_nested_mutable_sources() {
    let (_, evaluation) = evaluate_module(
        "DIRS = []\nWRAPPER = {'DIRS': DIRS}\nLEFT = RIGHT = WRAPPER\nRIGHT['DIRS'].append('/templates')",
    ).expect("Python module evaluation fixture should build");

    assert!(
        has_unknown_alternative(&evaluation, "DIRS"),
        "{evaluation:#?}"
    );
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
fn canonical_unknown_origins_merge_top_level_branch_spans() {
    let source = "if FLAG:\n    VALUE = first()\nelse:\n    VALUE = second()\n";
    let (_, evaluation) =
        evaluate_module(source).expect("Python module evaluation fixture should build");
    let bound = bound_value(&evaluation, "VALUE").expect("VALUE should have one bound alternative");
    let unknown = match &bound.value.kind {
        PythonValueKindView::Unknown(unknown) => Some(unknown),
        PythonValueKindView::Str(_)
        | PythonValueKindView::Bool(_)
        | PythonValueKindView::Path(_)
        | PythonValueKindView::UnsupportedLiteral
        | PythonValueKindView::List(_)
        | PythonValueKindView::Tuple(_)
        | PythonValueKindView::Dict(_)
        | PythonValueKindView::Module(_) => None,
    }
    .expect("equal unknown branches should produce one unknown");

    assert_eq!(unknown.cause, PythonUnknownCauseView::UnsupportedExpression);
    assert_eq!(
        unknown
            .origins
            .iter()
            .map(|origin| origin.span)
            .collect::<Vec<_>>(),
        [
            expected_span(source, "first()").expect("test source should contain expected text"),
            expected_span(source, "second()").expect("test source should contain expected text"),
        ]
    );
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
