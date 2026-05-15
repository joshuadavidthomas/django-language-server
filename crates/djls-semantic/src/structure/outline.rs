use djls_source::Span;
use djls_templates::TagBit;

use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;
use crate::Db;
use crate::TagOutlineRole;
use crate::TagSpecs;

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
    NamedRegion,
    ControlFlow,
    TemplateReference,
    LibraryImport,
    Callable,
    FileReference,
    RouteReference,
    Variable,
    Filter,
}

impl From<TagOutlineRole> for OutlineKind {
    fn from(role: TagOutlineRole) -> Self {
        match role {
            TagOutlineRole::TemplateReference => Self::TemplateReference,
            TagOutlineRole::LibraryImport => Self::LibraryImport,
            TagOutlineRole::NamedRegion => Self::NamedRegion,
            TagOutlineRole::ControlFlow => Self::ControlFlow,
            TagOutlineRole::Callable => Self::Callable,
            TagOutlineRole::AssetReference => Self::FileReference,
            TagOutlineRole::RouteReference => Self::RouteReference,
        }
    }
}

#[salsa::tracked(returns(ref))]
pub fn build_template_outline(db: &dyn Db, tree: TemplateTree<'_>) -> TemplateOutline {
    let regions = tree.regions(db);
    let root = tree.root(db);

    TemplateOutline {
        items: outline_items_for_region(regions, db.tag_specs(), root),
    }
}

fn outline_items_for_tag(
    role: TagOutlineRole,
    tag: &str,
    name_span: Span,
    bits: &[TagBit],
    span: Span,
    children: Vec<OutlineItem>,
) -> Vec<OutlineItem> {
    match role {
        TagOutlineRole::TemplateReference
        | TagOutlineRole::NamedRegion
        | TagOutlineRole::AssetReference
        | TagOutlineRole::RouteReference => {
            let item = if let Some(bit) = bits.first() {
                OutlineItem {
                    label: bit.template_string().value().to_string(),
                    detail: Some(tag.to_string()),
                    kind: role.into(),
                    span,
                    selection_span: bit.span,
                    children,
                }
            } else {
                OutlineItem {
                    label: tag.to_string(),
                    detail: Some(tag.to_string()),
                    kind: role.into(),
                    span,
                    selection_span: name_span,
                    children,
                }
            };
            vec![item]
        }
        TagOutlineRole::LibraryImport => bits
            .iter()
            .map(|bit| OutlineItem {
                label: bit.template_string().value().to_string(),
                detail: Some(tag.to_string()),
                kind: role.into(),
                span,
                selection_span: bit.span,
                children: Vec::new(),
            })
            .collect(),
        TagOutlineRole::ControlFlow | TagOutlineRole::Callable => {
            let mut label = tag.to_string();
            for bit in bits {
                label.push(' ');
                label.push_str(bit.as_str());
            }

            vec![OutlineItem {
                label,
                detail: Some(tag.to_string()),
                kind: role.into(),
                span,
                selection_span: name_span,
                children,
            }]
        }
    }
}

fn outline_items_for_region(
    regions: &Regions,
    tag_specs: &TagSpecs,
    region: RegionId,
) -> Vec<OutlineItem> {
    regions
        .get(region)
        .nodes()
        .iter()
        .flat_map(|node| outline_items_for_node(regions, tag_specs, node))
        .collect()
}

fn outline_items_for_node(
    regions: &Regions,
    tag_specs: &TagSpecs,
    node: &TemplateNode,
) -> Vec<OutlineItem> {
    match node {
        TemplateNode::Block {
            tag,
            name_span,
            bits,
            body,
            role: BlockRole::Opener,
            ..
        } => {
            let role = tag_specs
                .get(tag)
                .and_then(|spec| spec.outline_role)
                .unwrap_or(TagOutlineRole::ControlFlow);
            let children = regions
                .get(*body)
                .nodes()
                .iter()
                .flat_map(|node| match node {
                    TemplateNode::Block {
                        tag: segment_tag,
                        body: segment_body,
                        role: BlockRole::Segment,
                        ..
                    } if segment_tag == tag => {
                        outline_items_for_region(regions, tag_specs, *segment_body)
                    }
                    _ => outline_items_for_node(regions, tag_specs, node),
                })
                .collect();

            outline_items_for_tag(
                role,
                tag,
                *name_span,
                bits,
                *regions.get(*body).span(),
                children,
            )
        }
        TemplateNode::Block {
            tag,
            name_span,
            bits,
            full_span,
            body,
            role: BlockRole::Segment,
            ..
        } => {
            let children = outline_items_for_region(regions, tag_specs, *body);
            outline_items_for_tag(
                TagOutlineRole::ControlFlow,
                tag,
                *name_span,
                bits,
                *full_span,
                children,
            )
        }
        TemplateNode::StandaloneTag {
            tag,
            name_span,
            bits,
            full_span,
            ..
        } => tag_specs
            .get(tag)
            .and_then(|spec| spec.outline_role)
            .map_or_else(Vec::new, |role| {
                outline_items_for_tag(role, tag, *name_span, bits, *full_span, Vec::new())
            }),
        TemplateNode::Variable {
            var,
            var_span,
            filters,
            span,
        } => vec![OutlineItem {
            label: var.clone(),
            detail: Some("variable".to_string()),
            kind: OutlineKind::Variable,
            span: *span,
            selection_span: *var_span,
            children: filters
                .iter()
                .map(|filter| OutlineItem {
                    label: filter.label(),
                    detail: Some("filter".to_string()),
                    kind: OutlineKind::Filter,
                    span: filter.span,
                    selection_span: filter.span.with_length_usize_saturating(filter.name.len()),
                    children: Vec::new(),
                })
                .collect(),
        }],
        TemplateNode::Comment { .. } | TemplateNode::Text { .. } | TemplateNode::Error { .. } => {
            Vec::new()
        }
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

    fn outline_for_source<'db>(db: &'db TestDatabase, source: &str) -> &'db TemplateOutline {
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
            vec!["base.html", "static", "i18n", "partials/nav.html"]
        );
        assert_eq!(
            outline
                .items
                .iter()
                .map(|item| item.kind)
                .collect::<Vec<_>>(),
            vec![
                OutlineKind::TemplateReference,
                OutlineKind::LibraryImport,
                OutlineKind::LibraryImport,
                OutlineKind::TemplateReference,
            ]
        );
        assert_eq!(
            outline
                .items
                .iter()
                .map(|item| item.detail.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("extends"), Some("load"), Some("load"), Some("include")]
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

        assert_eq!(labels(&outline.items), vec!["content"]);
        assert_eq!(labels(&outline.items[0].children), vec!["title"]);
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

        assert_eq!(labels(&outline.items), vec!["content"]);
        assert_eq!(labels(&outline.items[0].children), vec!["card.html"]);
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
                outline_role: Some(crate::TagOutlineRole::Callable),
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
  <h1>Hello, {{ user.username|lower }}!</h1>
  {% if items %}
    {% for item in items %}<li>{{ item.name }}</li>{% endfor %}
  {% else %}
    <p>No items found.</p>
  {% endif %}
  <img src="{% static 'images/logo.png' %}" alt="Logo">
{% endblock %}"#,
        );

        assert_eq!(labels(&outline.items), vec!["static", "content"]);
        let block_children = &outline.items[1].children;
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
        assert_eq!(block_children[1].kind, OutlineKind::ControlFlow);
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
        assert_eq!(block_children[2].kind, OutlineKind::FileReference);
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

        assert_eq!(labels(&outline.items), vec!["content"]);
        assert_eq!(labels(&outline.items[0].children), vec!["if user"]);
        assert_eq!(
            labels(&outline.items[0].children[0].children),
            vec!["title"]
        );
    }
}
