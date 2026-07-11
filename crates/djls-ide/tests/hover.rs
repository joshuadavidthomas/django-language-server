use camino::Utf8Path;
use djls_ide::hover;
use djls_source::Offset;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

fn hover_markdown(hover: ls_types::Hover) -> String {
    let ls_types::HoverContents::Markup(contents) = hover.contents else {
        panic!("template hover should use markup content");
    };
    contents.value
}

#[test]
fn missing_template_hover_says_template_not_found() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "missing.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .template_file("child.html", child_path, source)
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let result = hover(
        &db,
        file,
        Offset::new(u32::try_from(source.find("missing").unwrap()).unwrap()),
    )
    .expect("missing template with known search roots should have hover");
    let markdown = hover_markdown(result);

    assert!(markdown.contains("Template not found."));
    assert!(markdown.contains("`/test/project/templates/missing.html`"));
    assert!(!markdown.contains("search is incomplete"));
}

#[test]
fn inconclusive_template_hover_describes_incomplete_search_and_possible_matches() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates', '/test/project/app/templates'], 'APP_DIRS': False}]\n",
        )
        .template_file("child.html", child_path, source)
        .template_file("base.html", "/test/project/templates/base.html", "first")
        .template_file("base.html", "/test/project/app/templates/base.html", "second")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let result = hover(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
    )
    .expect("inconclusive template search should have hover");
    let markdown = hover_markdown(result);

    assert!(markdown.contains("Template search is incomplete."));
    assert!(markdown.contains("Possible matches:"));
    assert!(markdown.contains("`/test/project/templates/base.html`"));
    assert!(!markdown.contains("`/test/project/app/templates/base.html`"));
    assert!(!markdown.contains("Template not found."));
}
