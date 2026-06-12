use djls_source::Span;
use djls_templates::TagBit;

use crate::db::Db;
use crate::scoping::LoadKind;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;
use crate::tags::TagRole;
use crate::tags::TagSpec;
use crate::tags::TagSpecs;

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

impl From<TagRole> for OutlineKind {
    fn from(role: TagRole) -> Self {
        match role {
            TagRole::TemplateReference(_) => Self::TemplateReference,
            TagRole::TemplateLibraryLoader => Self::TemplateLibrary,
            TagRole::TemplateBlock => Self::TemplateBlock,
            TagRole::ControlTag => Self::ControlTag,
            TagRole::TemplateTag => Self::TemplateTag,
            TagRole::StaticAssetReference => Self::StaticAssetReference,
            TagRole::RouteReference => Self::RouteReference,
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
    role: TagRole,
    tag: &str,
    name_span: Span,
    bits: &[TagBit],
    span: Span,
    children: Vec<OutlineItem>,
) -> Vec<OutlineItem> {
    match role {
        TagRole::TemplateReference(_)
        | TagRole::TemplateBlock
        | TagRole::StaticAssetReference
        | TagRole::RouteReference => {
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
        TagRole::TemplateLibraryLoader => match LoadKind::from_tag(tag, bits) {
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
        TagRole::ControlTag | TagRole::TemplateTag => {
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
                .and_then(TagSpec::role)
                .unwrap_or(TagRole::ControlTag);
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
                TagRole::ControlTag,
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
            .and_then(TagSpec::role)
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
