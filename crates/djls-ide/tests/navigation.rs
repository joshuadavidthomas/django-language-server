use camino::Utf8Path;
use djls_ide::find_references;
use djls_source::Offset;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

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
