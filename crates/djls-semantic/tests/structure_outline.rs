use std::borrow::Cow;

use camino::Utf8Path;
use djls_semantic::EndTag;
use djls_semantic::OutlineItem;
use djls_semantic::OutlineKind;
use djls_semantic::TagRole;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::build_template_outline;
use djls_semantic::build_template_tree;
use djls_semantic::builtin_tag_specs;
use djls_source::Span;
use djls_templates::parse_template;
use djls_testing::TestDatabase;
use rustc_hash::FxHashMap;

fn outline_for_source<'db>(db: &'db TestDatabase, source: &str) -> &'db Vec<OutlineItem> {
    db.add_file("test.html", source);
    let file = db.get_or_create_file(Utf8Path::new("test.html"));
    let nodelist = parse_template(db, file).expect("should parse");
    let tree = build_template_tree(db, nodelist);
    build_template_outline(db, tree)
}

fn labels(items: &[OutlineItem]) -> Vec<&str> {
    items.iter().map(|item| item.label.as_str()).collect()
}

#[test]
fn header_tags_produce_outline_items() {
    let db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}
{% load static i18n %}
{% include "partials/nav.html" %}"#;
    let outline = outline_for_source(&db, source);

    assert_eq!(
        labels(outline),
        vec!["base.html", "static", "i18n", "partials/nav.html"]
    );
    assert_eq!(
        outline.iter().map(|item| item.kind).collect::<Vec<_>>(),
        vec![
            OutlineKind::TemplateReference,
            OutlineKind::TemplateLibrary,
            OutlineKind::TemplateLibrary,
            OutlineKind::TemplateReference,
        ]
    );
    assert_eq!(
        outline
            .iter()
            .map(|item| item.detail.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("extends"), Some("load"), Some("load"), Some("include")]
    );
    assert_eq!(
        outline[0].selection_span,
        Span::saturating_from_parts_usize(source.find("base.html").unwrap(), "base.html".len())
    );
    assert_eq!(
        outline[3].selection_span,
        Span::saturating_from_parts_usize(
            source.find("partials/nav.html").unwrap(),
            "partials/nav.html".len()
        )
    );
}

#[test]
fn selective_load_uses_library_as_outline_item_with_imported_symbols() {
    let db = TestDatabase::new();
    let outline = outline_for_source(&db, "{% load trans blocktrans from i18n %}");

    assert_eq!(labels(outline), vec!["i18n"]);
    assert_eq!(outline[0].kind, OutlineKind::TemplateLibrary);
    assert_eq!(outline[0].detail.as_deref(), Some("load"));
    assert_eq!(
        outline[0].selection_span.start_usize(),
        "{% load trans blocktrans from ".len()
    );
    assert_eq!(labels(&outline[0].children), vec!["trans", "blocktrans"]);
    assert_eq!(
        outline[0].children[0].kind,
        OutlineKind::TemplateLibrarySymbol
    );
    assert_eq!(outline[0].children[0].detail.as_deref(), Some("from i18n"));
    assert_eq!(
        outline[0].children[0].selection_span.start_usize(),
        "{% load ".len()
    );
}

#[test]
fn nested_blocks_produce_nested_outline_items() {
    let db = TestDatabase::new();
    let outline = outline_for_source(
        &db,
        r"{% block content %}
  {% block title %}Title{% endblock %}
{% endblock %}",
    );

    assert_eq!(labels(outline), vec!["content"]);
    assert_eq!(labels(&outline[0].children), vec!["title"]);
}

#[test]
fn standalone_tag_items_inside_blocks_are_nested() {
    let db = TestDatabase::new();
    let outline = outline_for_source(
        &db,
        r#"{% block content %}
  {% include "card.html" %}
{% endblock %}"#,
    );

    assert_eq!(labels(outline), vec!["content"]);
    assert_eq!(labels(&outline[0].children), vec!["card.html"]);
}

#[test]
fn custom_callable_block_tags_produce_callable_outline_items() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([(
        "partialdef".to_string(),
        TagSpec::new(
            Cow::Borrowed("django_template_partials.templatetags.partials"),
            Some(EndTag {
                name: Cow::Borrowed("endpartialdef"),
                required: true,
            }),
            Cow::Borrowed(&[]),
            false,
        )
        .with_role(TagRole::TemplateTag),
    )])));
    let db = TestDatabase::new().with_specs(specs);
    let outline = outline_for_source(&db, "{% partialdef card %}Body{% endpartialdef %}");

    assert_eq!(labels(outline), vec!["partialdef card"]);
    assert_eq!(outline[0].kind, OutlineKind::TemplateTag);
}

#[test]
fn tags_without_role_hide_standalone_tags_but_keep_blocks() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([
        (
            "myblock".to_string(),
            TagSpec::new(
                Cow::Borrowed("myapp.templatetags.custom"),
                Some(EndTag {
                    name: Cow::Borrowed("endmyblock"),
                    required: true,
                }),
                Cow::Borrowed(&[]),
                false,
            ),
        ),
        (
            "mytag".to_string(),
            TagSpec::new(
                Cow::Borrowed("myapp.templatetags.custom"),
                None,
                Cow::Borrowed(&[]),
                false,
            ),
        ),
    ])));
    let db = TestDatabase::new().with_specs(specs);
    let outline = outline_for_source(&db, "{% mytag %}{% myblock thing %}Body{% endmyblock %}");

    assert_eq!(labels(outline), vec!["myblock thing"]);
    assert_eq!(outline[0].kind, OutlineKind::ControlTag);
}

#[test]
fn control_structure_inside_blocks_is_visible() {
    let db = TestDatabase::new();
    let outline = outline_for_source(
        &db,
        r#"{% load static %}
{% block content %}
  <h1>Hello, {{ user.username|lower }}!</h1>
  {% if items %}
    {% for item in items %}<li>{{ item.name }}</li>{% endfor %}
  {% else %}
    <p>No items found.</p>
  {% endif %}
  <img src="{% static 'images/logo.png' %}" alt="Logo">
{% endblock %}"#,
    );

    assert_eq!(labels(outline), vec!["static", "content"]);
    let block_children = &outline[1].children;
    assert_eq!(
        labels(block_children),
        vec!["user.username", "if items", "images/logo.png"]
    );
    assert_eq!(block_children[0].kind, OutlineKind::Variable);
    assert_eq!(
        block_children[0].selection_span.start_usize(),
        r"{% load static %}
{% block content %}
  <h1>Hello, {{ "
            .len()
    );
    assert_eq!(labels(&block_children[0].children), vec!["lower"]);
    assert_eq!(block_children[0].children[0].kind, OutlineKind::Filter);
    assert_eq!(block_children[1].kind, OutlineKind::ControlTag);
    assert_eq!(
        block_children[1].selection_span.start_usize(),
        r"{% load static %}
{% block content %}
  <h1>Hello, {{ user.username|lower }}!</h1>
  {% "
        .len()
    );
    assert_eq!(
        labels(&block_children[1].children),
        vec!["for item in items", "else"]
    );
    assert_eq!(
        labels(&block_children[1].children[0].children),
        vec!["item.name"]
    );
    assert_eq!(block_children[2].kind, OutlineKind::StaticAssetReference);
}

#[test]
fn malformed_unclosed_blocks_produce_best_effort_outline_items() {
    let db = TestDatabase::new();
    let outline = outline_for_source(
        &db,
        r"{% block content %}
  {% if user %}
    {% block title %}Title",
    );

    assert_eq!(labels(outline), vec!["content"]);
    assert_eq!(labels(&outline[0].children), vec!["if user"]);
    assert_eq!(labels(&outline[0].children[0].children), vec!["title"]);
}
