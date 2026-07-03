use camino::Utf8Path;
use djls_ide::find_references;
use djls_ide::goto_definition;
use djls_source::Offset;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

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
        .template_file("child.html", child_path, source)
        .template_file("base.html", "/test/project/templates/base.html", "base")
        .install(&mut db);

    let file = db.get_or_create_file(Utf8Path::new(child_path));
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
        .template_file("child.html", child_path, source)
        .template_file("base.html", "/test/project/templates/base.html", "base")
        .install(&mut db);

    let file = db.get_or_create_file(Utf8Path::new(child_path));
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
        .template_file("child.html", child_path, source)
        .template_file("base.html", "/test/project/templates/base.html", "base")
        .install(&mut db);

    let file = db.get_or_create_file(Utf8Path::new(child_path));
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
