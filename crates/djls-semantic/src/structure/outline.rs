use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagArgument;

use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;
use crate::Db;
use crate::TagOutlineRole;
use crate::TagOutlineSpec;
use crate::TagOutlineTarget;
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

impl OutlineItem {
    fn tag_name(
        tag: &str,
        tag_span: Span,
        arguments: &[TagArgument],
        spec: TagOutlineSpec,
        span: Span,
        children: Vec<Self>,
    ) -> Self {
        let mut label = tag.to_string();
        for argument in arguments {
            label.push(' ');
            label.push_str(argument.as_str());
        }

        Self {
            label,
            detail: Some(tag.to_string()),
            kind: spec.role.into(),
            span,
            selection_span: tag_span,
            children,
        }
    }

    fn argument(
        tag: &str,
        argument: &TagArgument,
        spec: TagOutlineSpec,
        span: Span,
        children: Vec<Self>,
    ) -> Self {
        Self {
            label: argument.template_string().value().to_string(),
            detail: Some(tag.to_string()),
            kind: spec.role.into(),
            span,
            selection_span: argument.span,
            children,
        }
    }

    fn variable(var: &str, var_span: Span, filters: &[Filter], span: Span) -> Self {
        Self {
            label: var.to_string(),
            detail: Some("variable".to_string()),
            kind: OutlineKind::Variable,
            span,
            selection_span: var_span,
            children: filters.iter().map(Self::filter).collect(),
        }
    }

    fn filter(filter: &Filter) -> Self {
        Self {
            label: filter.label(),
            detail: Some("filter".to_string()),
            kind: OutlineKind::Filter,
            span: filter.span,
            selection_span: filter.span.with_length_usize_saturating(filter.name.len()),
            children: Vec::new(),
        }
    }
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

impl TagOutlineSpec {
    fn items_for_tag(
        self,
        tag: &str,
        tag_span: Span,
        arguments: &[TagArgument],
        span: Span,
        children: Vec<OutlineItem>,
    ) -> Vec<OutlineItem> {
        match self.target {
            TagOutlineTarget::TagName => vec![OutlineItem::tag_name(
                tag, tag_span, arguments, self, span, children,
            )],
            TagOutlineTarget::FirstArgument => {
                if let Some(argument) = arguments.first() {
                    vec![OutlineItem::argument(tag, argument, self, span, children)]
                } else {
                    vec![OutlineItem::tag_name(
                        tag, tag_span, arguments, self, span, children,
                    )]
                }
            }
            TagOutlineTarget::EachArgument => arguments
                .iter()
                .map(|argument| OutlineItem::argument(tag, argument, self, span, Vec::new()))
                .collect(),
        }
    }
}

#[must_use]
pub fn build_template_outline(db: &dyn Db, tree: TemplateTree<'_>) -> TemplateOutline {
    let regions = tree.regions(db);
    let root = tree.root(db);

    TemplateOutline {
        items: outline_items_for_region(regions, db.tag_specs(), root),
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
            tag_span,
            arguments,
            body,
            role: BlockRole::Opener,
            ..
        } => {
            let spec = tag_specs.block_outline(tag);
            let children = outline_items_for_block_container(regions, tag_specs, *body, tag);
            spec.items_for_tag(
                tag,
                *tag_span,
                arguments,
                *regions.get(*body).span(),
                children,
            )
        }
        TemplateNode::Block {
            tag,
            tag_span,
            arguments,
            full_span,
            body,
            role: BlockRole::Segment,
            ..
        } => {
            let spec: TagOutlineSpec = TagOutlineRole::ControlFlow.into();
            let children = outline_items_for_region(regions, tag_specs, *body);
            spec.items_for_tag(tag, *tag_span, arguments, *full_span, children)
        }
        TemplateNode::StandaloneTag {
            tag,
            tag_span,
            arguments,
            full_span,
            ..
        } => tag_specs
            .standalone_outline(tag)
            .map_or_else(Vec::new, |spec| {
                spec.items_for_tag(tag, *tag_span, arguments, *full_span, Vec::new())
            }),
        TemplateNode::Variable {
            var,
            var_span,
            filters,
            span,
        } => vec![OutlineItem::variable(var, *var_span, filters, *span)],
        TemplateNode::Comment { .. } | TemplateNode::Text { .. } | TemplateNode::Error { .. } => {
            Vec::new()
        }
    }
}

fn outline_items_for_block_container(
    regions: &Regions,
    tag_specs: &TagSpecs,
    container: RegionId,
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
            } if tag == opener_tag => outline_items_for_region(regions, tag_specs, *body),
            _ => outline_items_for_node(regions, tag_specs, node),
        })
        .collect()
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
