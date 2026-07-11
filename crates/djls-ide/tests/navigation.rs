use camino::Utf8Path;
use djls_ide::find_references;
use djls_ide::goto_definition;
use djls_source::Offset;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

#[test]
fn goto_definition_does_not_leak_a_template_from_another_backend() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let settings = "TEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/child.html", source)
        .file("/test/project/b/base.html", "other backend")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/a/child.html"));

    assert!(
        goto_definition(
            &db,
            file,
            Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
            true,
        )
        .is_none()
    );
}

#[test]
fn goto_definition_reports_location_link_with_origin_range() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let response = goto_definition(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
        true,
    )
    .expect("template reference should resolve to the target template");

    assert_eq!(
        response,
        ls_types::GotoDefinitionResponse::Link(vec![ls_types::LocationLink {
            origin_selection_range: Some(ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 21),
            )),
            target_uri: "file:///test/project/templates/base.html"
                .parse()
                .expect("test URI should parse"),
            target_range: ls_types::Range::default(),
            target_selection_range: ls_types::Range::default(),
        }])
    );
}

#[test]
fn goto_definition_resolves_absolute_reference_from_originless_file() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/scratch.html", source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/scratch.html"));

    let response = goto_definition(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
        false,
    );

    assert_eq!(
        response,
        Some(ls_types::GotoDefinitionResponse::Scalar(
            ls_types::Location {
                uri: "file:///test/project/templates/base.html"
                    .parse()
                    .expect("test URI should parse"),
                range: ls_types::Range::default(),
            }
        ))
    );
}

#[test]
fn goto_definition_leaves_relative_reference_from_originless_file_unresolved() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "./base.html" %}"#;

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/scratch.html", source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/scratch.html"));

    assert!(
        goto_definition(
            &db,
            file,
            Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
            false,
        )
        .is_none()
    );
}

#[test]
fn goto_definition_falls_back_to_location_without_link_support() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let response = goto_definition(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
        false,
    )
    .expect("template reference should resolve to the target template");

    assert_eq!(
        response,
        ls_types::GotoDefinitionResponse::Scalar(ls_types::Location {
            uri: "file:///test/project/templates/base.html"
                .parse()
                .expect("test URI should parse"),
            range: ls_types::Range::default(),
        })
    );
}

#[test]
fn goto_definition_reports_the_known_possible_winner_for_inconclusive_search() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates', '/test/project/app/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "first")
        .file("/test/project/app/templates/base.html", "second")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let response = goto_definition(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
        true,
    )
    .expect("known possible origins should remain navigable");

    let ls_types::GotoDefinitionResponse::Link(links) = response else {
        panic!("location-link support should return location links");
    };
    let target_uris = links
        .iter()
        .map(|link| link.target_uri.as_str())
        .collect::<Vec<_>>();
    assert_eq!(target_uris, ["file:///test/project/templates/base.html"]);
    assert!(links.iter().all(|link| {
        link.origin_selection_range
            == Some(ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 21),
            ))
    }));
}

#[test]
fn goto_definition_returns_none_for_originless_inconclusive_search() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "missing.html" %}"#;
    let child_path = "/test/project/scratch.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let response = goto_definition(
        &db,
        file,
        Offset::new(u32::try_from(source.find("missing").unwrap()).unwrap()),
        false,
    );

    assert_eq!(response, None);
}

#[test]
fn find_references_reports_template_name_interior_range() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let locations = find_references(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
    )
    .expect("template reference should resolve to at least one reference");

    assert_eq!(locations.len(), 1);
    assert_eq!(
        locations[0].range,
        ls_types::Range::new(
            ls_types::Position::new(0, 12),
            ls_types::Position::new(0, 21)
        ),
    );
}
