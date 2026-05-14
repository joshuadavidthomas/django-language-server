use djls_source::Span;
use djls_templates::Filter;
use djls_templates::Node;

use crate::structure::BlockRole;
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
}

#[must_use]
pub fn build_template_outline(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    tree: TemplateTree<'_>,
) -> TemplateOutline {
    let regions = tree.regions(db);
    let root = tree.root(db);
    let source_nodes = SourceNodes::new(nodelist.nodelist(db));

    TemplateOutline {
        items: outline_items_for_region(regions, db.tag_specs(), &source_nodes, root),
    }
}

fn outline_items_for_region(
    regions: &Regions,
    tag_specs: &TagSpecs,
    source_nodes: &SourceNodes<'_>,
    region: crate::structure::RegionId,
) -> Vec<OutlineItem> {
    regions
        .get(region)
        .nodes()
        .iter()
        .flat_map(|node| outline_items_for_node(regions, tag_specs, source_nodes, node))
        .collect()
}

fn outline_items_for_node(
    regions: &Regions,
    tag_specs: &TagSpecs,
    source_nodes: &SourceNodes<'_>,
    node: &TemplateNode,
) -> Vec<OutlineItem> {
    match node {
        TemplateNode::Block {
            tag,
            bits,
            marker_span,
            body,
            role: BlockRole::Opener,
            ..
        } => {
            let children =
                outline_items_for_block_container(regions, tag_specs, source_nodes, *body, tag);
            let role = outline_role_for_block(tag_specs, tag);

            vec![OutlineItem {
                label: outline_label(tag, bits, role),
                detail: Some(tag.clone()),
                kind: outline_kind(role),
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
            let children = outline_items_for_region(regions, tag_specs, source_nodes, *body);
            let role = TagOutlineRole::ControlFlow;

            vec![OutlineItem {
                label: outline_label(tag, bits, role),
                detail: Some(tag.clone()),
                kind: outline_kind(role),
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
        } => outline_items_for_standalone(tag_specs, tag, bits, *full_span, *marker_span),
        TemplateNode::Variable { span } => source_nodes
            .variable(*span)
            .map_or_else(Vec::new, |variable| vec![variable.outline_item(*span)]),
        TemplateNode::Comment { .. } | TemplateNode::Text { .. } | TemplateNode::Error { .. } => {
            Vec::new()
        }
    }
}

fn outline_items_for_block_container(
    regions: &Regions,
    tag_specs: &TagSpecs,
    source_nodes: &SourceNodes<'_>,
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
            } if tag == opener_tag => {
                outline_items_for_region(regions, tag_specs, source_nodes, *body)
            }
            _ => outline_items_for_node(regions, tag_specs, source_nodes, node),
        })
        .collect()
}

struct SourceNodes<'a> {
    variables: Vec<VariableNode<'a>>,
}

impl<'a> SourceNodes<'a> {
    fn new(nodes: &'a [Node]) -> Self {
        let variables = nodes
            .iter()
            .filter_map(|node| match node {
                Node::Variable { var, filters, span } => Some(VariableNode {
                    var,
                    filters,
                    span: *span,
                }),
                Node::Tag { .. }
                | Node::Comment { .. }
                | Node::Text { .. }
                | Node::Error { .. } => None,
            })
            .collect();

        Self { variables }
    }

    fn variable(&self, span: Span) -> Option<&VariableNode<'a>> {
        self.variables.iter().find(|variable| variable.span == span)
    }
}

struct VariableNode<'a> {
    var: &'a str,
    filters: &'a [Filter],
    span: Span,
}

impl VariableNode<'_> {
    fn outline_item(&self, span: Span) -> OutlineItem {
        OutlineItem {
            label: variable_label(self.var, self.filters),
            detail: Some("variable".to_string()),
            kind: OutlineKind::Variable,
            span,
            selection_span: span,
            children: Vec::new(),
        }
    }
}

fn variable_label(var: &str, filters: &[Filter]) -> String {
    let mut label = var.to_string();

    for filter in filters {
        label.push('|');
        label.push_str(&filter.name);
        if let Some(arg) = &filter.arg {
            label.push(':');
            label.push_str(arg);
        }
    }

    label
}

fn outline_items_for_standalone(
    tag_specs: &TagSpecs,
    tag: &str,
    bits: &[String],
    full_span: Span,
    marker_span: Span,
) -> Vec<OutlineItem> {
    let Some(role) = outline_role_for_standalone(tag_specs, tag) else {
        return Vec::new();
    };

    if role == TagOutlineRole::LibraryImport {
        return bits
            .iter()
            .map(|bit| OutlineItem {
                label: outline_target_label(std::slice::from_ref(bit)),
                detail: Some(tag.to_string()),
                kind: outline_kind(role),
                span: full_span,
                selection_span: marker_span,
                children: Vec::new(),
            })
            .collect();
    }

    vec![OutlineItem {
        label: outline_label(tag, bits, role),
        detail: Some(tag.to_string()),
        kind: outline_kind(role),
        span: full_span,
        selection_span: marker_span,
        children: Vec::new(),
    }]
}

fn outline_role_for_block(tag_specs: &TagSpecs, tag: &str) -> TagOutlineRole {
    tag_specs
        .get(tag)
        .and_then(|spec| spec.outline_role)
        .unwrap_or(TagOutlineRole::ControlFlow)
}

fn outline_role_for_standalone(tag_specs: &TagSpecs, tag: &str) -> Option<TagOutlineRole> {
    tag_specs.get(tag).and_then(|spec| spec.outline_role)
}

fn outline_kind(role: TagOutlineRole) -> OutlineKind {
    match role {
        TagOutlineRole::TemplateReference => OutlineKind::TemplateReference,
        TagOutlineRole::LibraryImport => OutlineKind::LibraryImport,
        TagOutlineRole::NamedRegion => OutlineKind::NamedRegion,
        TagOutlineRole::ControlFlow => OutlineKind::ControlFlow,
        TagOutlineRole::Callable => OutlineKind::Callable,
        TagOutlineRole::AssetReference => OutlineKind::FileReference,
        TagOutlineRole::RouteReference => OutlineKind::RouteReference,
    }
}

fn outline_label(tag: &str, bits: &[String], role: TagOutlineRole) -> String {
    match role {
        TagOutlineRole::TemplateReference
        | TagOutlineRole::LibraryImport
        | TagOutlineRole::NamedRegion
        | TagOutlineRole::AssetReference
        | TagOutlineRole::RouteReference => outline_target_label(bits),
        TagOutlineRole::ControlFlow | TagOutlineRole::Callable => outline_tag_label(tag, bits),
    }
}

fn outline_tag_label(tag: &str, bits: &[String]) -> String {
    if bits.is_empty() {
        tag.to_string()
    } else {
        format!("{} {}", tag, bits.join(" "))
    }
}

fn outline_target_label(bits: &[String]) -> String {
    if bits.is_empty() {
        return String::new();
    }

    bits.iter()
        .map(|bit| strip_quotes(bit).unwrap_or(bit).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_quotes(value: &str) -> Option<&str> {
    value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
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
        build_template_outline(db, nodelist, tree)
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
    fn standalone_outline_items_inside_blocks_are_nested() {
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
            vec!["user.username|lower", "if items", "images/logo.png"]
        );
        assert_eq!(block_children[0].kind, OutlineKind::Variable);
        assert_eq!(block_children[1].kind, OutlineKind::ControlFlow);
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
