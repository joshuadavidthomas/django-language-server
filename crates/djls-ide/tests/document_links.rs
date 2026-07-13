use camino::Utf8Path;
use djls_ide::document_links;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

#[test]
fn document_links_do_not_leak_templates_from_another_backend() {
    let mut db = TestDatabase::new();
    let settings = "TEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/child.html", "{% include 'only-b.html' %}")
        .file("/test/project/b/only-b.html", "other backend")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/a/child.html"));

    assert!(document_links(&db, file).is_empty());
}

#[test]
fn document_links_resolve_absolute_references_from_originless_files() {
    let mut db = TestDatabase::new();
    let source = "{% include 'card.html' %}\n{% include './card.html' %}";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/scratch.html", source)
        .file("/test/project/templates/card.html", "card")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/scratch.html"));

    let links = document_links(&db, file);

    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].target.as_ref().map(|uri| uri.as_str()),
        Some("file:///test/project/templates/card.html")
    );
}

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
        .file(child_path, source)
        .file(base_path, "base")
        .file(partial_path, "partial")
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
fn document_links_skip_inconclusive_template_references() {
    let mut db = TestDatabase::new();
    let child_path = "/test/project/templates/child.html";
    let source = "{% extends \"base.html\" %}\n";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));

    assert!(document_links(&db, file).is_empty());
}

#[test]
fn document_links_do_not_invent_origin_for_source_less_configured_library() {
    let mut db = TestDatabase::new();
    let template_path = "/test/project/templates/load.html";
    ProjectFixture::new("/test/project")
        .django_settings_module("project.settings")
        .tag_specs(djls_conf::TagSpecDef {
            libraries: vec![djls_conf::TagLibraryDef {
                module: "missing.panel_tags".to_string(),
                requires_engine: None,
                tags: vec![djls_conf::TagDef {
                    name: "panel".to_string(),
                    tag_type: djls_conf::TagTypeDef::Block,
                    end: None,
                    intermediates: Vec::new(),
                    args: Vec::new(),
                    extra: None,
                }],
                extra: None,
            }],
            ..djls_conf::TagSpecDef::default()
        })
        .file(
            "/test/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'panels': 'missing.panel_tags'}}}]\n",
        )
        .file(template_path, "{% load panels %}")
        .install(&mut db);
    let file = db.file(Utf8Path::new(template_path));

    assert!(document_links(&db, file).is_empty());
}

#[test]
fn document_links_skip_library_candidate_beside_open_backend_alternative() {
    let mut db = TestDatabase::new();
    let template_path = "/test/project/templates/load.html";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': UNKNOWN}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'project_tags'}}}]\n",
        )
        .file(
            "/test/project/project_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file(template_path, "{% load custom %}")
        .install(&mut db);
    let file = db.file(Utf8Path::new(template_path));

    assert!(document_links(&db, file).is_empty());
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
        .file(child_path, source)
        .file(target_path, "target")
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
    let mut db = TestDatabase::new();
    let template_path = "/test/project/templates/load.html";
    let source = concat!(
        "{% load djls_app_tags extras missing %}\n",
        "{% load djls_greeting from djls_app_tags %}\n",
    );
    ProjectFixture::new("/test/project")
        .django_settings_module("project.settings")
        .file(
            "/test/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'djls_app_tags': 'djls_app.templatetags.djls_app_tags', 'extras': 'project.templatetags.extras'}}}]\n",
        )
        .file(
            "/test/project/djls_app/templatetags/djls_app_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef djls_greeting(): pass\n",
        )
        .file(
            "/test/project/project/templatetags/extras.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file(template_path, source)
        .install(&mut db);
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
                    "file:///test/project/djls_app/templatetags/djls_app_tags.py"
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
                    "file:///test/project/project/templatetags/extras.py"
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
                    "file:///test/project/djls_app/templatetags/djls_app_tags.py"
                        .parse()
                        .expect("test URI should parse"),
                ),
                tooltip: None,
                data: None,
            },
        ]
    );
}
