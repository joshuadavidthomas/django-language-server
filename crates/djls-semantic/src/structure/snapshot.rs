use serde::Serialize;

use crate::structure::tree::RegionId;
use crate::structure::tree::TemplateNode;
use crate::structure::tree::TemplateTree;

#[derive(Serialize)]
pub struct TemplateTreeSnapshot {
    root: u32,
    regions: Vec<RegionSnapshot>,
}

impl TemplateTreeSnapshot {
    pub fn from_tree(tree: TemplateTree<'_>, db: &dyn crate::Db) -> Self {
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
    Block {
        tag: String,
        bits: Vec<String>,
        marker_span: djls_source::Span,
        full_span: djls_source::Span,
        body: u32,
        role: String,
    },
    StandaloneTag {
        tag: String,
        bits: Vec<String>,
        marker_span: djls_source::Span,
        full_span: djls_source::Span,
    },
    Variable {
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
                bits,
                marker_span,
                full_span,
                body,
                role,
            } => Self::Block {
                tag: tag.clone(),
                bits: bits.clone(),
                marker_span: *marker_span,
                full_span: *full_span,
                body: body.id(),
                role: format!("{role:?}"),
            },
            TemplateNode::StandaloneTag {
                tag,
                bits,
                marker_span,
                full_span,
            } => Self::StandaloneTag {
                tag: tag.clone(),
                bits: bits.clone(),
                marker_span: *marker_span,
                full_span: *full_span,
            },
            TemplateNode::Variable { span } => Self::Variable { span: *span },
            TemplateNode::Comment { span } => Self::Comment { span: *span },
            TemplateNode::Text { span } => Self::Text { span: *span },
            TemplateNode::Error { span, full_span } => Self::Error {
                span: *span,
                full_span: *full_span,
            },
        }
    }
}
