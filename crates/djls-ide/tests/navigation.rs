use camino::Utf8Path;
use djls_ide::find_references;
use djls_ide::goto_definition as ide_goto_definition;
use djls_source::File;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

fn goto_definition(
    db: &TestDatabase,
    file: File,
    offset: Offset,
    supports_location_links: bool,
) -> Option<ls_types::GotoDefinitionResponse> {
    ide_goto_definition(
        db,
        file,
        offset,
        supports_location_links,
        PositionEncoding::Utf8,
    )
}

fn offset_of(source: &str, needle: &str) -> Offset {
    let Some(offset) = source.find(needle) else {
        panic!("test source should contain `{needle}`");
    };
    let Ok(offset) = u32::try_from(offset) else {
        panic!("test source offset should fit in u32");
    };
    Offset::new(offset)
}

const CUSTOM_SYMBOL_LIBRARY: &str = "from django import template\nregister = template.Library()\n\n@register.simple_tag(name='shown')\ndef tag_impl():\n    return ''\n\n@register.filter(name='shout')\ndef filter_impl(value):\n    return value\n";

fn custom_symbol_navigation_fixture(
    template_source: &str,
    library_source: &str,
) -> Result<(TestDatabase, File), Box<dyn std::error::Error>> {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
        )
        .file("/test/project/custom_tags.py", library_source)
        .file("/test/project/templates/page.html", template_source)
        .install(&mut db)?;
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"))?;
    Ok((db, file))
}

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
        .install(&mut db)
        .expect("multi-backend project fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/a/child.html"))
        .expect("child template fixture should exist");

    assert!(
        goto_definition(
            &db,
            file,
            Offset::new(
                u32::try_from(
                    source
                        .find("base")
                        .expect("test source should contain the expected text")
                )
                .expect("test source offset should fit in u32")
            ),
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
        .install(&mut db)
        .expect("location-link project fixture should install");

    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");
    let response = goto_definition(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("base")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
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
fn goto_definition_encodes_existing_template_reference_origins() {
    let mut db = TestDatabase::new();
    let source = "😀{% extends \"base.html\" %}";
    let child_path = "/test/project/templates/child.html";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db)
        .expect("encoded Template-reference fixture should install");
    let file = db
        .file(Utf8Path::new(child_path))
        .expect("encoded child template should exist");

    let response = ide_goto_definition(
        &db,
        file,
        offset_of(source, "base"),
        true,
        PositionEncoding::Utf16,
    )
    .expect("encoded Template reference should resolve");
    let ls_types::GotoDefinitionResponse::Link(links) = response else {
        panic!("LocationLink client should receive Template links")
    };

    assert_eq!(
        links[0].origin_selection_range,
        Some(ls_types::Range::new(
            ls_types::Position::new(0, 14),
            ls_types::Position::new(0, 23),
        ))
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
        .install(&mut db)
        .expect("originless template project fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/scratch.html"))
        .expect("scratch template fixture should exist");

    let response = goto_definition(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("base")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
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
        .install(&mut db)
        .expect("originless relative-reference fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/scratch.html"))
        .expect("scratch template fixture should exist");

    assert!(
        goto_definition(
            &db,
            file,
            Offset::new(
                u32::try_from(
                    source
                        .find("base")
                        .expect("test source should contain the expected text")
                )
                .expect("test source offset should fit in u32")
            ),
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
        .install(&mut db)
        .expect("location fallback project fixture should install");

    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");
    let response = goto_definition(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("base")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
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
        .install(&mut db)
        .expect("incomplete-search project fixture should install");

    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");
    let response = goto_definition(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("base")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
        true,
    )
    .expect("known possible origins should remain navigable");

    let links = match response {
        ls_types::GotoDefinitionResponse::Link(links) => Some(links),
        ls_types::GotoDefinitionResponse::Scalar(_)
        | ls_types::GotoDefinitionResponse::Array(_) => None,
    }
    .expect("location-link support should return location links");
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

    let plain = goto_definition(&db, file, offset_of(source, "base"), false)
        .expect("known possible origin should remain navigable without links");
    assert!(
        matches!(plain, ls_types::GotoDefinitionResponse::Array(locations) if locations.len() == 1),
        "an inconclusive plain response should preserve its array shape",
    );
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
        .install(&mut db)
        .expect("originless incomplete-search fixture should install");

    let file = db
        .file(Utf8Path::new(child_path))
        .expect("scratch template fixture should exist");
    let response = goto_definition(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("missing")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
        false,
    );

    assert_eq!(response, None);
}

#[test]
fn goto_definition_resolves_template_block_to_nearest_parent() {
    let mut db = TestDatabase::new();
    let parent_source = "{% block title %}Parent{% endblock %}";
    let child_source = "{% extends \"base.html\" %}\n{% block title %}Child{% endblock %}";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/templates/base.html", parent_source)
        .file("/test/project/templates/child.html", child_source)
        .install(&mut db)
        .expect("Template Block navigation fixture should install");
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("child Template fixture should exist");
    let parent = db
        .file(Utf8Path::new("/test/project/templates/base.html"))
        .expect("parent Template fixture should exist");

    let response = goto_definition(&db, child, offset_of(child_source, "title"), true)
        .expect("overridden Template Block should resolve to its parent");

    assert_eq!(
        response,
        ls_types::GotoDefinitionResponse::Link(vec![ls_types::LocationLink {
            origin_selection_range: Some(ls_types::Range::new(
                ls_types::Position::new(1, 9),
                ls_types::Position::new(1, 14),
            )),
            target_uri: "file:///test/project/templates/base.html"
                .parse()
                .expect("test URI should parse"),
            target_range: ls_types::Range::new(
                ls_types::Position::new(0, 0),
                ls_types::Position::new(
                    0,
                    u32::try_from(parent_source.len())
                        .expect("parent Template length should fit in u32"),
                ),
            ),
            target_selection_range: ls_types::Range::new(
                ls_types::Position::new(0, 9),
                ls_types::Position::new(0, 14),
            ),
        }])
    );
    assert!(
        goto_definition(&db, parent, offset_of(parent_source, "title"), true).is_none(),
        "a root Template Block has no parent definition"
    );
}

#[test]
fn goto_definition_encodes_template_block_targets_for_link_and_plain_clients() {
    let mut db = TestDatabase::new();
    let parent_source = "😀{% block title %}Parent{% endblock %}";
    let child_source = "{% extends \"base.html\" %}\n😀{% block title %}Child{% endblock %}";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/templates/base.html", parent_source)
        .file("/test/project/templates/child.html", child_source)
        .install(&mut db)
        .expect("encoded Template Block navigation fixture should install");
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("encoded child Template fixture should exist");
    let offset = offset_of(child_source, "title");

    let linked = ide_goto_definition(&db, child, offset, true, PositionEncoding::Utf16)
        .expect("encoded Template Block should resolve for a link client");
    let ls_types::GotoDefinitionResponse::Link(links) = linked else {
        panic!("LocationLink client should receive a Template Block link")
    };
    assert_eq!(
        links[0].origin_selection_range,
        Some(ls_types::Range::new(
            ls_types::Position::new(1, 11),
            ls_types::Position::new(1, 16),
        ))
    );
    assert_eq!(links[0].target_range.start, ls_types::Position::new(0, 2));
    assert_eq!(
        links[0].target_selection_range,
        ls_types::Range::new(
            ls_types::Position::new(0, 11),
            ls_types::Position::new(0, 16),
        )
    );

    let plain = ide_goto_definition(&db, child, offset, false, PositionEncoding::Utf16)
        .expect("encoded Template Block should resolve for a plain client");
    let ls_types::GotoDefinitionResponse::Scalar(location) = plain else {
        panic!("plain client should receive one Template Block location")
    };
    assert_eq!(
        location.uri,
        "file:///test/project/templates/base.html"
            .parse()
            .expect("test URI should parse")
    );
    assert_eq!(location.range.start, ls_types::Position::new(0, 2));
}

#[test]
fn goto_definition_does_not_skip_an_uncertain_parent_block() {
    let mut db = TestDatabase::new();
    let child_source = "{% extends \"layout.html\" %}\n{% block title %}Child{% endblock %}";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {**UNKNOWN}}}]\n",
        )
        .file(
            "/test/project/templates/base.html",
            "{% block title %}Base{% endblock %}",
        )
        .file(
            "/test/project/templates/layout.html",
            "{% extends \"base.html\" %}\n{% load unknown_library %}\n{% block title %}Layout{% endblock %}",
        )
        .file("/test/project/templates/child.html", child_source)
        .install(&mut db)
        .expect("uncertain parent Template Block fixture should install");
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("child Template fixture should exist");

    assert!(
        goto_definition(&db, child, offset_of(child_source, "title"), true).is_none(),
        "an uncertain block in the nearest ancestor must not be skipped"
    );
}

#[test]
fn goto_definition_resolves_template_library_to_module_start() {
    let source = "{% load custom %}";
    let (db, file) = custom_symbol_navigation_fixture(source, CUSTOM_SYMBOL_LIBRARY)
        .expect("custom-symbol project fixture should build");

    let response = goto_definition(&db, file, offset_of(source, "custom"), true)
        .expect("Template Library should resolve");

    assert_eq!(
        response,
        ls_types::GotoDefinitionResponse::Link(vec![ls_types::LocationLink {
            origin_selection_range: Some(ls_types::Range::new(
                ls_types::Position::new(0, 8),
                ls_types::Position::new(0, 14),
            )),
            target_uri: "file:///test/project/custom_tags.py"
                .parse()
                .expect("test URI should parse"),
            target_range: ls_types::Range::default(),
            target_selection_range: ls_types::Range::default(),
        }])
    );
}

#[test]
fn goto_definition_resolves_django_load_and_static_tags() {
    let source = "{% load static %}\n{% static 'asset.css' %}";
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['django.template.defaulttags'], 'libraries': {'static': 'django.templatetags.static'}}}]\n",
        )
        .file("/test/project/django/__init__.py", "")
        .file("/test/project/django/template/__init__.py", "")
        .file(
            "/test/project/django/template/defaulttags.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef load(parser, token): pass\n",
        )
        .file("/test/project/django/templatetags/__init__.py", "")
        .file(
            "/test/project/django/templatetags/static.py",
            "from django import template\nregister = template.Library()\n@register.tag('static')\ndef do_static(parser, token): pass\n",
        )
        .file("/test/project/templates/page.html", source)
        .install(&mut db)
        .expect("Django Tag project fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("Django Tag template should exist");

    let load = goto_definition(&db, file, offset_of(source, "load"), true)
        .expect("builtin load Tag should resolve");
    let static_library = goto_definition(&db, file, offset_of(source, "static"), true)
        .expect("static Template Library should resolve");
    let static_tag = goto_definition(
        &db,
        file,
        Offset::new(
            u32::try_from(source.rfind("static").expect("static Tag should exist"))
                .expect("test source offset should fit in u32"),
        ),
        true,
    )
    .expect("loaded static Tag should resolve");

    let target = |response: ls_types::GotoDefinitionResponse| match response {
        ls_types::GotoDefinitionResponse::Link(links) => links
            .into_iter()
            .next()
            .expect("resolved definition should have one target"),
        ls_types::GotoDefinitionResponse::Scalar(_)
        | ls_types::GotoDefinitionResponse::Array(_) => {
            panic!("LocationLink client should receive definition links")
        }
    };
    let load_target = target(load);
    let static_library_target = target(static_library);
    let static_tag_target = target(static_tag);

    assert!(
        load_target
            .target_uri
            .as_str()
            .ends_with("/django/template/defaulttags.py")
    );
    assert!(
        static_library_target
            .target_uri
            .as_str()
            .ends_with("/django/templatetags/static.py")
    );
    assert_eq!(
        static_library_target.target_range,
        ls_types::Range::default()
    );
    assert_eq!(
        static_library_target.target_selection_range,
        ls_types::Range::default()
    );
    assert!(
        static_tag_target
            .target_uri
            .as_str()
            .ends_with("/django/templatetags/static.py")
    );
    assert_eq!(
        static_tag_target.target_range.start,
        ls_types::Position::new(2, 0)
    );
    assert_eq!(
        static_tag_target.target_selection_range,
        ls_types::Range::new(
            ls_types::Position::new(3, 4),
            ls_types::Position::new(3, 13),
        )
    );
}

#[test]
fn goto_definition_resolves_loaded_tag_and_filter_to_local_functions() {
    let source = "{% load custom %}\n{% shown %}\n{{ value|shout }}";
    let (db, file) = custom_symbol_navigation_fixture(source, CUSTOM_SYMBOL_LIBRARY)
        .expect("custom-symbol project fixture should build");

    let tag = goto_definition(&db, file, offset_of(source, "shown"), true)
        .expect("loaded Tag should resolve");
    let filter = goto_definition(&db, file, offset_of(source, "shout"), true)
        .expect("loaded Filter should resolve");

    let tag_links = match tag {
        ls_types::GotoDefinitionResponse::Link(links) => links,
        ls_types::GotoDefinitionResponse::Scalar(_)
        | ls_types::GotoDefinitionResponse::Array(_) => {
            panic!("LocationLink client should receive Tag links")
        }
    };
    assert_eq!(tag_links.len(), 1);
    assert_eq!(
        tag_links[0].target_uri.as_str(),
        "file:///test/project/custom_tags.py"
    );
    assert_eq!(
        tag_links[0].origin_selection_range,
        Some(ls_types::Range::new(
            ls_types::Position::new(1, 3),
            ls_types::Position::new(1, 8),
        ))
    );
    assert_eq!(
        tag_links[0].target_range.start,
        ls_types::Position::new(3, 0)
    );
    assert_eq!(
        tag_links[0].target_selection_range,
        ls_types::Range::new(
            ls_types::Position::new(4, 4),
            ls_types::Position::new(4, 12),
        )
    );

    let filter_links = match filter {
        ls_types::GotoDefinitionResponse::Link(links) => links,
        ls_types::GotoDefinitionResponse::Scalar(_)
        | ls_types::GotoDefinitionResponse::Array(_) => {
            panic!("LocationLink client should receive Filter links")
        }
    };
    assert_eq!(filter_links.len(), 1);
    assert_eq!(
        filter_links[0].origin_selection_range,
        Some(ls_types::Range::new(
            ls_types::Position::new(2, 9),
            ls_types::Position::new(2, 14),
        ))
    );
    assert_eq!(
        filter_links[0].target_range.start,
        ls_types::Position::new(7, 0)
    );
    assert_eq!(
        filter_links[0].target_selection_range,
        ls_types::Range::new(
            ls_types::Position::new(8, 4),
            ls_types::Position::new(8, 15),
        )
    );
}

#[test]
fn goto_definition_follows_source_order_shadowing() {
    let source = "{% shared %}{% load alpha %}{% shared %}{% load beta %}{% shared %}";
    let mut db = TestDatabase::new();
    let library_source = |function: &str| {
        format!(
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared')\ndef {function}(): pass\n"
        )
    };
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['builtin_tags'], 'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags'}}}]\n",
        )
        .file("/test/project/builtin_tags.py", library_source("builtin"))
        .file("/test/project/alpha_tags.py", library_source("alpha"))
        .file("/test/project/beta_tags.py", library_source("beta"))
        .file("/test/project/templates/page.html", source)
        .install(&mut db)
        .expect("shadowed-definition project fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("shadowed-definition template should exist");
    let offsets = source
        .match_indices("shared")
        .map(|(offset, _)| {
            Offset::new(u32::try_from(offset).expect("test source offset should fit in u32"))
        })
        .collect::<Vec<_>>();

    let target_uris = offsets
        .into_iter()
        .map(|offset| {
            let response = goto_definition(&db, file, offset, true)
                .expect("effective Tag Definition should resolve");
            match response {
                ls_types::GotoDefinitionResponse::Link(links) => links[0].target_uri.to_string(),
                ls_types::GotoDefinitionResponse::Scalar(_)
                | ls_types::GotoDefinitionResponse::Array(_) => {
                    panic!("LocationLink client should receive Tag links")
                }
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(
        target_uris,
        [
            "file:///test/project/builtin_tags.py",
            "file:///test/project/alpha_tags.py",
            "file:///test/project/beta_tags.py",
        ]
    );
}

#[test]
fn goto_definition_requires_backend_definition_agreement() {
    let shared_source = "{% load shared %}\n{% common %}";
    let shared_settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['shared_tags'], 'libraries': {'shared': 'empty_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'shared_tags'}}}]\n";
    let mut shared_db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", shared_settings)
        .file(
            "/test/project/shared_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef common(): pass\n",
        )
        .file(
            "/test/project/empty_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/shared/page.html", shared_source)
        .install(&mut shared_db)
        .expect("shared-definition project fixture should install");
    let shared_file = shared_db
        .file(Utf8Path::new("/test/project/shared/page.html"))
        .expect("shared-definition template should exist");

    assert!(
        goto_definition(
            &shared_db,
            shared_file,
            offset_of(shared_source, "common"),
            true,
        )
        .is_some(),
        "builtin and loaded exposure of one definition should agree",
    );

    let conflicting_source = "{% common %}";
    let conflicting_settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['alpha_tags']}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['beta_tags']}}]\n";
    let mut conflicting_db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            conflicting_settings,
        )
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef common(): pass\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef common(): pass\n",
        )
        .file("/test/project/shared/page.html", conflicting_source)
        .install(&mut conflicting_db)
        .expect("conflicting-definition project fixture should install");
    let conflicting_file = conflicting_db
        .file(Utf8Path::new("/test/project/shared/page.html"))
        .expect("conflicting-definition template should exist");

    assert_eq!(
        goto_definition(
            &conflicting_db,
            conflicting_file,
            offset_of(conflicting_source, "common"),
            true,
        ),
        None
    );
}

#[test]
fn goto_definition_returns_both_selective_tag_and_filter_targets() {
    let source = "{% load shared from custom %}";
    let library_source = "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared')\ndef as_tag(): pass\n@register.filter(name='shared')\ndef as_filter(value): return value\n";
    let (db, file) = custom_symbol_navigation_fixture(source, library_source)
        .expect("selective-symbol project fixture should build");

    let response = goto_definition(&db, file, offset_of(source, "shared"), true)
        .expect("selective Tag and Filter should resolve");
    let links = match response {
        ls_types::GotoDefinitionResponse::Link(links) => links,
        ls_types::GotoDefinitionResponse::Scalar(_)
        | ls_types::GotoDefinitionResponse::Array(_) => {
            panic!("LocationLink client should receive selective links")
        }
    };

    assert_eq!(links.len(), 2);
    assert_eq!(
        links
            .iter()
            .map(|link| link.target_selection_range)
            .collect::<Vec<_>>(),
        [
            ls_types::Range::new(
                ls_types::Position::new(3, 4),
                ls_types::Position::new(3, 10),
            ),
            ls_types::Range::new(
                ls_types::Position::new(5, 4),
                ls_types::Position::new(5, 13),
            ),
        ]
    );
}

#[test]
fn goto_definition_encodes_template_origins_and_python_targets() {
    let source = "😀{% load custom %}\n😀{% shown %}";
    let library_source = "from django import template\nregister = template.Library()\n@register.simple_tag(name='shown')\ndef café(): pass\n";
    let (db, file) = custom_symbol_navigation_fixture(source, library_source)
        .expect("encoded-symbol project fixture should build");
    let offset = Offset::new(
        u32::try_from(source.rfind("shown").expect("shown Tag should exist"))
            .expect("test source offset should fit in u32"),
    );

    for (encoding, origin_start, origin_end, selection_end) in [
        (PositionEncoding::Utf8, 7, 12, 9),
        (PositionEncoding::Utf16, 5, 10, 8),
        (PositionEncoding::Utf32, 4, 9, 8),
    ] {
        let response = ide_goto_definition(&db, file, offset, true, encoding)
            .expect("encoded Tag target should resolve");
        let links = match response {
            ls_types::GotoDefinitionResponse::Link(links) => links,
            ls_types::GotoDefinitionResponse::Scalar(_)
            | ls_types::GotoDefinitionResponse::Array(_) => {
                panic!("LocationLink client should receive Tag links")
            }
        };
        assert_eq!(
            links[0].origin_selection_range,
            Some(ls_types::Range::new(
                ls_types::Position::new(1, origin_start),
                ls_types::Position::new(1, origin_end),
            ))
        );
        assert_eq!(
            links[0].target_selection_range,
            ls_types::Range::new(
                ls_types::Position::new(3, 4),
                ls_types::Position::new(3, selection_end),
            )
        );
    }

    let plain = ide_goto_definition(&db, file, offset, false, PositionEncoding::Utf16)
        .expect("plain encoded Tag target should resolve");
    let ls_types::GotoDefinitionResponse::Scalar(location) = plain else {
        panic!("one exact target should use a scalar Location")
    };
    assert_eq!(location.uri.as_str(), "file:///test/project/custom_tags.py");
    assert_eq!(location.range.start, ls_types::Position::new(2, 0));
}

#[test]
fn goto_definition_does_not_guess_unloaded_or_member_callable_targets() {
    let unloaded_source = "{% shown %}";
    let (unloaded_db, unloaded_file) =
        custom_symbol_navigation_fixture(unloaded_source, CUSTOM_SYMBOL_LIBRARY)
            .expect("unloaded-symbol project fixture should build");
    assert_eq!(
        goto_definition(
            &unloaded_db,
            unloaded_file,
            offset_of(unloaded_source, "shown"),
            true,
        ),
        None
    );

    let member_source = "{% load custom %}{% member %}";
    let member_library = "from django import template\nregister = template.Library()\nclass Node:\n    def handle(self, parser, token): pass\nregister.tag('member', Node.handle)\n";
    let (member_db, member_file) = custom_symbol_navigation_fixture(member_source, member_library)
        .expect("member-symbol project fixture should build");
    assert_eq!(
        goto_definition(
            &member_db,
            member_file,
            offset_of(member_source, "member"),
            true,
        ),
        None
    );
}

#[test]
fn find_references_resolves_extends_with_the_source_origin_skipped() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let child_path = "/test/project/first/base.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/first', '/test/project/second'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file(
            "/test/project/first/include.html",
            r#"{% include "base.html" %}"#,
        )
        .file("/test/project/second/base.html", "parent")
        .install(&mut db)
        .expect("shadowed-template project fixture should install");

    let file = db
        .file(Utf8Path::new(child_path))
        .expect("shadowing template fixture should exist");
    let locations = find_references(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("base")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
    )
    .expect("the shadowing template should reference the next origin");

    assert_eq!(
        locations,
        [ls_types::Location {
            uri: "file:///test/project/first/base.html"
                .parse()
                .expect("test URI should parse"),
            range: ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 21),
            ),
        }]
    );
}

#[test]
fn find_references_skips_the_source_file_across_template_name_aliases() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "alias/base.html" %}"#;
    let source_path = "/test/project/templates/alias/base.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', '/test/project/templates/alias', '/test/project/fallback'], 'APP_DIRS': False}]\n",
        )
        .file(source_path, source)
        .file(
            "/test/project/templates/include.html",
            r#"{% include "alias/base.html" %}"#,
        )
        .file("/test/project/fallback/alias/base.html", "parent")
        .install(&mut db)
        .expect("template-alias project fixture should install");

    let file = db
        .file(Utf8Path::new(source_path))
        .expect("source template fixture should exist");
    let locations = find_references(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("alias")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
    )
    .expect("every source alias should resolve the extends reference to the parent");

    assert_eq!(locations.len(), 1);
    assert_eq!(
        locations[0].uri,
        "file:///test/project/templates/alias/base.html"
            .parse()
            .expect("test URI should parse")
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
        .file(child_path, source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db)
        .expect("template-reference project fixture should install");

    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");
    let locations = find_references(
        &db,
        file,
        Offset::new(
            u32::try_from(
                source
                    .find("base")
                    .expect("test source should contain the expected text"),
            )
            .expect("test source offset should fit in u32"),
        ),
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
