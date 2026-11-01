//! Incremental analysis must match analysis from scratch.
//!
//! One long-lived database receives a random sequence of file edits the way the
//! server applies them: a `SourceChanges` event, then full Django Discovery or
//! intrinsic re-priming as the server's readiness coverage decides. After every
//! edit, each Template's diagnostics must equal those of a fresh database built
//! from the same files. A difference means a memoized result went stale.

use std::collections::BTreeMap;

use camino::Utf8Path;
use djls_ide::PrimedTemplateLibraries;
use djls_ide::prime_template_library_products;
use djls_project::run_django_discovery;
use djls_semantic::collect_template_diagnostics;
use djls_source::ChangeEvent;
use djls_source::Db as _;
use djls_source::SourceChanges;
use djls_source::path_to_file;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use proptest::prelude::*;

const SETTINGS: &str = "/project/settings.py";

/// Each path and the contents it can take. `None` in a state means the file is absent.
const FILES: &[(&str, &[&str])] = &[
    (
        SETTINGS,
        &[
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'OPTIONS': {'builtins': ['project_tags']}}]\n",
            "INSTALLED_APPS = ['app']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'OPTIONS': {}}]\n",
            "INSTALLED_APPS = ['app']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'OPTIONS': {'builtins': ['project_tags'], 'libraries': {'alias': 'app.templatetags.app_tags'}}}]\n",
            "from base_settings import *\n",
            "from base_settings import *\nINSTALLED_APPS = INSTALLED_APPS + ['app']\n",
        ],
    ),
    (
        "/project/base_settings.py",
        &[
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'OPTIONS': {'builtins': ['project_tags']}}]\n",
            "INSTALLED_APPS = ['app']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'APP_DIRS': True, 'OPTIONS': {}}]\n",
        ],
    ),
    (
        "/project/project_tags.py",
        &[
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello(): pass\n",
            "from django import template\nregister = template.Library()\n@register.tag\ndef panel(parser, token):\n    nodelist = parser.parse(('endpanel',))\n    parser.delete_first_token()\n    return template.Node()\n",
            "from django import template\nregister = template.Library()\n@register.filter\ndef shout(value): pass\n",
            "from django import template\nregister = template.Library(\n",
        ],
    ),
    ("/project/app/__init__.py", &[""]),
    ("/project/app/templatetags/__init__.py", &[""]),
    (
        "/project/app/templatetags/app_tags.py",
        &[
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef widget(value): pass\n",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef widget(): pass\n@register.filter\ndef shout(value, arg): pass\n",
            "from django import template\nfrom app.helpers import compile_widget\nregister = template.Library()\nregister.tag('widget', compile_widget)\n",
        ],
    ),
    (
        "/project/app/helpers.py",
        &[
            "from django import template\ndef compile_widget(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 2:\n        raise template.TemplateSyntaxError('widget takes one argument')\n    return template.Node()\n",
            "from django import template\ndef compile_widget(parser, token):\n    return template.Node()\n",
        ],
    ),
    (
        "/project/templates/page.html",
        &[
            "{% load app_tags %}{% widget 1 %}{% hello %}{% panel %}",
            "{% load alias %}{% widget %}{{ x|shout }}{{ x|shout:1 }}",
            "{% extends 'base.html' %}{% block content %}{% panel %}{% endpanel %}{% endblock %}",
            "{% load unknown %}{% block a %}{% widget 1 2 %}",
        ],
    ),
    (
        "/project/templates/base.html",
        &[
            "{% block content %}{% endblock %}",
            "{% load app_tags %}{% block other %}{% widget %}",
        ],
    ),
    (
        "/project/app/templates/app.html",
        &["{% load app_tags alias %}{% widget %}{% hello %}"],
    ),
];

type State = Vec<Option<usize>>;

/// One edit: set `FILES[file]` to the given variant, or delete it.
#[derive(Clone)]
struct Edit {
    file: usize,
    variant: Option<usize>,
}

impl std::fmt::Debug for Edit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (path, variants) = FILES[self.file];
        match self.variant {
            Some(variant) => write!(f, "write {path} = {:?}", variants[variant]),
            None => write!(f, "delete {path}"),
        }
    }
}

/// Mostly present files, so edits land in projects where dependency chains are intact.
fn variant(file: usize) -> impl Strategy<Value = Option<usize>> {
    proptest::option::weighted(0.85, 0..FILES[file].1.len())
}

fn state() -> impl Strategy<Value = State> {
    (0..FILES.len()).map(variant).collect::<Vec<_>>()
}

/// Python sources carry the invalidation chains (settings, star imports, helpers), so they are
/// edited twice as often as Templates.
fn edit() -> impl Strategy<Value = Edit> {
    let targets: Vec<usize> = (0..FILES.len())
        .flat_map(|file| {
            let weight = if Utf8Path::new(FILES[file].0).extension() == Some("py") {
                2
            } else {
                1
            };
            std::iter::repeat_n(file, weight)
        })
        .collect();
    proptest::sample::select(targets)
        .prop_flat_map(|file| variant(file).prop_map(move |variant| Edit { file, variant }))
}

/// The present files and their contents, for failure messages.
fn describe(state: &State) -> Vec<String> {
    FILES
        .iter()
        .zip(state)
        .filter_map(|((path, variants), variant)| {
            variant.map(|variant| format!("{path} = {:?}", variants[variant]))
        })
        .collect()
}

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn install(state: &State) -> TestResult<TestDatabase> {
    let mut db = TestDatabase::new();
    let mut fixture = ProjectFixture::new("/project").django_settings_module("settings");
    for (file, variant) in state.iter().enumerate() {
        if let Some(variant) = variant {
            let (path, variants) = FILES[file];
            fixture = fixture.file(path, variants[*variant]);
        }
    }
    fixture.install(&mut db)?;
    run_django_discovery(&mut db)?;
    Ok(db)
}

/// Every present Template's diagnostics, in a comparable form.
fn diagnostics(
    db: &TestDatabase,
    state: &State,
) -> TestResult<BTreeMap<&'static str, Vec<String>>> {
    FILES
        .iter()
        .zip(state)
        .filter(|((path, _), variant)| {
            Utf8Path::new(path).extension() == Some("html") && variant.is_some()
        })
        .map(|((path, _), _)| {
            let file = path_to_file(db, Utf8Path::new(path))?;
            let found = collect_template_diagnostics(db, file);
            let mut rendered: Vec<String> = found
                .template_errors
                .iter()
                .map(|error| format!("{error:?}"))
                .chain(
                    found
                        .validation_errors
                        .iter()
                        .map(|error| format!("{error:?}")),
                )
                .collect();
            rendered.sort();
            Ok((*path, rendered))
        })
        .collect()
}

/// Apply one edit the way the server does after a file event.
fn apply(
    db: &mut TestDatabase,
    primed: &mut Option<PrimedTemplateLibraries>,
    state: &mut State,
    edit: &Edit,
) -> TestResult<()> {
    let (path, variants) = FILES[edit.file];
    let before = state[edit.file];
    let event = match (before, edit.variant) {
        (_, Some(variant)) => {
            db.add_file(path, variants[variant])?;
            if before.is_some() {
                ChangeEvent::ContentChanged(path.into())
            } else {
                ChangeEvent::BecameVisible(path.into())
            }
        }
        (Some(_), None) => {
            db.remove_file(path)?;
            ChangeEvent::Deleted(path.into())
        }
        (None, None) => return Ok(()),
    };
    state[edit.file] = edit.variant;
    let membership_changed = matches!(
        event,
        ChangeEvent::BecameVisible(_) | ChangeEvent::Deleted(_)
    );
    SourceChanges::new([event]).apply(db);

    let covers = |files: &[djls_source::File]| {
        files
            .iter()
            .any(|file| file.path(db) == Utf8Path::new(path))
    };
    let full_reload = membership_changed
        || primed
            .as_ref()
            .is_none_or(|primed| covers(primed.full_reload_files()));
    let reprime = primed
        .as_ref()
        .is_some_and(|primed| covers(primed.reprime_files()))
        || db.files().root(db, Utf8Path::new(path)).is_some();
    if full_reload {
        run_django_discovery(db)?;
        *primed = prime_template_library_products(db);
    } else if reprime {
        *primed = prime_template_library_products(db);
    }
    Ok(())
}

fn fail(error: impl std::fmt::Display) -> TestCaseError {
    TestCaseError::fail(error.to_string())
}

proptest! {
    // Small enough for every `cargo test`; set PROPTEST_CASES for a deeper run.
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|cases| cases.parse().ok()).unwrap_or(32),
        ..ProptestConfig::default()
    })]

    #[test]
    fn incremental_diagnostics_match_a_fresh_database(
        initial in state(),
        edits in proptest::collection::vec(edit(), 1..14),
    ) {
        let start = describe(&initial);
        let mut state = initial;
        let mut db = install(&state).map_err(fail)?;
        let mut primed = prime_template_library_products(&db);
        // Memoize every Template before the first edit, so stale results can surface.
        diagnostics(&db, &state).map_err(fail)?;

        for (step, edit) in edits.iter().enumerate() {
            apply(&mut db, &mut primed, &mut state, edit).map_err(fail)?;
            let fresh = install(&state).map_err(fail)?;
            prop_assert_eq!(
                diagnostics(&db, &state).map_err(fail)?,
                diagnostics(&fresh, &state).map_err(fail)?,
                "after edit {} of {:#?}\nstarting from {:#?}", step, edits, start
            );
        }
    }
}
