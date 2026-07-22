use djls_source::File;
use djls_source::Span;
use djls_templates::NodeList;
use djls_templates::TagBit;
use djls_templates::TemplateString;

use crate::TagSpec;
use crate::db::Db;
use crate::scoping::LoadKind;
use crate::scoping::ScopedTagFacts;
use crate::scoping::template_analysis_projection_for_file;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::tags::TagRole;

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
            TagRole::TemplatePartial | TagRole::TemplateTag => Self::TemplateTag,
            TagRole::StaticAssetReference => Self::StaticAssetReference,
            TagRole::RouteReference => Self::RouteReference,
        }
    }
}

#[salsa::tracked(returns(ref))]
pub fn build_template_outline_for_file(
    db: &dyn Db,
    file: File,
    nodelist: NodeList<'_>,
) -> Vec<OutlineItem> {
    let projection = template_analysis_projection_for_file(db, file, nodelist);
    let tree = projection.tree(db);
    let roles = OutlineTagRoles {
        facts: projection.scoped_tag_facts(db),
    };
    outline_items_for_region(tree.regions(db), &roles, tree.root(db))
}

struct OutlineTagRoles<'a> {
    facts: &'a ScopedTagFacts,
}

impl OutlineTagRoles<'_> {
    fn role(&self, name_span: Span) -> Option<TagRole> {
        self.facts
            .for_name_span(name_span)
            .and_then(|fact| fact.spec.as_ref())
            .and_then(TagSpec::role)
    }
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
        | TagRole::TemplatePartial
        | TagRole::StaticAssetReference
        | TagRole::RouteReference => {
            let item = if let Some(bit) = bits.first() {
                let (label, selection_span) = match bit.template_string() {
                    TemplateString::Quoted { value, span } => (value.to_string(), span),
                    TemplateString::Unquoted(value) => (value.to_string(), bit.span),
                };

                OutlineItem {
                    label,
                    detail: Some(tag.to_string()),
                    kind: role.into(),
                    span,
                    selection_span,
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
        TagRole::TemplateLibraryLoader => match LoadKind::from_loader_bits(bits) {
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
    roles: &OutlineTagRoles<'_>,
    region: RegionId,
) -> Vec<OutlineItem> {
    regions
        .get(region)
        .nodes()
        .iter()
        .flat_map(|node| outline_items_for_node(regions, roles, node))
        .collect()
}

#[allow(clippy::too_many_lines)]
fn outline_items_for_node(
    regions: &Regions,
    roles: &OutlineTagRoles<'_>,
    node: &TemplateNode,
) -> Vec<OutlineItem> {
    match node {
        TemplateNode::Block {
            tag,
            name_span,
            bits,
            full_span: _,
            body,
            role: BlockRole::Opener,
        } => {
            let role = roles.role(*name_span).unwrap_or(TagRole::ControlTag);
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
                        outline_items_for_region(regions, roles, *segment_body)
                    }
                    TemplateNode::Block { .. }
                    | TemplateNode::Opaque { .. }
                    | TemplateNode::StandaloneTag { .. }
                    | TemplateNode::Variable { .. }
                    | TemplateNode::Comment { .. }
                    | TemplateNode::Text { .. }
                    | TemplateNode::Error { .. } => outline_items_for_node(regions, roles, node),
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
            let children = outline_items_for_region(regions, roles, *body);
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
        } => roles.role(*name_span).map_or_else(Vec::new, |role| {
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
        TemplateNode::Opaque { .. }
        | TemplateNode::Comment { .. }
        | TemplateNode::Text { .. }
        | TemplateNode::Error { .. } => Vec::new(),
    }
}
