use djls_source::Span;
use djls_templates::{Node, NodeList};

use super::shapes::{EndPolicy, TagShapes};
use crate::db::Db;

pub struct BlockTree {
    root: BodyId,
    bodies: Vec<Body>,
}

impl BlockTree {
    pub fn new() -> Self {
        Self {
            root: BodyId(0),
            bodies: Vec::new(),
        }
    }
    pub fn build(mut self, db: &dyn Db, nodelist: NodeList, shapes: &TagShapes) -> Self {
        let mut stack = TreeStack::new();

        for node in nodelist.nodelist(db) {
            match node {
                Node::Tag { .. } => (),
                Node::Comment { .. } => (),
                Node::Variable { .. } => (),
                Node::Text { .. } => (),
                Node::Error { .. } => (),
            }
        }

        self
    }
    fn leaf() {}
    fn open_block() {}
    fn split_segment() {}
    fn close_block() {}
    fn error_segment() {}
    fn error_end() {}
    fn error_close() {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BodyId(u32);

pub struct Body {
    span: Span,
    nodes: Vec<BlockNode>,
}

pub enum BlockNode {
    Leaf {
        span: Span,
    },
    Block {
        name: String,
        span: Span,
        body: BodyId,
    },
    Segment {
        label: String,
        span: Span,
        body: BodyId,
    },
}

type TreeStack = Vec<TreeFrame>;

struct TreeFrame {
    opener_name: String,
    opener_span: Span,
    end_policy: EndPolicy,
    intermediates: Vec<String>,
}
