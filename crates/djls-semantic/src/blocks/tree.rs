use std::ops::Deref;
use std::ops::DerefMut;

use djls_source::Span;
use djls_templates::Node;
use djls_templates::NodeList;

use super::shapes::build_end_index;
use super::shapes::EndPolicy;
use super::shapes::IntermediateShape;
use super::shapes::TagForm;
use super::shapes::TagShape;
use super::shapes::TagShapes;
use crate::db::Db;

pub struct BlockTree {
    roots: Vec<BlockId>,
    blocks: Blocks,
}

impl BlockTree {
    pub fn new() -> Self {
        let (blocks, root) = Blocks::with_root();
        Self {
            roots: vec![root],
            blocks,
        }
    }

    pub fn build(mut self, db: &dyn Db, nodelist: NodeList, shapes: &TagShapes) -> Self {
        let mut stack = TreeStack::default();

        let root = self.roots[0];
        let end_index = build_end_index(shapes);

        for node in nodelist.nodelist(db) {
            match node {
                Node::Tag { name, bits, span } => {
                    let tag_name = name.text(db);
                    let name_arg = bits.first().map(|bit| bit.text(db).to_string());

                    if let Some(end) = end_index.get(&tag_name) {
                        let opener = loop {
                            match stack.pop() {
                                Some(top) if end.matches_opener(&top.opener_tag) => {
                                    break Some(top)
                                }
                                Some(top) => {
                                    self.blocks.add_error(
                                        top.parent_body,
                                        format!("Unclosed block '{}'", top.opener_tag),
                                        top.opener_span,
                                    );
                                }
                                None => break None,
                            }
                        };

                        if let Some(top) = opener {
                            match top.decide_close(name_arg.as_deref(), &tag_name) {
                                CloseDecision::Close => {
                                    self.blocks.extend(top.container_body, *span);
                                }
                                CloseDecision::Restore { message } => {
                                    self.blocks.add_error(top.segment_body, message, *span);
                                    stack.push(top);
                                }
                            }
                        } else {
                            let target = stack.active_segment(root);
                            self.blocks.add_error(
                                target,
                                format!("Unexpected closing tag '{tag_name}'"),
                                *span,
                            );
                        }

                        continue;
                    }

                    if let Some(top) = stack.last_mut() {
                        if top
                            .intermediates
                            .iter()
                            .any(|shape| shape.name() == tag_name)
                        {
                            top.segment_body = self.blocks.add_segment(
                                top.container_body,
                                tag_name.to_string(),
                                *span,
                            );
                            continue;
                        } else if !top.intermediates.is_empty() {
                            self.blocks.add_error(
                                top.segment_body,
                                format!(
                                    "'{}' is not a valid intermediate for '{}'",
                                    tag_name, top.opener_tag
                                ),
                                *span,
                            );
                        }
                    }

                    match shapes.get(&tag_name).map(TagShape::form) {
                        Some(TagForm::Block { end, intermediates }) => {
                            let parent = stack.active_segment(root);
                            let (container, segment) =
                                self.blocks.add_block(parent, &tag_name, *span);
                            stack.push(TreeFrame {
                                opener_tag: tag_name.to_string(),
                                opener_span: *span,
                                end_policy: end.policy(),
                                intermediates: intermediates.clone(),
                                open_name_arg: name_arg.clone(),
                                parent_body: parent,
                                container_body: container,
                                segment_body: segment,
                            });
                        }
                        Some(TagForm::Leaf) | None => {
                            self.blocks.add_leaf(
                                stack.active_segment(root),
                                tag_name.to_string(),
                                *span,
                            );
                        }
                    }
                }
                Node::Comment { span, .. } => {
                    self.blocks
                        .add_leaf(stack.active_segment(root), "<comment>".into(), *span);
                }
                Node::Variable { span, .. } => {
                    self.blocks
                        .add_leaf(stack.active_segment(root), "<var>".into(), *span);
                }
                Node::Text { span } => {
                    self.blocks
                        .add_leaf(stack.active_segment(root), "<text>".into(), *span);
                }
                Node::Error {
                    full_span, error, ..
                } => {
                    self.blocks
                        .add_leaf(stack.active_segment(root), error.to_string(), *full_span);
                }
            }
        }

        while let Some(frame) = stack.pop() {
            match frame.end_policy {
                EndPolicy::Optional => {
                    self.blocks.extend(frame.container_body, frame.opener_span);
                }
                EndPolicy::Required => {
                    self.blocks.add_error(
                        frame.parent_body,
                        format!("Unclosed block '{}'", frame.opener_tag),
                        frame.opener_span,
                    );
                }
                EndPolicy::MustMatchOpenName => (),
            }
        }

        self
    }
}

impl Default for BlockTree {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(u32);

impl BlockId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Default)]
pub struct Blocks {
    entries: Vec<Block>,
}

impl Blocks {
    fn with_root() -> (Self, BlockId) {
        let mut blocks = Self::default();
        let root = blocks.alloc(Span::new(0, 0));
        (blocks, root)
    }

    fn alloc(&mut self, span: Span) -> BlockId {
        let id = BlockId(self.entries.len() as u32);
        self.entries.push(Block::new(span));
        id
    }

    fn add_block(&mut self, parent: BlockId, name: &str, span: Span) -> (BlockId, BlockId) {
        let container = self.alloc(span);

        self.push_node(
            parent,
            BlockNode::Block {
                name: name.to_string(),
                span,
                body: container,
            },
        );
        let segment = self.add_segment(container, name.to_string(), span);

        (container, segment)
    }

    fn add_segment(&mut self, container: BlockId, label: String, span: Span) -> BlockId {
        let segment = self.alloc(span);
        self.push_node(
            container,
            BlockNode::Segment {
                label,
                span,
                body: segment,
            },
        );
        segment
    }

    fn add_leaf(&mut self, target: BlockId, label: String, span: Span) {
        self.push_node(target, BlockNode::Leaf { label, span });
    }

    fn add_error(&mut self, target: BlockId, message: String, span: Span) {
        self.push_node(target, BlockNode::Error { message, span });
    }

    fn extend(&mut self, id: BlockId, span: Span) {
        self.block_mut(id).extend_span(span);
    }

    fn push_node(&mut self, target: BlockId, node: BlockNode) {
        let span = node.span();
        self.extend(target, span);
        self.block_mut(target).nodes.push(node);
    }

    fn block_mut(&mut self, id: BlockId) -> &mut Block {
        let idx = id.index();
        &mut self.entries[idx]
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    span: Span,
    nodes: Vec<BlockNode>,
}

impl Block {
    fn new(span: Span) -> Self {
        Self {
            span,
            nodes: Vec::new(),
        }
    }

    fn extend_span(&mut self, span: Span) {
        if self.nodes.is_empty() && self.span.length() == 0 {
            self.span = span;
            return;
        }

        let start = self.span.start().min(span.start());
        let end = self.span.end().max(span.end());
        self.span = Span::from_bounds(start as usize, end as usize);
    }
}

#[derive(Clone, Debug)]
pub enum BlockNode {
    Leaf {
        label: String,
        span: Span,
    },
    Block {
        name: String,
        span: Span,
        body: BlockId,
    },
    Segment {
        label: String,
        span: Span,
        body: BlockId,
    },
    Error {
        message: String,
        span: Span,
    },
}

impl BlockNode {
    fn span(&self) -> Span {
        match self {
            BlockNode::Leaf { span, .. }
            | BlockNode::Block { span, .. }
            | BlockNode::Segment { span, .. }
            | BlockNode::Error { span, .. } => *span,
        }
    }
}

#[derive(Default)]
struct TreeStack(Vec<TreeFrame>);

impl TreeStack {
    fn active_segment(&self, root: BlockId) -> BlockId {
        self.0.last().map_or(root, |frame| frame.segment_body)
    }
}

impl Deref for TreeStack {
    type Target = Vec<TreeFrame>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TreeStack {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

struct TreeFrame {
    opener_tag: String,
    opener_span: Span,
    end_policy: EndPolicy,
    intermediates: Vec<IntermediateShape>,
    open_name_arg: Option<String>,
    parent_body: BlockId,
    container_body: BlockId,
    segment_body: BlockId,
}

impl TreeFrame {
    fn decide_close(&self, closer_arg: Option<&str>, closer_tag: &str) -> CloseDecision {
        match self.end_policy {
            EndPolicy::Required | EndPolicy::Optional => CloseDecision::Close,
            EndPolicy::MustMatchOpenName => match (self.open_name_arg.as_deref(), closer_arg) {
                (Some(open), Some(close)) if open == close => CloseDecision::Close,
                (Some(open), Some(close)) => CloseDecision::Restore {
                    message: format!(
                        "Expected closing tag '{closer_tag}' to reference '{open}', got '{close}'",
                    ),
                },
                (Some(open), None) => CloseDecision::Restore {
                    message: format!(
                        "Closing tag '{closer_tag}' is missing the required name '{open}'",
                    ),
                },
                (None, Some(close)) => CloseDecision::Restore {
                    message: format!(
                        "Closing tag '{closer_tag}' should not include a name, found '{close}'",
                    ),
                },
                (None, None) => CloseDecision::Close,
            },
        }
    }
}

enum CloseDecision {
    Close,
    Restore { message: String },
}
