use djls_source::Span;
use djls_templates::{Node, NodeList};

use super::shapes::{build_end_index, EndPolicy, IntermediateShape, TagForm, TagShape, TagShapes};
use crate::db::Db;

#[derive(Default)]
pub struct BlockTree {
    roots: Vec<BlockId>,
    blocks: Vec<Block>,
}

impl BlockTree {
    pub fn build(mut self, db: &dyn Db, nodelist: NodeList, shapes: &TagShapes) -> Self {
        let mut stack = TreeStack::new();

        let end_index = build_end_index(shapes);

        for node in nodelist.nodelist(db) {
            match node {
                Node::Tag { name, bits, span } => {
                    let tag_name = name.text(db);

                    if let Some(end) = end_index.get(&tag_name) {
                        while let Some(top) = stack.pop() {
                            if end.matches_opener(&top.opener_tag) {
                                #[allow(clippy::single_match, clippy::match_same_arms)]
                                match end.policy() {
                                    EndPolicy::MustMatchOpenName => {}
                                    _ => {}
                                }
                                self.close_block(top.opener_tag.to_string(), *span);
                                break;
                            }
                            self.error_close(
                                top.opener_span,
                                format!("Unclosed block '{}'", top.opener_tag),
                            );
                        }
                        if stack.is_empty() && end.opener() != "" {
                            // End with no matching opener at all
                            // (This triggers if the while loop never matched and stack got empty)
                            // Optionally: only emit if we didn't just close something
                            // For simplicity, emit an error in the else branch above instead.
                        }
                        continue;
                    }

                    if let Some(top) = stack.last_mut() {
                        if top
                            .intermediates
                            .iter()
                            .any(|shape| shape.name() == tag_name)
                        {
                            self.split_segment(tag_name.to_string(), *span);
                        }
                    }

                    match shapes.get(&tag_name).map(TagShape::form) {
                        Some(TagForm::Block { end, intermediates }) => {
                            self.open_block(tag_name.to_string(), *span);
                            stack.push(TreeFrame {
                                opener_tag: tag_name.to_string(),
                                opener_span: *span,
                                end_policy: end.policy(),
                                intermediates: intermediates.clone(),
                                open_name_arg: node
                                    .first_tag_bit()
                                    .map(|bit| bit.text(db).to_string()),
                            });
                        }
                        Some(TagForm::Leaf) | None => {
                            self.leaf(tag_name.to_string(), *span);
                        }
                    }
                }
                Node::Comment { span, .. } => self.leaf("<comment>".into(), *span),
                Node::Variable { span, .. } => self.leaf("<var>".into(), *span),
                Node::Text { span } => self.leaf("<text>".into(), *span),
                Node::Error {
                    full_span, error, ..
                } => self.leaf(error.to_string(), *full_span),
            }
        }

        while let Some(frame) = stack.pop() {
            match frame.end_policy {
                EndPolicy::Optional => {
                    // If you want, silently close optional blocks at EOF, or still emit a warning. Your call.
                    self.close_block(frame.opener_tag.to_string(), frame.opener_span);
                }
                EndPolicy::Required => {
                    self.error_close(
                        frame.opener_span,
                        format!("Unclosed block '{}'", frame.opener_tag),
                    );
                }
                EndPolicy::MustMatchOpenName => (),
            }
        }

        self
    }

    fn open_block(&mut self, name: String, at: Span) {
        /* start a Body */
    }

    fn split_segment(&mut self, label: String, at: Span) {
        /* start new sibling segment */
    }

    fn close_block(&mut self, name: String, at: Span) {
        /* end current Body */
    }

    fn leaf(&mut self, label: String, at: Span) {
        /* append leaf node */
    }

    fn error_end(&mut self, at: Span, msg: impl Into<String>) {
        /* record diag node */
    }

    fn error_close(&mut self, opener_at: Span, msg: impl Into<String>) {
        /* diag for EOF-miss */
    }

    fn error_segment(&mut self, at: Span, msg: impl Into<String>) {
        /* bogus intermediate */
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(u32);

pub struct Block {
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
        body: BlockId,
    },
    Segment {
        label: String,
        span: Span,
        body: BlockId,
    },
}

type TreeStack = Vec<TreeFrame>;

struct TreeFrame {
    opener_tag: String,
    opener_span: Span,
    end_policy: EndPolicy,
    intermediates: Vec<IntermediateShape>,
    open_name_arg: Option<String>,
}
