use djls_source::Span;
use djls_templates::TagBit;

use crate::db::Db;
use crate::scoping::LoadKind;
use crate::specs::tags::TagSemanticRole;
use crate::specs::tags::TagSpecs;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutlineItem {
    pub label: String,
    pub detail: Option<String>,
    pub kind: OutlineKind,
    pub span: Span,
    pub selection_span: Span,
    pub children: Vec<OutlineItem>,
}

/// Kind of template-domain item represented in the outline.
///
/// The template outline is a navigational projection over template semantics,
/// not the source of truth for every semantic fact in a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutlineKind {
    TemplateBlock,
    ControlTag,
    TemplateReference,
    TemplateLibrary,
    TemplateLibrarySymbol,
    TemplateTag,
    StaticAssetReference,
    RouteReference,
    Variable,
    Filter,
}

impl From<TagSemanticRole> for OutlineKind {
    fn from(role: TagSemanticRole) -> Self {
        match role {
            TagSemanticRole::TemplateReference => Self::TemplateReference,
            TagSemanticRole::TemplateLibraryLoader => Self::TemplateLibrary,
            TagSemanticRole::TemplateBlock => Self::TemplateBlock,
            TagSemanticRole::ControlTag => Self::ControlTag,
            TagSemanticRole::TemplateTag => Self::TemplateTag,
            TagSemanticRole::StaticAssetReference => Self::StaticAssetReference,
            TagSemanticRole::RouteReference => Self::RouteReference,
        }
    }
}

#[salsa::tracked(returns(ref))]
pub fn build_template_outline(db: &dyn Db, tree: TemplateTree<'_>) -> Vec<OutlineItem> {
    let regions = tree.regions(db);
    let root = tree.root(db);

    outline_items_for_region(regions, db.tag_specs(), root)
}

fn outline_items_for_tag(
    role: TagSemanticRole,
    tag: &str,
    name_span: Span,
    bits: &[TagBit],
    span: Span,
    children: Vec<OutlineItem>,
) -> Vec<OutlineItem> {
    match role {
        TagSemanticRole::TemplateReference
        | TagSemanticRole::TemplateBlock
        | TagSemanticRole::StaticAssetReference
        | TagSemanticRole::RouteReference => {
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
        TagSemanticRole::TemplateLibraryLoader => match LoadKind::from_tag(tag, bits) {
            Some(LoadKind::FullLoad { libraries }) => libraries
                .into_iter()
                .map(|library| OutlineItem {
                    label: library.as_str().to_string(),
                    detail: Some(tag.to_string()),
                    kind: role.into(),
                    span,
                    selection_span: library.span(),
                    children: Vec::new(),
                })
                .collect(),
            Some(LoadKind::SelectiveImport { symbols, library }) => vec![OutlineItem {
                label: library.as_str().to_string(),
                detail: Some(tag.to_string()),
                kind: role.into(),
                span,
                selection_span: library.span(),
                children: symbols
                    .into_iter()
                    .map(|symbol| OutlineItem {
                        label: symbol.as_str().to_string(),
                        detail: Some(format!("from {}", library.as_str())),
                        kind: OutlineKind::TemplateLibrarySymbol,
                        span,
                        selection_span: symbol.span(),
                        children: Vec::new(),
                    })
                    .collect(),
            }],
            None => Vec::new(),
        },
        TagSemanticRole::ControlTag | TagSemanticRole::TemplateTag => {
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
                .and_then(|spec| spec.semantic_role)
                .unwrap_or(TagSemanticRole::ControlTag);
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
                TagSemanticRole::ControlTag,
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
            .and_then(|spec| spec.semantic_role)
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
            detail: None,
            kind: OutlineKind::Variable,
            span: *span,
            selection_span: *var_span,
            children: filters
                .iter()
                .map(|filter| OutlineItem {
                    label: filter.label(),
                    detail: None,
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

    use camino::Utf8Path;
    use djls_templates::parse_template;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::build_template_tree;
    use crate::builtin_tag_specs;
    use crate::testing::TestDatabase;
    use crate::EndTag;
    use crate::TagSpec;
    use crate::TagSpecs;

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
        let outline = outline_for_source(
            &db,
            r#"{% extends "base.html" %}
{% load static i18n %}
{% include "partials/nav.html" %}"#,
        );

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
            TagSpec {
                module: Cow::Borrowed("django_template_partials.templatetags.partials"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endpartialdef"),
                    required: true,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                semantic_role: Some(TagSemanticRole::TemplateTag),
                extracted_rules: None,
            },
        )])));
        let db = TestDatabase::new().with_specs(specs);
        let outline = outline_for_source(&db, "{% partialdef card %}Body{% endpartialdef %}");

        assert_eq!(labels(outline), vec!["partialdef card"]);
        assert_eq!(outline[0].kind, OutlineKind::TemplateTag);
    }

    #[test]
    fn tags_without_semantic_role_hide_standalone_tags_but_keep_blocks() {
        let mut specs = builtin_tag_specs();
        specs.merge(TagSpecs::new(FxHashMap::from_iter([
            (
                "myblock".to_string(),
                TagSpec {
                    module: Cow::Borrowed("myapp.templatetags.custom"),
                    end_tag: Some(EndTag {
                        name: Cow::Borrowed("endmyblock"),
                        required: true,
                    }),
                    intermediate_tags: Cow::Borrowed(&[]),
                    opaque: false,
                    semantic_role: None,
                    extracted_rules: None,
                },
            ),
            (
                "mytag".to_string(),
                TagSpec {
                    module: Cow::Borrowed("myapp.templatetags.custom"),
                    end_tag: None,
                    intermediate_tags: Cow::Borrowed(&[]),
                    opaque: false,
                    semantic_role: None,
                    extracted_rules: None,
                },
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
}
