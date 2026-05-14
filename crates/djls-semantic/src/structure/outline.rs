use djls_source::Span;

use crate::structure::BlockRole;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;
use crate::Db;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TemplateOutline {
    pub items: Vec<OutlineItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutlineItem {
    pub label: String,
    pub detail: Option<String>,
    pub kind: OutlineKind,
    pub span: Span,
    pub selection_span: Span,
    pub children: Vec<OutlineItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutlineKind {
    Block,
    Control,
    Extends,
    Include,
    Load,
    Callable,
    Tag,
}

#[must_use]
pub fn build_template_outline(db: &dyn Db, tree: TemplateTree<'_>) -> TemplateOutline {
    let regions = tree.regions(db);
    let root = tree.root(db);

    TemplateOutline {
        items: outline_items_for_region(regions, root),
    }
}

fn outline_items_for_region(
    regions: &Regions,
    region: crate::structure::RegionId,
) -> Vec<OutlineItem> {
    regions
        .get(region)
        .nodes()
        .iter()
        .flat_map(|node| outline_items_for_node(regions, node))
        .collect()
}

fn outline_items_for_node(regions: &Regions, node: &TemplateNode) -> Vec<OutlineItem> {
    match node {
        TemplateNode::Block {
            tag,
            bits,
            marker_span,
            body,
            role: BlockRole::Opener,
            ..
        } => {
            let children = outline_items_for_block_container(regions, *body, tag);

            vec![OutlineItem {
                label: outline_label(tag, bits),
                detail: Some(tag.clone()),
                kind: outline_kind_for_block(tag),
                span: *regions.get(*body).span(),
                selection_span: *marker_span,
                children,
            }]
        }
        TemplateNode::Block {
            tag,
            bits,
            marker_span,
            full_span,
            body,
            role: BlockRole::Segment,
        } => {
            let children = outline_items_for_region(regions, *body);

            vec![OutlineItem {
                label: outline_label(tag, bits),
                detail: Some(tag.clone()),
                kind: OutlineKind::Control,
                span: *full_span,
                selection_span: *marker_span,
                children,
            }]
        }
        TemplateNode::StandaloneTag {
            tag,
            bits,
            full_span,
            marker_span,
        } => outline_kind_for_standalone(tag).map_or_else(Vec::new, |kind| {
            vec![OutlineItem {
                label: outline_label(tag, bits),
                detail: Some(tag.clone()),
                kind,
                span: *full_span,
                selection_span: *marker_span,
                children: Vec::new(),
            }]
        }),
        TemplateNode::Variable { .. }
        | TemplateNode::Comment { .. }
        | TemplateNode::Text { .. }
        | TemplateNode::Error { .. } => Vec::new(),
    }
}

fn outline_items_for_block_container(
    regions: &Regions,
    container: crate::structure::RegionId,
    opener_tag: &str,
) -> Vec<OutlineItem> {
    regions
        .get(container)
        .nodes()
        .iter()
        .flat_map(|node| match node {
            TemplateNode::Block {
                tag,
                body,
                role: BlockRole::Segment,
                ..
            } if tag == opener_tag => outline_items_for_region(regions, *body),
            _ => outline_items_for_node(regions, node),
        })
        .collect()
}

fn outline_kind_for_block(tag: &str) -> OutlineKind {
    match tag {
        "block" => OutlineKind::Block,
        "macro" | "partialdef" => OutlineKind::Callable,
        _ => OutlineKind::Control,
    }
}

fn outline_kind_for_standalone(tag: &str) -> Option<OutlineKind> {
    match tag {
        "extends" => Some(OutlineKind::Extends),
        "include" => Some(OutlineKind::Include),
        "load" => Some(OutlineKind::Load),
        "static" | "url" => Some(OutlineKind::Tag),
        _ => None,
    }
}

fn outline_label(tag: &str, bits: &[String]) -> String {
    if bits.is_empty() {
        tag.to_string()
    } else {
        format!("{} {}", tag, bits.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use djls_source::File;
    use djls_templates::parse_template;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::build_template_tree;
    use crate::builtin_tag_specs;
    use crate::testing::TestDatabase;
    use crate::EndTag;
    use crate::TagSpec;
    use crate::TagSpecs;

    fn outline_for_source(db: &TestDatabase, source: &str) -> TemplateOutline {
        db.add_file("test.html", source);
        let file = File::new(db, "test.html".into(), 0);
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
        let outline = outline_for_source(
            &db,
            r#"{% extends "base.html" %}
{% load static i18n %}
{% include "partials/nav.html" %}"#,
        );

        assert_eq!(
            labels(&outline.items),
            vec![
                "extends \"base.html\"",
                "load static i18n",
                "include \"partials/nav.html\"",
            ]
        );
        assert_eq!(
            outline
                .items
                .iter()
                .map(|item| item.kind)
                .collect::<Vec<_>>(),
            vec![
                OutlineKind::Extends,
                OutlineKind::Load,
                OutlineKind::Include,
            ]
        );
        assert_eq!(
            outline
                .items
                .iter()
                .map(|item| item.detail.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("extends"), Some("load"), Some("include")]
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

        assert_eq!(labels(&outline.items), vec!["block content"]);
        assert_eq!(labels(&outline.items[0].children), vec!["block title"]);
    }

    #[test]
    fn standalone_outline_items_inside_blocks_are_nested() {
        let db = TestDatabase::new();
        let outline = outline_for_source(
            &db,
            r#"{% block content %}
  {% include "card.html" %}
{% endblock %}"#,
        );

        assert_eq!(labels(&outline.items), vec!["block content"]);
        assert_eq!(
            labels(&outline.items[0].children),
            vec!["include \"card.html\""]
        );
    }

    #[test]
    fn custom_callable_block_tags_produce_callable_outline_items() {
        let mut specs = builtin_tag_specs();
        specs.merge(TagSpecs::new(FxHashMap::from_iter([(
            "partialdef".to_string(),
            TagSpec {
                module: Cow::Borrowed("django_template_partials.templatetags.partials"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endpartialdef"),
                    required: true,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: None,
            },
        )])));
        let db = TestDatabase::new().with_specs(specs);
        let outline = outline_for_source(&db, "{% partialdef card %}Body{% endpartialdef %}");

        assert_eq!(labels(&outline.items), vec!["partialdef card"]);
        assert_eq!(outline.items[0].kind, OutlineKind::Callable);
    }

    #[test]
    fn control_structure_inside_blocks_is_visible() {
        let db = TestDatabase::new();
        let outline = outline_for_source(
            &db,
            r#"{% load static %}
{% block content %}
  {% if items %}
    {% for item in items %}<li>{{ item.name }}</li>{% endfor %}
  {% else %}
    <p>No items found.</p>
  {% endif %}
  <img src="{% static 'images/logo.png' %}" alt="Logo">
{% endblock %}"#,
        );

        assert_eq!(labels(&outline.items), vec!["load static", "block content"]);
        let block_children = &outline.items[1].children;
        assert_eq!(
            labels(block_children),
            vec!["if items", "static 'images/logo.png'"]
        );
        assert_eq!(block_children[0].kind, OutlineKind::Control);
        assert_eq!(
            labels(&block_children[0].children),
            vec!["for item in items", "else"]
        );
        assert_eq!(block_children[1].kind, OutlineKind::Tag);
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

        assert_eq!(labels(&outline.items), vec!["block content"]);
        assert_eq!(labels(&outline.items[0].children), vec!["if user"]);
        assert_eq!(
            labels(&outline.items[0].children[0].children),
            vec!["block title"]
        );
    }
}
