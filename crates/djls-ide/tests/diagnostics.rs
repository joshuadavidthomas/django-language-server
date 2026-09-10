use camino::Utf8Path;
use djls_ide::collect_diagnostics;
use djls_source::PositionEncoding;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

#[test]
fn unreadable_library_diagnostic_is_a_hint_with_python_source_information() {
    let mut db = TestDatabase::new();
    let library_source = concat!(
        "from django import template\n",
        "register = template.Library()\n",
        "def other_tag(context): pass\n",
        "register.simple_tag(takes_context=True)(globals()['other_tag'])\n",
    );
    ProjectFixture::new("/proj")
        .django_settings_module("project.settings")
        .file(
            "/proj/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'open': 'open_tags'}}}]\n",
        )
        .file("/proj/open_tags.py", library_source)
        .file("/proj/templates/page.html", "{% load open %}")
        .install(&mut db)
        .expect("unreadable-library diagnostic fixture should install");
    let template_file = db
        .file(Utf8Path::new("/proj/templates/page.html"))
        .expect("template fixture should exist");
    let registration_file = db
        .file(Utf8Path::new("/proj/open_tags.py"))
        .expect("registration fixture should exist");

    let diagnostics = collect_diagnostics(&db, template_file, PositionEncoding::Utf8)
        .expect("template should be a diagnostic target");
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("expected one unreadable-library diagnostic, got {diagnostics:#?}");
    };
    assert_eq!(
        diagnostic.code,
        Some(ls_types::NumberOrString::String("S124".to_string()))
    );
    assert_eq!(
        diagnostic.severity,
        Some(ls_types::DiagnosticSeverity::HINT)
    );
    assert_eq!(
        diagnostic.range,
        ls_types::Range::new(
            ls_types::Position::new(0, 8),
            ls_types::Position::new(0, 12),
        )
    );
    let [related] = diagnostic
        .related_information
        .as_deref()
        .expect("unreadable registration should have related information")
    else {
        panic!("expected one related-information entry");
    };
    assert_eq!(
        related.location.uri,
        ls_types::Uri::from_file_path(registration_file.path(&db).as_std_path())
            .expect("registration path should convert to a file URI")
    );
    assert_eq!(
        related.location.range,
        ls_types::Range::new(
            ls_types::Position::new(3, 0),
            ls_types::Position::new(3, 63),
        )
    );
    assert_eq!(related.message, "the registered name cannot be resolved");
}
