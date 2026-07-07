use std::collections::HashMap;

use camino::Utf8Path;
use djls_ide::document_links;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use djls_testing::make_template_libraries;
use tower_lsp_server::ls_types;

#[test]
fn document_links_resolve_template_references_with_interior_ranges() {
    let mut db = TestDatabase::new();
    let child_path = "/test/project/templates/child.html";
    let base_path = "/test/project/templates/base.html";
    let partial_path = "/test/project/templates/partials/card.html";
    let source = concat!(
        "{% extends \"base.html\" %}\n",
        "{% include \"partials/card.html\" %}\n",
        "{% include \"missing.html\" %}\n",
        "{% include dynamic_template %}\n",
    );

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .template_file("child.html", child_path, source)
        .template_file("base.html", base_path, "base")
        .template_file("partials/card.html", partial_path, "partial")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let links = document_links(&db, file);

    assert_eq!(links.len(), 2);
    assert_eq!(
        links[0],
        ls_types::DocumentLink {
            range: ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 21),
            ),
            target: Some(
                "file:///test/project/templates/base.html"
                    .parse()
                    .expect("test URI should parse"),
            ),
            tooltip: None,
            data: None,
        }
    );
    assert_eq!(
        links[1],
        ls_types::DocumentLink {
            range: ls_types::Range::new(
                ls_types::Position::new(1, 12),
                ls_types::Position::new(1, 30),
            ),
            target: Some(
                "file:///test/project/templates/partials/card.html"
                    .parse()
                    .expect("test URI should parse"),
            ),
            tooltip: None,
            data: None,
        }
    );
}

#[test]
fn document_links_resolve_relative_include_to_sibling_template() {
    let mut db = TestDatabase::new();
    let child_path = "/test/project/templates/dir/child.html";
    let target_path = "/test/project/templates/dir/x.html";
    let source = "{% include \"./x.html\" %}\n";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .template_file("dir/child.html", child_path, source)
        .template_file("dir/x.html", target_path, "target")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let links = document_links(&db, file);

    assert_eq!(
        links,
        vec![ls_types::DocumentLink {
            range: ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 20),
            ),
            target: Some(
                "file:///test/project/templates/dir/x.html"
                    .parse()
                    .expect("test URI should parse"),
            ),
            tooltip: None,
            data: None,
        }]
    );
}

#[test]
fn document_links_resolve_load_libraries_with_argument_ranges() {
    let db = TestDatabase::new();
    let library_modules = HashMap::from([
        (
            "djls_app_tags".to_string(),
            "djls_app.templatetags.djls_app_tags".to_string(),
        ),
        (
            "extras".to_string(),
            "project.templatetags.extras".to_string(),
        ),
    ]);
    let template_libraries = make_template_libraries(&db, &[], &[], &library_modules, &[]);
    let db = db.with_template_libraries(template_libraries);
    let template_path = "/test/project/templates/load.html";
    let source = concat!(
        "{% load djls_app_tags extras missing %}\n",
        "{% load djls_greeting from djls_app_tags %}\n",
    );

    db.add_file(template_path, source);
    let file = db.file(Utf8Path::new(template_path));
    let links = document_links(&db, file);

    assert_eq!(
        links,
        vec![
            ls_types::DocumentLink {
                range: ls_types::Range::new(
                    ls_types::Position::new(0, 8),
                    ls_types::Position::new(0, 21),
                ),
                target: Some(
                    "file:///__djls_testing__/djls_app/templatetags/djls_app_tags.py"
                        .parse()
                        .expect("test URI should parse"),
                ),
                tooltip: None,
                data: None,
            },
            ls_types::DocumentLink {
                range: ls_types::Range::new(
                    ls_types::Position::new(0, 22),
                    ls_types::Position::new(0, 28),
                ),
                target: Some(
                    "file:///__djls_testing__/project/templatetags/extras.py"
                        .parse()
                        .expect("test URI should parse"),
                ),
                tooltip: None,
                data: None,
            },
            ls_types::DocumentLink {
                range: ls_types::Range::new(
                    ls_types::Position::new(1, 27),
                    ls_types::Position::new(1, 40),
                ),
                target: Some(
                    "file:///__djls_testing__/djls_app/templatetags/djls_app_tags.py"
                        .parse()
                        .expect("test URI should parse"),
                ),
                tooltip: None,
                data: None,
            },
        ]
    );
}
