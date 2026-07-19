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
            _ => false,
        })
        .count()
}

fn project_with_templates(
    db: &mut TestDatabase,
    template_dirs: Vec<&str>,
    templates: Vec<(&str, &str, &str)>,
) -> Project {
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
    templates
        .into_iter()
        .fold(fixture, |fixture, (_name, path, source)| {
            fixture.file(path, source)
        })
        .build(db)
}

#[test]
fn template_environment_correlates_libraries_with_resolving_backends() {
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
        .build(&db);

    let alpha_file = db.file(Utf8Path::new("/test/project/a/alpha.html"));
    let beta_file = db.file(Utf8Path::new("/test/project/b/beta.html"));
    let outside_file = db.file(Utf8Path::new("/test/project/outside.html"));
    let alpha = template_environment(&db, project, alpha_file)
        .loadable_library_str("shared")
        .found()
        .expect("backend A should provide shared");
    let beta = template_environment(&db, project, beta_file)
        .loadable_library_str("shared")
        .found()
        .expect("backend B should provide shared");

    assert_eq!(alpha.module_name_str(), "alpha_tags");
    assert_eq!(beta.module_name_str(), "beta_tags");

    let catalog = template_libraries(&db, project);
    let inventory = TemplateEnvironment::from_project_inventory(catalog);
    let outside = template_environment(&db, project, outside_file);
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
        "a file environment must borrow backend A's library from the shared catalog"
    );
    assert!(
        ptr::eq(beta, catalog_beta),
        "a file environment must borrow backend B's library from the shared catalog"
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
        .build(&db);

    let definitions = TemplateEnvironment::from_project_inventory(template_libraries(&db, project))
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
        .build(&db);
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"));

    let environment = template_environment(&db, project, file);
    assert!(matches!(
        environment.loadable_library_str("shared"),
        LoadableLibraryLookup::Inconclusive(candidates)
            if candidates.len() == 1 && candidates[0].module_name_str() == "alpha_tags"
    ));
    assert_eq!(
        environment.contextual_library_chains(&["shared"]).len(),
        1,
        "backend-field uncertainty must not fabricate a configuration remainder alternative"
    );
}

#[test]
fn template_environment_file_backend_index_invalidates_with_settings_evidence() {
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
        .build(&db);
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"));

    let initial = template_environment(&db, project, file)
        .loadable_library_str("shared")
        .found()
        .expect("initial direct file scope should select alpha");
    assert_eq!(initial.module_name_str(), "alpha_tags");
    let _ = events.take();

    db.add_file(
        settings_path,
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/other-templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n",
    );
    SourceChanges::new([ChangeEvent::ContentChanged(settings_path.into())]).apply(&mut db);

    let updated = template_environment(&db, project, file)
        .loadable_library_str("shared")
        .found()
        .expect("invalidated direct file scope should select beta");
    assert_eq!(updated.module_name_str(), "beta_tags");
    let executed = events.take();
    assert_eq!(
        will_execute_count(&db, &executed, "template_directory_index"),
        1
    );
    assert_eq!(
        will_execute_count(&db, &executed, "template_environment_scope"),
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
        .build(&db);
    let file = db.file(Utf8Path::new("/test/project/outside.html"));
    let environment = template_environment(&db, project, file);

    let LoadableLibraryLookup::Found(library) = environment.loadable_library_str("shared") else {
        panic!("project inventory should remain available outside configured roots");
    };
    assert_eq!(library.module_name_str(), "missing.shared_tags");
    assert!(library.source_file().is_none());
    assert!(library.symbol_inventory_is_open());
    assert_eq!(
        environment.symbol("possibly_defined", TemplateSymbolKind::Tag),
        EnvironmentSymbolLookup::Inconclusive
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
        .build(&db);

    let file = db.file(Utf8Path::new("/test/project/one-templates/page.html"));
    let library = template_environment(&db, project, file)
        .loadable_library_str("shared")
        .found()
        .expect("the template must inherit libraries only from its feasible app branch");

    assert_eq!(library.module_name_str(), "first.templatetags.shared");
}

#[test]
fn template_environment_retains_wholly_unknown_settings_branch() {
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
        .build(&db);
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"));

    assert!(matches!(
        template_environment(&db, project, file).loadable_library_str("shared"),
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
        .build(&db);

    let file = db.file(Utf8Path::new("/test/project/shared/page.html"));
    assert!(matches!(
        template_environment(&db, project, file).loadable_library_str("shared"),
        LoadableLibraryLookup::Ambiguous(libraries) if libraries.len() == 2
    ));

    let name = TemplateName::new(&db, "page.html".to_string());
    assert!(matches!(
        template_resolution(&db, project).resolve(&db, name),
        FindTemplateResult::Found(_)
    ));
}

#[test]
fn template_environment_is_ambiguous_when_settings_branches_resolve_same_file_differently() {
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
        .build(&db);

    let file = db.file(Utf8Path::new("/test/project/shared/page.html"));
    assert!(matches!(
        template_environment(&db, project, file).loadable_library_str("shared"),
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
    );

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
    );

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
    );

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
    );

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
    );

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
    );

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
    );

    let file = db.file(Utf8Path::new("/test/project/templates/account/detail.html"));
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
    );

    let name = TemplateName::new(&db, "base.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);
    let FindTemplateResult::Found(origin) = result else {
        panic!("expected base.html to resolve");
    };

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
    );

    let name = TemplateName::new(&db, "missing.html".to_string());
    let FindTemplateResult::DoesNotExist(error) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected missing template");
    };

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
        .build(&db);

    let name = TemplateName::new(&db, "base.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected inconclusive search");
    };

    assert_eq!(search.name, name);
    assert_eq!(search.possible_origins.len(), 1);
    let directories = template_directories(&db, project);
    assert!(directories.configuration_may_omit_roots());
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
        .build(&known_first_db);
    let name = TemplateName::new(&known_first_db, "base.html".to_string());
    assert!(matches!(
        template_resolution(&known_first_db, known_first).resolve(&known_first_db, name),
        FindTemplateResult::Found(_)
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
        .build(&unknown_first_db);
    let name = TemplateName::new(&unknown_first_db, "base.html".to_string());
    assert!(matches!(
        template_resolution(&unknown_first_db, unknown_first).resolve(&unknown_first_db, name),
        FindTemplateResult::Inconclusive(_)
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
        .build(&db);
    let name = TemplateName::new(&db, "base.html".to_string());

    assert!(matches!(
        template_resolution(&db, project).resolve(&db, name),
        FindTemplateResult::Found(_)
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
        .build(&db);

    let name = TemplateName::new(&db, "base.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected dynamic DIRS to precede and weaken the APP_DIRS candidate");
    };

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
        .build(&db);

    let unanimous = TemplateName::new(&db, "unanimous.html".to_string());
    let FindTemplateResult::Found(origin) =
        template_resolution(&db, project).resolve(&db, unanimous)
    else {
        panic!("the same winner in every scalar path alternative should be definitive");
    };
    assert_eq!(
        origin.path_buf(&db),
        Utf8Path::new("/test/project/common/unanimous.html")
    );

    let divergent = TemplateName::new(&db, "conflict.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, divergent)
    else {
        panic!("different winners across scalar path alternatives should be inconclusive");
    };
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("alternative installed app lists cannot select a definitive template");
    };
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("the two feasible branches have different winners");
    };
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("the imported/imported and local/local branches have different winners");
    };
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Found(origin) = template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("all feasible arms should select the same physical template");
    };
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("independent settings joins should retain both successful cross-product paths");
    };
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("the feasible branches have different winners");
    };
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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Found(origin) = template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("the common leading app should win in every alternative");
    };

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
        .build(&db);

    let name = TemplateName::new(&db, "shared.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("a found/miss split cannot select a definitive template");
    };

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
        .build(&db);

    let name = TemplateName::new(&db, "missing.html".to_string());
    let FindTemplateResult::DoesNotExist(error) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("exhaustive misses in every alternative should report does not exist");
    };

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
        .build(&db);

    let name = TemplateName::new(&db, "base.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);

    assert!(matches!(result, FindTemplateResult::Found(_)));
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
        .build(&db);

    let name = TemplateName::new(&db, "base.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);

    assert!(matches!(result, FindTemplateResult::Found(_)));
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
        .build(&db);

    let name = TemplateName::new(&db, "missing.html".to_string());
    let FindTemplateResult::Inconclusive(search) =
        template_resolution(&db, project).resolve(&db, name)
    else {
        panic!("expected inconclusive search");
    };

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
        .build(&db);
    let resolution = template_resolution(&db, project);
    let page = db.file(Utf8Path::new("/test/project/a/page.html"));
    let only_b = TemplateName::new(&db, "only-b.html".to_string());

    assert!(matches!(
        resolution.resolve_for_file(&db, only_b, page),
        FindTemplateResult::DoesNotExist(_)
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
        .build(&db);
    let resolution = template_resolution(&db, project);
    let page = db.file(Utf8Path::new("/test/project/a/page.html"));
    let missing = TemplateName::new(&db, "missing.html".to_string());

    let FindTemplateResult::DoesNotExist(error) = resolution.resolve_for_file(&db, missing, page)
    else {
        panic!("the selected backend should exhaustively miss");
    };
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
        .build(&db);
    let resolution = template_resolution(&db, project);
    let child = db.file(Utf8Path::new("/test/project/shared/child.html"));
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

    let FindTemplateResult::Inconclusive(search) = indexed else {
        panic!("different engine-local parents must be inconclusive");
    };
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
        .build(&db);
    let resolution = template_resolution(&db, project);
    let child = db.file(Utf8Path::new("/test/project/shared/child.html"));
    let parent = TemplateName::new(&db, "parent.html".to_string());

    let FindTemplateResult::Inconclusive(search) = resolution.resolve_for_file(&db, parent, child)
    else {
        panic!("an additional open engine must keep scoped resolution inconclusive");
    };
    assert_eq!(search.possible_origins.len(), 1);
    assert_eq!(
        search.possible_origins[0].path_buf(&db),
        Utf8Path::new("/test/project/shared/parent.html")
    );

    assert!(matches!(
        resolution.resolve(&db, parent),
        FindTemplateResult::Found(_)
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
    );
    let resolution = template_resolution(&db, project);
    let name = TemplateName::new(&db, "base.html".to_string());
    let first = resolution.origins_for_name(&db, name)[0].file(&db);

    let FindTemplateResult::Found(origin) = resolution.resolve_excluding(&db, name, &[first])
    else {
        panic!("expected the non-excluded origin");
    };

    assert_eq!(
        origin.path_buf(&db).as_str(),
        "/test/project/app/templates/base.html"
    );
}
