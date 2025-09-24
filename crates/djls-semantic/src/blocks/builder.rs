use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;

use super::grammar::CloseValidation;
use super::grammar::TagClass;
use super::grammar::TagIndex;
use super::tree::BlockId;
use super::tree::BlockNode;
use super::tree::BlockTree;
use super::tree::BranchKind;
use crate::traits::SemanticModel;
use crate::Db;

#[derive(Debug, Clone)]
enum BlockSemanticOp {
    AddRoot {
        id: BlockId,
    },
    AddBranchNode {
        target: BlockId,
        tag: String,
        marker_span: Span,
        body: BlockId,
        kind: BranchKind,
    },
    AddErrorNode {
        target: BlockId,
        message: String,
        span: Span,
    },
    AddLeafNode {
        target: BlockId,
        label: String,
        span: Span,
    },
    ExtendBlockSpan {
        id: BlockId,
        span: Span,
    },
    FinalizeSpanTo {
        id: BlockId,
        end: u32,
    },
}

pub struct BlockTreeBuilder<'db> {
    db: &'db dyn Db,
    index: &'db TagIndex,
    stack: Vec<TreeFrame>,
    block_allocs: Vec<(Span, Option<BlockId>)>,
    semantic_ops: Vec<BlockSemanticOp>,
}

impl<'db> BlockTreeBuilder<'db> {
    #[allow(dead_code)] // use is gated behind cfg(test) for now
    pub fn new(db: &'db dyn Db, index: &'db TagIndex) -> Self {
        Self {
            db,
            index,
            stack: Vec::new(),
            block_allocs: Vec::new(),
            semantic_ops: Vec::new(),
        }
    }

    /// Allocate a new `BlockId` and track its metadata for later creation
    fn alloc_block_id(&mut self, span: Span, parent: Option<BlockId>) -> BlockId {
        let id = BlockId::new(u32::try_from(self.block_allocs.len()).unwrap_or_default());
        self.block_allocs.push((span, parent));
        id
    }

    /// Apply all semantic operations to build a `BlockTree`
    fn apply_operations(self) -> BlockTree {
        let mut tree = BlockTree::new();

        // Allocate all blocks using metadata
        for (span, parent) in self.block_allocs {
            if let Some(p) = parent {
                tree.blocks_mut().alloc(span, Some(p));
            } else {
                tree.blocks_mut().alloc(span, None);
            }
        }

        for op in self.semantic_ops {
            match op {
                BlockSemanticOp::AddRoot { id } => {
                    tree.roots_mut().push(id);
                }
                BlockSemanticOp::AddBranchNode {
                    target,
                    tag,
                    marker_span,
                    body,
                    kind,
                } => {
                    tree.blocks_mut().push_node(
                        target,
                        BlockNode::Branch {
                            tag,
                            marker_span,
                            body,
                            kind,
                        },
                    );
                }
                BlockSemanticOp::AddLeafNode {
                    target,
                    label,
                    span,
                } => {
                    tree.blocks_mut()
                        .push_node(target, BlockNode::Leaf { label, span });
                }
                BlockSemanticOp::AddErrorNode {
                    target,
                    message,
                    span,
                } => {
                    tree.blocks_mut()
                        .push_node(target, BlockNode::Error { message, span });
                }
                BlockSemanticOp::ExtendBlockSpan { id, span } => {
                    tree.blocks_mut().extend_block(id, span);
                }
                BlockSemanticOp::FinalizeSpanTo { id, end } => {
                    tree.blocks_mut().finalize_block_span(id, end);
                }
            }
        }

        tree
    }

    fn handle_tag(&mut self, name: &String, bits: &Vec<String>, span: Span) {
        let tag_name = name;
        match self.index.classify(tag_name) {
            TagClass::Opener => {
                let parent = get_active_segment(&self.stack);

                let container = self.alloc_block_id(span, parent);
                let segment = self.alloc_block_id(
                    Span::new(span.end().saturating_add(TagDelimiter::LENGTH_U32), 0),
                    Some(container),
                );

                if let Some(parent_id) = parent {
                    // Nested block
                    self.semantic_ops.push(BlockSemanticOp::AddBranchNode {
                        target: parent_id,
                        tag: tag_name.clone(),
                        marker_span: span,
                        body: container,
                        kind: BranchKind::Opener,
                    });
                    self.semantic_ops.push(BlockSemanticOp::AddBranchNode {
                        target: container,
                        tag: tag_name.clone(),
                        marker_span: span,
                        body: segment,
                        kind: BranchKind::Segment,
                    });
                } else {
                    // Root block
                    self.semantic_ops
                        .push(BlockSemanticOp::AddRoot { id: container });
                    self.semantic_ops.push(BlockSemanticOp::AddBranchNode {
                        target: container,
                        tag: tag_name.clone(),
                        marker_span: span,
                        body: segment,
                        kind: BranchKind::Segment,
                    });
                }

                self.stack.push(TreeFrame {
                    opener_name: tag_name.clone(),
                    opener_bits: bits.clone(),
                    opener_span: span,
                    container_body: container,
                    segment_body: segment,
                    parent_body: parent,
                });
            }
            TagClass::Closer { opener_name } => {
                self.close_block(&opener_name, bits, span);
            }
            TagClass::Intermediate { possible_openers } => {
                self.add_intermediate(tag_name, &possible_openers, span);
            }
            TagClass::Unknown => {
                if let Some(segment) = get_active_segment(&self.stack) {
                    self.semantic_ops.push(BlockSemanticOp::AddLeafNode {
                        target: segment,
                        label: tag_name.clone(),
                        span,
                    });
                }
            }
        }
    }

    fn close_block(&mut self, opener_name: &str, closer_bits: &[String], span: Span) {
        if let Some(frame_idx) = find_frame_from_opener(&self.stack, opener_name) {
            // Pop any unclosed blocks above this one
            while self.stack.len() > frame_idx + 1 {
                if let Some(unclosed) = self.stack.pop() {
                    if let Some(parent) = unclosed.parent_body {
                        self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                            target: parent,
                            message: format!("Unclosed block '{}'", unclosed.opener_name),
                            span: unclosed.opener_span,
                        });
                    }
                    // If no parent, this was a root block that wasn't closed - we could track this separately
                }
            }

            // validate and close
            let frame = self.stack.pop().unwrap();
            match self
                .index
                .validate_close(opener_name, &frame.opener_bits, closer_bits, self.db)
            {
                CloseValidation::Valid => {
                    // Finalize the last segment body to end just before the closer marker
                    let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                    self.semantic_ops.push(BlockSemanticOp::FinalizeSpanTo {
                        id: frame.segment_body,
                        end: content_end,
                    });
                    // Extend to include closer
                    self.semantic_ops.push(BlockSemanticOp::ExtendBlockSpan {
                        id: frame.container_body,
                        span,
                    });
                }
                CloseValidation::ArgumentMismatch { arg, expected, got } => {
                    self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                        target: frame.segment_body,
                        message: format!(
                            "Argument '{arg}' mismatch: expected '{expected}', got '{got}'"
                        ),
                        span,
                    });
                    self.stack.push(frame); // Restore frame
                }
                CloseValidation::MissingRequiredArg { arg, expected } => {
                    self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                        target: frame.segment_body,
                        message: format!(
                            "Missing required argument '{arg}': expected '{expected}'"
                        ),
                        span,
                    });
                    self.stack.push(frame);
                }
                CloseValidation::UnexpectedArg { arg, got } => {
                    self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                        target: frame.segment_body,
                        message: format!("Unexpected argument '{arg}' with value '{got}'"),
                        span,
                    });
                    self.stack.push(frame);
                }
                CloseValidation::NotABlock => {
                    // Should not happen as we already classified it
                    if let Some(segment) = get_active_segment(&self.stack) {
                        self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                            target: segment,
                            message: format!("Internal error: {opener_name} is not a block"),
                            span,
                        });
                    }
                }
            }
        } else if let Some(segment) = get_active_segment(&self.stack) {
            self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                target: segment,
                message: format!("Unexpected closing tag '{opener_name}'"),
                span,
            });
        }
    }

    fn add_intermediate(&mut self, tag_name: &str, possible_openers: &[String], span: Span) {
        if let Some(frame) = self.stack.last() {
            if possible_openers.contains(&frame.opener_name) {
                // Finalize previous segment body to just before this marker (full start)
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                let segment_to_finalize = frame.segment_body;
                let container = frame.container_body;

                self.semantic_ops.push(BlockSemanticOp::FinalizeSpanTo {
                    id: segment_to_finalize,
                    end: content_end,
                });

                let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
                let new_segment_id = self.alloc_block_id(Span::new(body_start, 0), Some(container));

                // Add the branch node for the new segment
                self.semantic_ops.push(BlockSemanticOp::AddBranchNode {
                    target: container,
                    tag: tag_name.to_string(),
                    marker_span: span,
                    body: new_segment_id,
                    kind: BranchKind::Segment,
                });

                self.stack.last_mut().unwrap().segment_body = new_segment_id;
            } else {
                let segment = frame.segment_body;
                let opener_name = frame.opener_name.clone();

                self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                    target: segment,
                    message: format!("'{tag_name}' is not valid in '{opener_name}'"),
                    span,
                });
            }
        } else {
            // Intermediate tag at top level - this is an error
            // Could track this in a separate error list
        }
    }

    fn finish(&mut self) {
        while let Some(frame) = self.stack.pop() {
            if self.index.is_end_optional(&frame.opener_name) {
                // No explicit closer: finalize last segment to end of input (best-effort)
                // We do not know the real end; leave as-is and extend container by opener span only.
                self.semantic_ops.push(BlockSemanticOp::ExtendBlockSpan {
                    id: frame.container_body,
                    span: frame.opener_span,
                });
            } else if let Some(parent) = frame.parent_body {
                self.semantic_ops.push(BlockSemanticOp::AddErrorNode {
                    target: parent,
                    message: format!("Unclosed block '{}'", frame.opener_name),
                    span: frame.opener_span,
                });
            }
        }
    }
}

type TreeStack = Vec<TreeFrame>;

/// Get the currently active segment (the innermost block we're in)
fn get_active_segment(stack: &TreeStack) -> Option<BlockId> {
    stack.last().map(|frame| frame.segment_body)
}

/// Find a frame in the stack by name
fn find_frame_from_opener(stack: &TreeStack, opener_name: &str) -> Option<usize> {
    stack.iter().rposition(|f| f.opener_name == opener_name)
}

struct TreeFrame {
    opener_name: String,
    opener_bits: Vec<String>,
    opener_span: Span,
    container_body: BlockId,
    segment_body: BlockId,
    parent_body: Option<BlockId>, // Can be None for root blocks
}

impl<'db> SemanticModel<'db> for BlockTreeBuilder<'db> {
    type Model = BlockTree;

    fn observe(&mut self, node: Node) {
        match node {
            Node::Tag { name, bits, span } => {
                self.handle_tag(&name, &bits, span);
            }
            Node::Comment { span, .. } => {
                if let Some(parent) = get_active_segment(&self.stack) {
                    self.semantic_ops.push(BlockSemanticOp::AddLeafNode {
                        target: parent,
                        label: "<comment>".into(),
                        span,
                    });
                }
            }
            Node::Variable { span, .. } => {
                if let Some(parent) = get_active_segment(&self.stack) {
                    self.semantic_ops.push(BlockSemanticOp::AddLeafNode {
                        target: parent,
                        label: "<var>".into(),
                        span,
                    });
                }
            }
            Node::Error {
                full_span, error, ..
            } => {
                if let Some(parent) = get_active_segment(&self.stack) {
                    self.semantic_ops.push(BlockSemanticOp::AddLeafNode {
                        target: parent,
                        label: error.to_string(),
                        span: full_span,
                    });
                }
            }
            Node::Text { .. } => {} // Skip text nodes - we only care about Django constructs
        }
    }

    fn construct(mut self) -> Self::Model {
        self.finish();
        self.apply_operations()
    }
}
