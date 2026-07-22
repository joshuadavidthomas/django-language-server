use std::collections::BTreeSet;
use std::ptr;

use camino::Utf8Path;
use djls_project::*;
use djls_source::ChangeEvent;
use djls_source::SourceChanges;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use salsa::Database as _;
use salsa::Event;
use salsa::EventKind;

fn will_execute_count(db: &TestDatabase, events: &[Event], query_name: &str) -> usize {
    events
        .iter()
        .filter(|event| match &event.kind {
            EventKind::WillExecute { database_key } => db
                .ingredient_debug_name(database_key.ingredient_index())
                .contains(query_name),
            EventKind::DidValidateMemoizedValue { .. }
            | EventKind::WillBlockOn { .. }
            | EventKind::WillIterateCycle { .. }
            | EventKind::DidFinalizeCycle { .. }
            | EventKind::WillCheckCancellation
            | EventKind::DidSetCancellationFlag
            | EventKind::WillDiscardStaleOutput { .. }
            | EventKind::DidDiscard { .. }
            | EventKind::DidDiscardAccumulated { .. }
            | EventKind::DidInternValue { .. }
            | EventKind::DidReuseInternedValue { .. }
            | EventKind::DidValidateInternedValue { .. } => false,
        })
        .count()
}

fn project_with_templates(
    db: &mut TestDatabase,
    template_dirs: Vec<&str>,
    templates: Vec<(&str, &str, &str)>,
) -> Result<Project, Box<dyn std::error::Error>> {
    let dirs_literal = template_dirs
        .into_iter()
        .map(|dir| format!("'{dir}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let settings_source = format!(
        "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{dirs_literal}], 'APP_DIRS': False}}]\n"
    );
    let fixture = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings_source);
    Ok(templates
        .into_iter()
        .fold(fixture, |fixture, (_name, path, source)| {
            fixture.file(path, source)
        })
        .build(db)?)
}

#[test]
fn scoped_template_libraries_correlate_with_resolving_backends() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha():\n    pass\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef beta():\n    pass\n",
        )
        .file("/test/project/a/alpha.html", "{% load shared %}")
        .file("/test/project/b/beta.html", "{% load shared %}")
        .file("/test/project/outside.html", "{% load shared %}")
        .build(&db)
        .expect("backend-correlation project fixture should build");

    let alpha_file = db
        .file(Utf8Path::new("/test/project/a/alpha.html"))
        .expect("backend A template fixture should exist in the test database");
    let beta_file = db
        .file(Utf8Path::new("/test/project/b/beta.html"))
        .expect("backend B template fixture should exist in the test database");
    let outside_file = db
        .file(Utf8Path::new("/test/project/outside.html"))
        .expect("outside template fixture should exist in the test database");
    let alpha = scoped_template_libraries(&db, project, alpha_file)
        .loadable_library_str("shared")
        .found()
        .expect("backend A should provide shared");
    let beta = scoped_template_libraries(&db, project, beta_file)
        .loadable_library_str("shared")
        .found()
        .expect("backend B should provide shared");

    assert_eq!(alpha.module_name_str(), "alpha_tags");
    assert_eq!(beta.module_name_str(), "beta_tags");

    let catalog = template_library_catalog(&db, project);
    let inventory = ScopedTemplateLibraries::from_project_inventory(catalog);
    let outside = scoped_template_libraries(&db, project, outside_file);
    assert!(matches!(
        inventory.loadable_library_str("shared"),
        LoadableLibraryLookup::Ambiguous(libraries) if libraries.len() == 2
    ));
    assert!(matches!(
        outside.loadable_library_str("shared"),
        LoadableLibraryLookup::Ambiguous(libraries)
            if libraries.iter().any(|library| library.module_name_str() == "alpha_tags")
                && libraries.iter().any(|library| library.module_name_str() == "beta_tags")
    ));
    let catalog_alpha = inventory
        .resolved_libraries()
        .into_iter()
        .find(|library| library.module_name_str() == "alpha_tags")
        .expect("the shared catalog should contain backend A's library");
    let catalog_beta = inventory
        .resolved_libraries()
        .into_iter()
        .find(|library| library.module_name_str() == "beta_tags")
        .expect("the shared catalog should contain backend B's library");
    assert!(
        ptr::eq(alpha, catalog_alpha),
        "a file scope must borrow backend A's library from the shared catalog"
    );
    assert!(
        ptr::eq(beta, catalog_beta),
        "a file scope must borrow backend B's library from the shared catalog"
    );
}

#[test]
fn project_inventory_preserves_backend_remainder_slot_order() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'builtins': ['alpha_builtin']}},\n    *UNKNOWN,\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'builtins': ['beta_builtin']}},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_builtin.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef marker(): pass\n",
        )
        .file(
            "/test/project/beta_builtin.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef marker(): pass\n",
        )
        .build(&db)
        .expect("backend-remainder project fixture should build");

    let definitions =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .effective_definition_libraries("marker", TemplateSymbolKind::Tag, &[]);

    assert!(matches!(
        definitions.as_slice(),
        [
            EffectiveDefinitionLibrary::Known(Some(alpha)),
            EffectiveDefinitionLibrary::Unknown,
            EffectiveDefinitionLibrary::Known(Some(beta)),
        ] if alpha.module_name_str() == "alpha_builtin"
            && beta.module_name_str() == "beta_builtin"
    ));
}

#[test]
fn partial_known_backend_field_uncertainty_keeps_one_correlated_backend_alternative() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', unknown_key: 'maybe', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/templates/page.html", "{% load shared %}")
        .build(&db)
        .expect("partial-backend project fixture should build");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("partial-backend template fixture should exist in the test database");

    let scoped_libraries = scoped_template_libraries(&db, project, file);
    assert!(matches!(
        scoped_libraries.loadable_library_str("shared"),
        LoadableLibraryLookup::Inconclusive(candidates)
            if candidates.len() == 1 && candidates[0].module_name_str() == "alpha_tags"
    ));
    assert_eq!(
        scoped_libraries.library_chains(&["shared"]).len(),
        1,
        "backend-field uncertainty must not fabricate a settings-case remainder alternative"
    );
}

#[test]
fn scoped_template_libraries_backend_index_invalidates_with_settings_evidence() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    let settings_path = "/test/project/testproject/settings.py";
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(settings_path, settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/templates/page.html", "{% load shared %}")
        .build(&db)
        .expect("invalidating-scope project fixture should build");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("invalidating-scope template fixture should exist in the test database");

    let initial = scoped_template_libraries(&db, project, file)
        .loadable_library_str("shared")
        .found()
        .expect("initial direct file scope should select alpha");
    assert_eq!(initial.module_name_str(), "alpha_tags");
    drop(
        events
            .take()
            .expect("Salsa event log should be readable before the settings edit"),
    );

    db.add_file(
        settings_path,
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/other-templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n",
    )
    .expect("updated settings fixture should be added to the test database");
    SourceChanges::new([ChangeEvent::ContentChanged(settings_path.into())]).apply(&mut db);

    let updated = scoped_template_libraries(&db, project, file)
        .loadable_library_str("shared")
        .found()
        .expect("invalidated direct file scope should select beta");
    assert_eq!(updated.module_name_str(), "beta_tags");
    let executed = events
        .take()
        .expect("Salsa event log should be readable after the settings edit");
    assert_eq!(
        will_execute_count(&db, &executed, "template_directory_index"),
        1
    );
    assert_eq!(
        will_execute_count(&db, &executed, "template_library_scope"),
        1
    );
}

#[test]
fn file_outside_template_roots_uses_open_project_inventory() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'missing.shared_tags'}}}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/outside.html", "{% load shared %}")
        .build(&db)
        .expect("outside-template project fixture should build");
    let file = db
        .file(Utf8Path::new("/test/project/outside.html"))
        .expect("outside-template fixture should exist in the test database");
    let scoped_libraries = scoped_template_libraries(&db, project, file);

    let library = match scoped_libraries.loadable_library_str("shared") {
        LoadableLibraryLookup::Found(library) => Some(library),
        LoadableLibraryLookup::Ambiguous(_)
        | LoadableLibraryLookup::Inconclusive(_)
        | LoadableLibraryLookup::Absent => None,
    }
    .expect("project inventory should remain available outside configured roots");
    assert_eq!(library.module_name_str(), "missing.shared_tags");
    assert!(library.source_file().is_none());
    assert!(library.symbols_are_unobserved());
    assert_eq!(
        scoped_libraries.symbol("possibly_defined", TemplateSymbolKind::Tag),
        ScopedTemplateSymbolLookup::Inconclusive
    );
}

#[test]
fn locally_assembled_dirs_and_installed_apps_stay_on_the_same_branch() {
    let db = TestDatabase::new();
    let settings = "if FLAG:\n    from one.values import ROOT, APPS\nelse:\n    from two.values import ROOT, APPS\nINSTALLED_APPS = APPS\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [ROOT], 'APP_DIRS': True}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/one/values.py",
            "ROOT = '/test/project/one-templates'\nAPPS = ['first']\n",
        )
        .file(
            "/test/project/two/values.py",
            "ROOT = '/test/project/two-templates'\nAPPS = ['second']\n",
        )
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templatetags/__init__.py", "")
        .file(
            "/test/project/first/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templatetags/__init__.py", "")
        .file(
            "/test/project/second/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/one-templates/page.html", "{% load shared %}")
        .build(&db)
        .expect("correlated-settings project fixture should build");

    let file = db
        .file(Utf8Path::new("/test/project/one-templates/page.html"))
        .expect("correlated-settings template fixture should exist in the test database");
    let library = scoped_template_libraries(&db, project, file)
        .loadable_library_str("shared")
        .found()
        .expect("the template must inherit libraries only from its feasible app branch");

    assert_eq!(library.module_name_str(), "first.templatetags.shared");
}

#[test]
fn scoped_template_libraries_retain_wholly_unknown_settings_case() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = UNKNOWN\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha(value):\n    pass\n",
        )
        .file("/test/project/templates/page.html", "{% load shared %}")
        .build(&db)
        .expect("unknown-settings project fixture should build");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("unknown-settings template fixture should exist in the test database");

    assert!(matches!(
        scoped_template_libraries(&db, project, file).loadable_library_str("shared"),
        LoadableLibraryLookup::Inconclusive(libraries)
            if libraries.iter().any(|library| library.module_name_str() == "alpha_tags")
    ));
}

#[test]
fn duplicate_backend_memberships_preserve_conflicting_libraries() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha():\n    pass\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef beta():\n    pass\n",
        )
        .file("/test/project/shared/page.html", "{% load shared %}")
        .build(&db)
        .expect("duplicate-backend project fixture should build");

    let file = db
        .file(Utf8Path::new("/test/project/shared/page.html"))
        .expect("duplicate-backend template fixture should exist in the test database");
    assert!(matches!(
        scoped_template_libraries(&db, project, file).loadable_library_str("shared"),
        LoadableLibraryLookup::Ambiguous(libraries) if libraries.len() == 2
    ));

    let name = TemplateName::new(&db, "page.html".to_string());
    assert!(matches!(
        template_resolution(&db, project).resolve(&db, name),
        TemplateResolutionResult::Found(_)
    ));
}

#[test]
fn scoped_template_libraries_are_ambiguous_when_settings_cases_resolve_same_file_differently() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha():\n    pass\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef beta():\n    pass\n",
        )
        .file("/test/project/shared/page.html", "{% load shared %}")
        .build(&db)
        .expect("ambiguous-settings project fixture should build");

    let file = db
        .file(Utf8Path::new("/test/project/shared/page.html"))
        .expect("ambiguous-settings template fixture should exist in the test database");
    assert!(matches!(
        scoped_template_libraries(&db, project, file).loadable_library_str("shared"),
        LoadableLibraryLookup::Ambiguous(libraries) if libraries.len() == 2
    ));
}

#[test]
fn template_origins_preserve_django_search_order() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "base.html",
                "/test/project/templates/base.html",
                "project base",
            ),
            (
                "base.html",
                "/test/project/app/templates/base.html",
                "app base",
            ),
            (
                "account/detail.html",
                "/test/project/app/templates/account/detail.html",
                "detail",
            ),
        ],
    )
    .expect("template-search-order project fixture should build");

    let names: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.template_name(&db).name(&db).clone())
        .collect();

    assert_eq!(names, ["base.html", "account/detail.html", "base.html"]);
}

#[test]
fn derived_template_origins_keep_shadowed_names_in_template_dir_order() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "shared.html",
                "/test/project/app/templates/shared.html",
                "app shared",
            ),
            (
                "shared.html",
                "/test/project/templates/shared.html",
                "project shared",
            ),
        ],
    )
    .expect("shadowed-template project fixture should build");

    let paths: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.file(&db).path(&db).as_str())
        .collect();

    assert_eq!(
        paths,
        [
            "/test/project/templates/shared.html",
            "/test/project/app/templates/shared.html",
        ]
    );
}

#[test]
fn template_names_returns_unique_resolvable_names() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "base.html",
                "/test/project/templates/base.html",
                "project base",
            ),
            (
                "base.html",
                "/test/project/app/templates/base.html",
                "app base",
            ),
            (
                "account/detail.html",
                "/test/project/app/templates/account/detail.html",
                "detail",
            ),
        ],
    )
    .expect("template-name project fixture should build");

    let mut names: Vec<_> = template_resolution(&db, project)
        .template_names(&db)
        .map(|name| name.name(&db).clone())
        .collect();
    names.sort();

    assert_eq!(names, ["account/detail.html", "base.html"]);
}

#[test]
fn origins_for_name_returns_origins_in_django_search_order() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "shared.html",
                "/test/project/app/templates/shared.html",
                "app shared",
            ),
            (
                "shared.html",
                "/test/project/templates/shared.html",
                "project shared",
            ),
        ],
    )
    .expect("origin-order project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let paths: Vec<_> = template_resolution(&db, project)
        .origins_for_name(&db, name)
        .iter()
        .map(|origin| origin.file(&db).path(&db).as_str())
        .collect();

    assert_eq!(
        paths,
        [
            "/test/project/templates/shared.html",
            "/test/project/app/templates/shared.html",
        ]
    );
}

#[test]
fn origins_for_name_retains_duplicate_template_names() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "base.html",
                "/test/project/templates/base.html",
                "project base",
            ),
            (
                "base.html",
                "/test/project/app/templates/base.html",
                "app base",
            ),
        ],
    )
    .expect("duplicate-template-name project fixture should build");

    let name = TemplateName::new(&db, "base.html".to_string());
    let origins = template_resolution(&db, project).origins_for_name(&db, name);

    assert_eq!(origins.len(), 2);
}

#[test]
fn origins_for_name_returns_empty_slice_for_unknown_template_name() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        Vec::new(),
    )
    .expect("unknown-template project fixture should build");

    let name = TemplateName::new(&db, "missing.html".to_string());
    let origins = template_resolution(&db, project).origins_for_name(&db, name);

    assert!(origins.is_empty());
}

#[test]
fn template_names_for_file_returns_names_in_discovery_order() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec![
            "/test/project/templates",
            "/test/project/override",
            "/test/project/templates/account",
        ],
        vec![
            (
                "account/detail.html",
                "/test/project/templates/account/detail.html",
                "target",
            ),
            (
                "detail.html",
                "/test/project/override/detail.html",
                "shadow",
            ),
        ],
    )
    .expect("template-file-name project fixture should build");

    let file = db
        .file(Utf8Path::new("/test/project/templates/account/detail.html"))
        .expect("detail template fixture should exist in the test database");
    let names: Vec<_> = template_resolution(&db, project)
        .template_names_for_file(&db, file)
        .iter()
        .map(|name| name.name(&db).as_str())
        .collect();

    assert_eq!(names, ["account/detail.html", "detail.html"]);
}

#[test]
fn resolve_returns_first_origin_for_duplicate_template_names() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "base.html",
                "/test/project/templates/base.html",
                "project base",
            ),
            (
                "base.html",
                "/test/project/app/templates/base.html",
                "app base",
            ),
        ],
    )
    .expect("duplicate-resolution project fixture should build");

    let name = TemplateName::new(&db, "base.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);
    let origin = match result {
        TemplateResolutionResult::Found(origin) => Some(origin),
        TemplateResolutionResult::DoesNotExist(_) | TemplateResolutionResult::Inconclusive(_) => {
            None
        }
    }
    .expect("base.html should resolve to an origin");

    assert_eq!(
        origin.file(&db).path(&db).as_str(),
        "/test/project/templates/base.html"
    );
}

#[test]
fn resolve_reports_missing_template() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        Vec::new(),
    )
    .expect("missing-template project fixture should build");

    let name = TemplateName::new(&db, "missing.html".to_string());
    let error = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::DoesNotExist(error) => Some(error),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::Inconclusive(_) => None,
    }
    .expect("missing.html should not resolve in any template directory");

    assert_eq!(error.name, name);
    assert_eq!(
        error.tried,
        [
            Utf8Path::new("/test/project/templates/missing.html"),
            Utf8Path::new("/test/project/app/templates/missing.html"),
        ]
    );
}

#[test]
fn uncertainty_between_known_directories_weakens_a_later_candidate() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/first', UNKNOWN, '/test/project/later'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/first/other.html", "other")
        .file("/test/project/later/base.html", "later")
        .build(&db)
        .expect("open-directory project fixture should build");

    let name = TemplateName::new(&db, "base.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("directory uncertainty should make the template search inconclusive");

    assert_eq!(search.name, name);
    assert_eq!(search.possible_origins.len(), 1);
    let directories = template_directories(&db, project);
    assert!(directories.settings_cases_may_omit_roots());
    assert_eq!(
        directories.known_roots().collect::<Vec<_>>(),
        [
            Utf8Path::new("/test/project/first"),
            Utf8Path::new("/test/project/later"),
        ]
    );
}

#[test]
fn dynamic_installed_apps_preserve_uncertainty_position() {
    let known_first_db = TestDatabase::new();
    let known_first = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = ['known', UNKNOWN]\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/known/__init__.py", "")
        .file("/test/project/known/templates/base.html", "known")
        .build(&known_first_db)
        .expect("known-first project fixture should build");
    let name = TemplateName::new(&known_first_db, "base.html".to_string());
    assert!(matches!(
        template_resolution(&known_first_db, known_first).resolve(&known_first_db, name),
        TemplateResolutionResult::Found(_)
    ));

    let unknown_first_db = TestDatabase::new();
    let unknown_first = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = [UNKNOWN, 'known']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/known/__init__.py", "")
        .file("/test/project/known/templates/base.html", "known")
        .build(&unknown_first_db)
        .expect("unknown-first project fixture should build");
    let name = TemplateName::new(&unknown_first_db, "base.html".to_string());
    assert!(matches!(
        template_resolution(&unknown_first_db, unknown_first).resolve(&unknown_first_db, name),
        TemplateResolutionResult::Inconclusive(_)
    ));
}

#[test]
fn dynamic_template_backends_preserve_uncertainty_position() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}, UNKNOWN]\n",
        )
        .file("/test/project/templates/base.html", "known")
        .build(&db)
        .expect("unknown-backend project fixture should build");
    let name = TemplateName::new(&db, "base.html".to_string());

    assert!(matches!(
        template_resolution(&db, project).resolve(&db, name),
        TemplateResolutionResult::Found(_)
    ));
}

#[test]
fn wholly_dynamic_dirs_weakens_app_dirs_candidates() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': UNKNOWN, 'APP_DIRS': True}]\n",
        )
        .file("/test/project/blog/__init__.py", "")
        .file("/test/project/blog/templates/base.html", "app base")
        .build(&db)
        .expect("open-app-directories project fixture should build");

    let name = TemplateName::new(&db, "base.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("dynamic DIRS should precede and weaken the APP_DIRS candidate");

    assert_eq!(search.possible_origins.len(), 1);
    assert_eq!(
        search.possible_origins[0].path_buf(&db),
        Utf8Path::new("/test/project/blog/templates/base.html")
    );
}

#[test]
fn scalar_path_alternatives_follow_unanimous_and_divergent_resolution_policy() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/__init__.py", "")
        .file(
            "/test/project/testproject/settings.py",
            "if FLAG:\n    from one.values import SHARED\nelse:\n    from two.values import SHARED\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [SHARED, '/test/project/common'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/one/__init__.py", "")
        .file("/test/project/one/values.py", "SHARED = 'shared'")
        .file("/test/project/two/__init__.py", "")
        .file("/test/project/two/values.py", "SHARED = 'shared'")
        .file("/test/project/one/shared/conflict.html", "one")
        .file("/test/project/two/shared/conflict.html", "two")
        .file("/test/project/common/unanimous.html", "common")
        .build(&db)
        .expect("alternative-directory project fixture should build");

    let unanimous = TemplateName::new(&db, "unanimous.html".to_string());
    let origin = match template_resolution(&db, project).resolve(&db, unanimous) {
        TemplateResolutionResult::Found(origin) => Some(origin),
        TemplateResolutionResult::DoesNotExist(_) | TemplateResolutionResult::Inconclusive(_) => {
            None
        }
    }
    .expect("the same winner in every scalar path alternative should be definitive");
    assert_eq!(
        origin.path_buf(&db),
        Utf8Path::new("/test/project/common/unanimous.html")
    );

    let divergent = TemplateName::new(&db, "conflict.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, divergent) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("different winners across scalar path alternatives should be inconclusive");
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        possible_paths,
        [
            "/test/project/one/shared/conflict.html",
            "/test/project/two/shared/conflict.html",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn divergent_installed_app_alternatives_preserve_all_possible_origins() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templates/shared.html", "first")
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templates/shared.html", "second")
        .build(&db)
        .expect("divergent installed-app project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("alternative installed app lists cannot select a definitive template");
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        possible_paths,
        [
            "/test/project/first/templates/shared.html",
            "/test/project/second/templates/shared.html",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn same_branch_settings_exclude_impossible_template_roots() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if FLAG:\n    INSTALLED_APPS = ['first']\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': True}]\nelse:\n    INSTALLED_APPS = ['second']\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/a/shared.html", "a")
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templates/shared.html", "impossible")
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templates/shared.html", "second")
        .build(&db)
        .expect("same-branch settings project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the two feasible branches should have different winners");
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        possible_paths,
        [
            "/test/project/a/shared.html",
            "/test/project/second/templates/shared.html",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn conditional_star_import_keeps_imported_and_fallback_settings_correlated() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = ['local']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\nfrom .plugin import *\n",
        )
        .file(
            "/test/project/testproject/plugin.py",
            "if ENABLED:\n    INSTALLED_APPS = ['imported']\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/imported-root'], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/imported-root/shared.html", "imported")
        .file("/test/project/imported/__init__.py", "")
        .file(
            "/test/project/imported/templates/shared.html",
            "impossible mixed branch",
        )
        .file("/test/project/local/__init__.py", "")
        .file("/test/project/local/templates/shared.html", "local")
        .build(&db)
        .expect("installed-app alternative project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the imported/imported and local/local branches should have different winners");
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        possible_paths,
        [
            "/test/project/imported-root/shared.html",
            "/test/project/local/templates/shared.html",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn branch_local_namespace_uncertainty_only_pairs_with_its_template_arm() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if FLAG:\n    from missing import *\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/second/templates'], 'APP_DIRS': True}]\nelse:\n    INSTALLED_APPS = ['second']\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templates/shared.html", "second")
        .build(&db)
        .expect("branch-local uncertainty project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let origin = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Found(origin) => Some(origin),
        TemplateResolutionResult::DoesNotExist(_) | TemplateResolutionResult::Inconclusive(_) => {
            None
        }
    }
    .expect("all feasible arms should select the same physical template");
    assert_eq!(
        origin.path_buf(&db).as_str(),
        "/test/project/second/templates/shared.html"
    );
}

#[test]
fn independently_joined_settings_keep_cross_product_feasible_after_branch_translation() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if APPS_FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nif TEMPLATES_FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': True}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/empty'], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/a/shared.html", "a")
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templates/shared.html", "first")
        .file("/test/project/second/__init__.py", "")
        .build(&db)
        .expect("unknown-app-directory project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("independent settings joins should retain both successful cross-product paths");
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        possible_paths,
        [
            "/test/project/a/shared.html",
            "/test/project/first/templates/shared.html",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn branch_specific_apps_correlate_with_equal_relative_template_lists() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if FLAG:\n    from .one.base import TEMPLATES\n    INSTALLED_APPS = ['first']\nelse:\n    from .two.base import TEMPLATES\n    INSTALLED_APPS = ['second']\n",
        )
        .file("/test/project/testproject/one/__init__.py", "")
        .file(
            "/test/project/testproject/one/base.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates'], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/testproject/one/templates/shared.html", "one")
        .file("/test/project/testproject/two/__init__.py", "")
        .file(
            "/test/project/testproject/two/base.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates'], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templates/shared.html", "impossible")
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templates/shared.html", "second")
        .build(&db)
        .expect("branch-specific app project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("the feasible branches should have different winners");
    let possible_paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        possible_paths,
        [
            "/test/project/testproject/one/templates/shared.html",
            "/test/project/second/templates/shared.html",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn common_leading_installed_app_wins_across_alternatives() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if APP_FLAG:\n    INSTALLED_APPS = ['common', 'first']\nelse:\n    INSTALLED_APPS = ['common', 'second']\nif TEMPLATE_FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/first-dirs'], 'APP_DIRS': True}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/second-dirs'], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/first-dirs/other.html", "first dirs")
        .file("/test/project/second-dirs/other.html", "second dirs")
        .file("/test/project/common/__init__.py", "")
        .file("/test/project/common/templates/shared.html", "common")
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templates/shared.html", "first")
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templates/shared.html", "second")
        .build(&db)
        .expect("common-leading-app project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let origin = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Found(origin) => Some(origin),
        TemplateResolutionResult::DoesNotExist(_) | TemplateResolutionResult::Inconclusive(_) => {
            None
        }
    }
    .expect("the common leading app should win in every alternative");

    assert_eq!(
        origin.path_buf(&db),
        Utf8Path::new("/test/project/common/templates/shared.html")
    );
}

#[test]
fn installed_app_found_and_miss_alternatives_are_inconclusive() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templates/shared.html", "first")
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templates/other.html", "other")
        .build(&db)
        .expect("found-and-miss project fixture should build");

    let name = TemplateName::new(&db, "shared.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("a found/miss split should make template resolution inconclusive");

    assert_eq!(search.possible_origins.len(), 1);
    assert_eq!(
        search.possible_origins[0].path_buf(&db),
        Utf8Path::new("/test/project/first/templates/shared.html")
    );
}

#[test]
fn installed_app_alternatives_that_all_miss_report_does_not_exist() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "if FLAG:\n    INSTALLED_APPS = ['first']\nelse:\n    INSTALLED_APPS = ['second']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/first/__init__.py", "")
        .file("/test/project/first/templates/other.html", "first")
        .file("/test/project/second/__init__.py", "")
        .file("/test/project/second/templates/other.html", "second")
        .build(&db)
        .expect("all-missing project fixture should build");

    let name = TemplateName::new(&db, "missing.html".to_string());
    let error = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::DoesNotExist(error) => Some(error),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::Inconclusive(_) => None,
    }
    .expect("exhaustive misses in every alternative should report does not exist");

    assert_eq!(error.name, name);
    assert_eq!(
        error.tried,
        [
            Utf8Path::new("/test/project/first/templates/missing.html"),
            Utf8Path::new("/test/project/second/templates/missing.html"),
        ]
    );
}

#[test]
fn later_dynamic_directory_does_not_weaken_an_earlier_winner() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', UNKNOWN], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/templates/base.html", "base")
        .build(&db)
        .expect("later-dynamic-directory project fixture should build");

    let name = TemplateName::new(&db, "base.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);

    assert!(matches!(result, TemplateResolutionResult::Found(_)));
}

#[test]
fn later_uncertainty_does_not_weaken_an_earlier_winner() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = ['missing']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': True}]\n",
        )
        .file("/test/project/templates/base.html", "base")
        .build(&db)
        .expect("later-uncertainty project fixture should build");

    let name = TemplateName::new(&db, "base.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);

    assert!(matches!(result, TemplateResolutionResult::Found(_)));
}

#[test]
fn missing_template_with_directory_uncertainty_is_inconclusive() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN], 'APP_DIRS': False}]\n",
        )
        .build(&db)
        .expect("uncertain-directory project fixture should build");

    let name = TemplateName::new(&db, "missing.html".to_string());
    let search = match template_resolution(&db, project).resolve(&db, name) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("directory uncertainty should make the missing-template search inconclusive");

    assert_eq!(search.name, name);
    assert!(search.possible_origins.is_empty());
}

#[test]
fn scoped_resolution_and_names_exclude_other_backends() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/page.html", "a")
        .file("/test/project/a/only-a.html", "a")
        .file("/test/project/b/page.html", "b")
        .file("/test/project/b/only-b.html", "b")
        .build(&db)
        .expect("scoped-resolution project fixture should build");
    let resolution = template_resolution(&db, project);
    let page = db
        .file(Utf8Path::new("/test/project/a/page.html"))
        .expect("scoped-resolution template fixture should exist in the test database");
    let only_b = TemplateName::new(&db, "only-b.html".to_string());

    assert!(matches!(
        resolution.resolve_for_file(&db, only_b, page),
        TemplateResolutionResult::DoesNotExist(_)
    ));
    let names = resolution
        .template_names_for_backend_scope(&db, page)
        .into_iter()
        .map(|name| name.name(&db).clone())
        .collect::<Vec<_>>();
    assert!(names.contains(&"only-a.html".to_string()));
    assert!(!names.contains(&"only-b.html".to_string()));
}

#[test]
fn scoped_missing_template_reports_only_selected_backend_roots() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/page.html", "a")
        .file("/test/project/b/other.html", "b")
        .build(&db)
        .expect("scoped-missing project fixture should build");
    let resolution = template_resolution(&db, project);
    let page = db
        .file(Utf8Path::new("/test/project/a/page.html"))
        .expect("scoped-missing template fixture should exist in the test database");
    let missing = TemplateName::new(&db, "missing.html".to_string());

    let error = match resolution.resolve_for_file(&db, missing, page) {
        TemplateResolutionResult::DoesNotExist(error) => Some(error),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::Inconclusive(_) => None,
    }
    .expect("the selected backend should exhaustively miss the template");
    assert_eq!(error.tried, [Utf8Path::new("/test/project/a/missing.html")]);
}

#[test]
fn scoped_resolution_joins_duplicate_backend_memberships() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared', '/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared', '/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/shared/child.html",
            "{% extends 'parent.html' %}",
        )
        .file("/test/project/a/parent.html", "a")
        .file("/test/project/b/parent.html", "b")
        .build(&db)
        .expect("duplicate-membership project fixture should build");
    let resolution = template_resolution(&db, project);
    let child = db
        .file(Utf8Path::new("/test/project/shared/child.html"))
        .expect("duplicate-membership child fixture should exist in the test database");
    let child_name = TemplateName::new(&db, "child.html".to_string());
    let child_origin = resolution
        .origins_for_name(&db, child_name)
        .iter()
        .copied()
        .find(|origin| origin.file(&db) == child)
        .expect("the shared child should have a concrete origin");
    let parent = TemplateName::new(&db, "parent.html".to_string());

    let indexed = resolution.resolve_for_file(&db, parent, child);
    let scanned_scope = resolution.backend_scope_for_origin(&db, child_origin);
    let scanned = resolution
        .resolve_reference_from_origin_in_scope(
            &db,
            child_origin,
            parent,
            &[],
            true,
            &scanned_scope,
        )
        .expect("an absolute parent name should normalize")
        .result;
    assert_eq!(
        indexed, scanned,
        "the direct file index must preserve origin-scan scope"
    );

    let search = match indexed {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("different engine-local parents should make resolution inconclusive");
    let paths = search
        .possible_origins
        .iter()
        .map(|origin| origin.path_buf(&db).as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        ["/test/project/a/parent.html", "/test/project/b/parent.html"]
    );
}

#[test]
fn scoped_resolution_retains_open_backend_after_concrete_membership() {
    let db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False},\n    UNKNOWN,\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/shared/child.html",
            "{% extends 'parent.html' %}",
        )
        .file("/test/project/shared/parent.html", "parent")
        .build(&db)
        .expect("open-backend project fixture should build");
    let resolution = template_resolution(&db, project);
    let child = db
        .file(Utf8Path::new("/test/project/shared/child.html"))
        .expect("open-backend child fixture should exist in the test database");
    let parent = TemplateName::new(&db, "parent.html".to_string());

    let search = match resolution.resolve_for_file(&db, parent, child) {
        TemplateResolutionResult::Inconclusive(search) => Some(search),
        TemplateResolutionResult::Found(_) | TemplateResolutionResult::DoesNotExist(_) => None,
    }
    .expect("an additional open engine should keep scoped resolution inconclusive");
    assert_eq!(search.possible_origins.len(), 1);
    assert_eq!(
        search.possible_origins[0].path_buf(&db),
        Utf8Path::new("/test/project/shared/parent.html")
    );

    assert!(matches!(
        resolution.resolve(&db, parent),
        TemplateResolutionResult::Found(_)
    ));
}

#[test]
fn resolve_excluding_skips_excluded_origins() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            ("base.html", "/test/project/templates/base.html", "first"),
            (
                "base.html",
                "/test/project/app/templates/base.html",
                "second",
            ),
        ],
    )
    .expect("excluded-origin project fixture should build");
    let resolution = template_resolution(&db, project);
    let name = TemplateName::new(&db, "base.html".to_string());
    let first = resolution.origins_for_name(&db, name)[0].file(&db);

    let origin = match resolution.resolve_excluding(&db, name, &[first]) {
        TemplateResolutionResult::Found(origin) => Some(origin),
        TemplateResolutionResult::DoesNotExist(_) | TemplateResolutionResult::Inconclusive(_) => {
            None
        }
    }
    .expect("resolution should return the non-excluded origin");

    assert_eq!(
        origin.path_buf(&db).as_str(),
        "/test/project/app/templates/base.html"
    );
}
