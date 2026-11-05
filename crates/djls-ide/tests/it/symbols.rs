use std::borrow::Cow;
use std::collections::BTreeMap;

use camino::Utf8Path;
use djls_conf::DjangoVersion;
use djls_ide::document_symbols;
use djls_project::bundled_django_libraries;
use djls_semantic::EndTag;
use djls_semantic::TagSpec;
use djls_semantic::bundled_django_tag_specs;
use djls_source::PositionEncoding;
use djls_testing::ProjectFixture;
use djls_testing::ProjectSettings;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types::DocumentSymbol;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::Range;
use tower_lsp_server::ls_types::SymbolKind;

fn symbols_for_source(db: &TestDatabase, source: &str) -> Result<Vec<DocumentSymbol>, String> {
    db.add_file("test.html", source)
        .map_err(|error| error.to_string())?;
    let file = db
        .file(Utf8Path::new("test.html"))
        .map_err(|error| error.to_string())?;
    Ok(document_symbols(db, file, PositionEncoding::Utf16))
}

fn labels(items: &[DocumentSymbol]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

#[test]
fn header_tags_produce_outline_items() {
    let db = TestDatabase::new();
    let symbols = symbols_for_source(
        &db,
        "{% extends \"base.html\" %}\n{% load static i18n %}\n{% include \"partials/nav.html\" %}",
    )
    .expect("fixture symbols");
    assert_eq!(
        labels(&symbols),
        ["base.html", "static", "i18n", "partials/nav.html"]
    );
    assert_eq!(
        symbols.iter().map(|item| item.kind).collect::<Vec<_>>(),
        [
            SymbolKind::FILE,
            SymbolKind::MODULE,
            SymbolKind::MODULE,
            SymbolKind::FILE
        ]
    );
    assert_eq!(
        symbols
            .iter()
            .map(|item| item.detail.as_deref())
            .collect::<Vec<_>>(),
        [Some("extends"), Some("load"), Some("load"), Some("include")]
    );
    assert_eq!(
        symbols[0].selection_range,
        Range::new(Position::new(0, 12), Position::new(0, 21))
    );
    assert_eq!(
        symbols[3].selection_range,
        Range::new(Position::new(2, 12), Position::new(2, 29))
    );
}

#[test]
fn outline_roles_follow_load_position() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .settings(&ProjectSettings {
            dirs: vec!["/test/project/templates".to_string()],
            libraries: BTreeMap::from([("custom".to_string(), "custom_tags".to_string())]),
            ..ProjectSettings::default()
        })
        .file("/test/project/custom_tags.py", "from django import template\nregister = template.Library()\n@register.simple_tag(name='include')\ndef custom_include(value):\n    pass\n")
        .file("/test/project/templates/page.html", "{% include 'before.html' %}{% load custom %}{% include 'after.html' %}")
        .install(&mut db).expect("project fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("fixture should exist");
    let symbols = document_symbols(&db, file, PositionEncoding::Utf16);
    assert_eq!(labels(&symbols), ["before.html", "custom"]);
    assert_eq!(symbols[0].kind, SymbolKind::FILE);
    assert_eq!(symbols[1].kind, SymbolKind::MODULE);
}

#[test]
fn loader_role_outline_does_not_depend_on_load_spelling() {
    let mut specs = bundled_django_tag_specs().clone();
    let loader = specs
        .get("load")
        .cloned()
        .expect("bundled Django defines load");
    specs.insert("use_library".to_string(), loader);
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let symbols =
        symbols_for_source(&db, "{% use_library custom_tags %}").expect("fixture symbols");
    assert_eq!(labels(&symbols), ["custom_tags"]);
    assert_eq!(symbols[0].kind, SymbolKind::MODULE);
    assert_eq!(symbols[0].detail.as_deref(), Some("use_library"));
}

#[test]
fn selective_load_uses_library_as_outline_item_with_imported_symbols() {
    let db = TestDatabase::new();
    let symbols =
        symbols_for_source(&db, "{% load trans blocktrans from i18n %}").expect("fixture symbols");
    assert_eq!(labels(&symbols), ["i18n"]);
    assert_eq!(symbols[0].kind, SymbolKind::MODULE);
    assert_eq!(symbols[0].detail.as_deref(), Some("load"));
    assert_eq!(symbols[0].selection_range.start, Position::new(0, 30));
    let children = symbols[0].children.as_deref().expect("load children");
    assert_eq!(labels(children), ["trans", "blocktrans"]);
    assert_eq!(children[0].kind, SymbolKind::FUNCTION);
    assert_eq!(children[0].detail.as_deref(), Some("from i18n"));
    assert_eq!(children[0].selection_range.start, Position::new(0, 8));
}

#[test]
fn nested_blocks_produce_nested_outline_items() {
    let db = TestDatabase::new();
    let symbols = symbols_for_source(
        &db,
        "{% block content %}\n  {% block title %}Title{% endblock %}\n{% endblock %}",
    )
    .expect("fixture symbols");
    assert_eq!(labels(&symbols), ["content"]);
    let children = symbols[0].children.as_deref().expect("block children");
    assert_eq!(labels(children), ["title"]);
}

#[test]
fn standalone_tag_items_inside_blocks_are_nested() {
    let db = TestDatabase::new();
    let symbols = symbols_for_source(
        &db,
        "{% block content %}\n  {% include \"card.html\" %}\n{% endblock %}",
    )
    .expect("fixture symbols");
    assert_eq!(labels(&symbols), ["content"]);
    let children = symbols[0].children.as_deref().expect("block children");
    assert_eq!(labels(children), ["card.html"]);
}

#[test]
fn partial_definitions_outline_by_partial_name() {
    let defaulttags = bundled_django_libraries(DjangoVersion::Django61)
        .iter()
        .find(|library| library.module().as_str() == "django.template.defaulttags")
        .expect("Django 6.1 defaulttags");
    let mut specs = bundled_django_tag_specs().clone();
    specs
        .merge_tag_rules(defaulttags.tag_rules())
        .merge_block_specs(defaulttags.block_specs())
        .merge_tag_meanings(defaulttags.tag_meanings());
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let symbols = symbols_for_source(
        &db,
        "{% partialdef card %}Body{% endpartialdef %}{% partial card %}",
    )
    .expect("fixture symbols");
    assert_eq!(labels(&symbols), ["card", "card"]);
    assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
    assert_eq!(symbols[0].detail.as_deref(), Some("partialdef"));
    assert_eq!(symbols[1].kind, SymbolKind::FILE);
    assert_eq!(symbols[1].detail.as_deref(), Some("partial"));
}

#[test]
fn tags_without_role_hide_standalone_tags_but_keep_blocks() {
    let mut specs = bundled_django_tag_specs().clone();
    specs.insert(
        "myblock".into(),
        TagSpec::new(
            Cow::Borrowed("myapp.templatetags.custom"),
            Some(EndTag {
                name: Cow::Borrowed("endmyblock"),
                required: true,
            }),
            Cow::Borrowed(&[]),
            djls_semantic::BodyAnalysis::Analyze,
        ),
    );
    specs.insert(
        "mytag".into(),
        TagSpec::new(
            Cow::Borrowed("myapp.templatetags.custom"),
            None,
            Cow::Borrowed(&[]),
            djls_semantic::BodyAnalysis::Analyze,
        ),
    );
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let symbols = symbols_for_source(&db, "{% mytag %}{% myblock thing %}Body{% endmyblock %}")
        .expect("fixture symbols");
    assert_eq!(labels(&symbols), ["myblock thing"]);
    assert_eq!(symbols[0].kind, SymbolKind::OPERATOR);
}

#[test]
fn control_structure_inside_blocks_is_visible() {
    let db = TestDatabase::new();
    let symbols = symbols_for_source(
        &db,
        "{% load static %}\n{% block content %}\n  <h1>Hello, {{ user.username|lower }}!</h1>\n  {% if items %}\n    {% for item in items %}<li>{{ item.name }}</li>{% endfor %}\n  {% else %}\n    <p>No items found.</p>\n  {% endif %}\n  <img src=\"{% static 'images/logo.png' %}\" alt=\"Logo\">\n{% endblock %}",
    ).expect("fixture symbols");
    assert_eq!(labels(&symbols), ["static", "content"]);
    let children = symbols[1].children.as_deref().expect("block children");
    assert_eq!(
        labels(children),
        ["user.username", "if items", "images/logo.png"]
    );
    assert_eq!(children[0].kind, SymbolKind::VARIABLE);
    assert_eq!(children[0].selection_range.start, Position::new(2, 16));
    let filters = children[0].children.as_deref().expect("filters");
    assert_eq!(labels(filters), ["lower"]);
    assert_eq!(filters[0].kind, SymbolKind::FUNCTION);
    assert_eq!(children[1].kind, SymbolKind::OPERATOR);
    assert_eq!(children[1].selection_range.start, Position::new(3, 5));
    let branches = children[1].children.as_deref().expect("branches");
    assert_eq!(labels(branches), ["for item in items", "else"]);
    assert_eq!(
        labels(branches[0].children.as_deref().expect("for children")),
        ["item.name"]
    );
    assert_eq!(children[2].kind, SymbolKind::FILE);
}

#[test]
fn malformed_unclosed_blocks_produce_best_effort_outline_items() {
    let db = TestDatabase::new();
    let symbols = symbols_for_source(
        &db,
        "{% block content %}\n  {% if user %}\n    {% block title %}Title",
    )
    .expect("fixture symbols");
    assert_eq!(labels(&symbols), ["content"]);
    let children = symbols[0].children.as_deref().expect("block children");
    assert_eq!(labels(children), ["if user"]);
    assert_eq!(
        labels(children[0].children.as_deref().expect("if children")),
        ["title"]
    );
}
