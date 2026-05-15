use serde::Serialize;

use crate::structure::tree::RegionId;
use crate::structure::tree::TemplateNode;
use crate::structure::tree::TemplateTree;

#[derive(Serialize)]
pub(crate) struct TemplateTreeSnapshot {
    root: u32,
    regions: Vec<RegionSnapshot>,
}

impl TemplateTreeSnapshot {
    pub(crate) fn from_tree(tree: TemplateTree<'_>, db: &dyn crate::Db) -> Self {
        let root = tree.root(db);
        let regions_ref = tree.regions(db);

        let regions: Vec<RegionSnapshot> = regions_ref
            .iter()
            .map(|region| RegionSnapshot {
                span: *region.span(),
                parent: region.parent().map(RegionId::id),
                nodes: region.nodes().iter().map(NodeSnapshot::from).collect(),
            })
            .collect();

        Self {
            root: root.id(),
            regions,
        }
    }
}

#[derive(Serialize)]
struct RegionSnapshot {
    span: djls_source::Span,
    parent: Option<u32>,
    nodes: Vec<NodeSnapshot>,
}

#[derive(Serialize)]
#[serde(tag = "node")]
enum NodeSnapshot {
    BlockTag {
        tag: String,
        name_span: djls_source::Span,
        bits: Vec<djls_templates::TagBit>,
        full_span: djls_source::Span,
        body: u32,
        role: String,
    },
    StandaloneTag {
        tag: String,
        name_span: djls_source::Span,
        bits: Vec<djls_templates::TagBit>,
        full_span: djls_source::Span,
    },
    Variable {
        var: String,
        var_span: djls_source::Span,
        filters: Vec<djls_templates::Filter>,
        span: djls_source::Span,
    },
    Comment {
        span: djls_source::Span,
    },
    Text {
        span: djls_source::Span,
    },
    Error {
        span: djls_source::Span,
        full_span: djls_source::Span,
    },
}

impl From<&TemplateNode> for NodeSnapshot {
    fn from(node: &TemplateNode) -> Self {
        match node {
            TemplateNode::Block {
                tag,
                name_span,
                bits,
                full_span,
                body,
                role,
            } => Self::BlockTag {
                tag: tag.clone(),
                name_span: *name_span,
                bits: bits.clone(),
                full_span: *full_span,
                body: body.id(),
                role: format!("{role:?}"),
            },
            TemplateNode::StandaloneTag {
                tag,
                name_span,
                bits,
                full_span,
            } => Self::StandaloneTag {
                tag: tag.clone(),
                name_span: *name_span,
                bits: bits.clone(),
                full_span: *full_span,
            },
            TemplateNode::Variable {
                var,
                var_span,
                filters,
                span,
            } => Self::Variable {
                var: var.clone(),
                var_span: *var_span,
                filters: filters.clone(),
                span: *span,
            },
            TemplateNode::Comment { span } => Self::Comment { span: *span },
            TemplateNode::Text { span } => Self::Text { span: *span },
            TemplateNode::Error { span, full_span } => Self::Error {
                span: *span,
                full_span: *full_span,
            },
        }
    }
}
