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
                    let name_arg = bits.first().map(|bit| bit.text(db));

                    if let Some(end) = end_index.get(&tag_name) {
                        let opener = loop {
                            match stack.pop() {
                                Some(top) if end.matches_opener(&top.opener_tag) => {
                                    break Some(top)
                                }
                                Some(top) => {
                                    self.error_close(
                                        top.opener_span,
                                        format!("Unclosed block '{}'", top.opener_tag),
                                    );
                                }
                                None => break None,
                            }
                        };

                        if let Some(top) = opener {
                            match top.decide_close(name_arg.as_deref(), &tag_name) {
                                CloseDecision::Close => {
                                    self.close_block(top.opener_tag.clone(), *span);
                                }
                                CloseDecision::Restore { message } => {
                                    self.error_end(*span, message);
                                    stack.push(top);
                                }
                            }
                        } else {
                            self.error_end(*span, format!("Unexpected closing tag '{}'", tag_name));
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
                            continue;
                        } else if !top.intermediates.is_empty() {
                            self.error_segment(
                                *span,
                                format!(
                                    "'{}' is not a valid intermediate for '{}'",
                                    tag_name, top.opener_tag
                                ),
                            );
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
                                open_name_arg: name_arg.clone(),
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
        root: BlockId,
        name: String,
        span: Span,
    },
    Segment {
        root: BlockId,
        label: String,
        span: Span,
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

impl TreeFrame {
    fn decide_close(&self, closer_arg: Option<&str>, closer_tag: &str) -> CloseDecision {
        match self.end_policy {
            EndPolicy::Required | EndPolicy::Optional => CloseDecision::Close,
            EndPolicy::MustMatchOpenName => match (self.open_name_arg.as_deref(), closer_arg) {
                (Some(open), Some(close)) if open == close => CloseDecision::Close,
                (Some(open), Some(close)) => CloseDecision::Restore {
                    message: format!(
                        "Expected closing tag '{}' to reference '{}', got '{}'",
                        closer_tag, open, close
                    ),
                },
                (Some(open), None) => CloseDecision::Restore {
                    message: format!(
                        "Closing tag '{}' is missing the required name '{}'",
                        closer_tag, open
                    ),
                },
                (None, Some(close)) => CloseDecision::Restore {
                    message: format!(
                        "Closing tag '{}' should not include a name, found '{}'",
                        closer_tag, close
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
