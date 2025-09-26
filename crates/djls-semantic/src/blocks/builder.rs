use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use salsa::Accumulator;

use super::grammar::CloseValidation;
use super::grammar::TagClass;
use super::grammar::TagIndex;
use super::tree::BlockId;
use super::tree::BlockNode;
use super::tree::BlockTree;
use super::tree::Blocks;
use super::tree::BranchKind;
use crate::traits::SemanticModel;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

#[derive(Debug, Clone)]
enum TreeOp {
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
    AccumulateDiagnostic(ValidationError),
}

pub struct BlockTreeBuilder<'db> {
    db: &'db dyn Db,
    index: TagIndex<'db>,
    stack: Vec<TreeFrame>,
    block_allocs: Vec<(Span, Option<BlockId>)>,
    ops: Vec<TreeOp>,
}

impl<'db> BlockTreeBuilder<'db> {
    #[allow(dead_code)] // use is gated behind cfg(test) for now
    pub fn new(db: &'db dyn Db, index: TagIndex<'db>) -> Self {
        Self {
            db,
            index,
            stack: Vec::new(),
            block_allocs: Vec::new(),
            ops: Vec::new(),
        }
    }

    /// Allocate a new `BlockId` and track its metadata for later creation
    fn alloc_block_id(&mut self, span: Span, parent: Option<BlockId>) -> BlockId {
        let id = BlockId::new(u32::try_from(self.block_allocs.len()).unwrap_or_default());
        self.block_allocs.push((span, parent));
        id
    }

    /// Apply all semantic operations to build a tracked `BlockTree`
    fn apply_operations(self) -> BlockTree<'db> {
        let BlockTreeBuilder {
            db,
            block_allocs,
            ops,
            ..
        } = self;

        let mut roots = Vec::new();
        let mut blocks = Blocks::default();

        for (span, parent) in block_allocs {
            if let Some(p) = parent {
                blocks.alloc(span, Some(p));
            } else {
                blocks.alloc(span, None);
            }
        }

        for op in ops {
            match op {
                TreeOp::AddRoot { id } => {
                    roots.push(id);
                }
                TreeOp::AddBranchNode {
                    target,
                    tag,
                    marker_span,
                    body,
                    kind,
                } => {
                    blocks.push_node(
                        target,
                        BlockNode::Branch {
                            tag,
                            marker_span,
                            body,
                            kind,
                        },
                    );
                }
                TreeOp::AddLeafNode {
                    target,
                    label,
                    span,
                } => {
                    blocks.push_node(target, BlockNode::Leaf { label, span });
                }
                TreeOp::ExtendBlockSpan { id, span } => {
                    blocks.extend_block(id, span);
                }
                TreeOp::FinalizeSpanTo { id, end } => {
                    blocks.finalize_block_span(id, end);
                }
                TreeOp::AccumulateDiagnostic(error) => {
                    ValidationErrorAccumulator(error).accumulate(db);
                }
            }
        }

        BlockTree::new(db, roots, blocks)
    }

    fn handle_tag(&mut self, name: &str, bits: &[String], span: Span) {
        let full_span = expand_marker(span);
        match self.index.classify(self.db, name) {
            TagClass::Opener => {
                let parent = get_active_segment(&self.stack);

                let container = self.alloc_block_id(span, parent);
                let segment = self.alloc_block_id(
                    Span::new(span.end().saturating_add(TagDelimiter::LENGTH_U32), 0),
                    Some(container),
                );

                if let Some(parent_id) = parent {
                    // Nested block
                    self.ops.push(TreeOp::AddBranchNode {
                        target: parent_id,
                        tag: name.to_string(),
                        marker_span: span,
                        body: container,
                        kind: BranchKind::Opener,
                    });
                    self.ops.push(TreeOp::AddBranchNode {
                        target: container,
                        tag: name.to_string(),
                        marker_span: span,
                        body: segment,
                        kind: BranchKind::Segment,
                    });
                } else {
                    // Root block
                    self.ops.push(TreeOp::AddRoot { id: container });
                    self.ops.push(TreeOp::AddBranchNode {
                        target: container,
                        tag: name.to_string(),
                        marker_span: span,
                        body: segment,
                        kind: BranchKind::Segment,
                    });
                }

                self.stack.push(TreeFrame {
                    opener_name: name.to_string(),
                    opener_bits: bits.to_vec(),
                    opener_span: full_span,
                    container_body: container,
                    segment_body: segment,
                });
            }
            TagClass::Closer { opener_name } => {
                self.close_block(&opener_name, bits, span);
            }
            TagClass::Intermediate { possible_openers } => {
                self.add_intermediate(name, &possible_openers, span);
            }
            TagClass::Unknown => {
                if let Some(segment) = get_active_segment(&self.stack) {
                    self.ops.push(TreeOp::AddLeafNode {
                        target: segment,
                        label: name.to_string(),
                        span,
                    });
                }
            }
        }
    }

    fn close_block(&mut self, opener_name: &str, closer_bits: &[String], span: Span) {
        let marker_span = expand_marker(span);

        let Some(frame_idx) = find_frame_from_opener(&self.stack, opener_name) else {
            self.ops.push(TreeOp::AccumulateDiagnostic(
                ValidationError::UnbalancedStructure {
                    opening_tag: opener_name.to_string(),
                    expected_closing: String::new(),
                    opening_span: marker_span,
                    closing_span: None,
                },
            ));
            return;
        };

        // Pop any unclosed blocks above this one
        while self.stack.len() > frame_idx + 1 {
            if let Some(unclosed) = self.stack.pop() {
                self.ops
                    .push(TreeOp::AccumulateDiagnostic(ValidationError::UnclosedTag {
                        tag: unclosed.opener_name,
                        span: unclosed.opener_span,
                    }));
            }
        }

        // validate and close
        let frame = self.stack.pop().unwrap();
        match self
            .index
            .validate_close(self.db, opener_name, &frame.opener_bits, closer_bits)
        {
            CloseValidation::Valid => {
                // Finalize the last segment body to end just before the closer marker
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                self.ops.push(TreeOp::FinalizeSpanTo {
                    id: frame.segment_body,
                    end: content_end,
                });
                // Extend to include closer
                self.ops.push(TreeOp::ExtendBlockSpan {
                    id: frame.container_body,
                    span,
                });
            }
            CloseValidation::ArgumentMismatch { expected, got, .. } => {
                let name = if got.is_empty() { expected } else { got };
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnmatchedBlockName {
                        name,
                        span: marker_span,
                    },
                ));
                self.ops
                    .push(TreeOp::AccumulateDiagnostic(ValidationError::UnclosedTag {
                        tag: frame.opener_name.clone(),
                        span: frame.opener_span,
                    }));
                self.stack.push(frame); // Restore frame
            }
            CloseValidation::MissingRequiredArg { expected, .. } => {
                let expected_closing = format!("{} {}", frame.opener_name, expected);
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnbalancedStructure {
                        opening_tag: frame.opener_name.clone(),
                        expected_closing,
                        opening_span: frame.opener_span,
                        closing_span: Some(marker_span),
                    },
                ));
                self.stack.push(frame);
            }
            CloseValidation::UnexpectedArg { arg, got } => {
                let name = if got.is_empty() { arg } else { got };
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnmatchedBlockName {
                        name,
                        span: marker_span,
                    },
                ));
                self.ops
                    .push(TreeOp::AccumulateDiagnostic(ValidationError::UnclosedTag {
                        tag: frame.opener_name.clone(),
                        span: frame.opener_span,
                    }));
                self.stack.push(frame);
            }
            CloseValidation::NotABlock => {
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnbalancedStructure {
                        opening_tag: opener_name.to_string(),
                        expected_closing: opener_name.to_string(),
                        opening_span: frame.opener_span,
                        closing_span: Some(marker_span),
                    },
                ));
                self.stack.push(frame);
            }
        }
    }

    fn add_intermediate(&mut self, tag_name: &str, possible_openers: &[String], span: Span) {
        let marker_span = expand_marker(span);

        if let Some(frame) = self.stack.last() {
            if possible_openers.contains(&frame.opener_name) {
                // Finalize previous segment body to just before this marker (full start)
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                let segment_to_finalize = frame.segment_body;
                let container = frame.container_body;

                self.ops.push(TreeOp::FinalizeSpanTo {
                    id: segment_to_finalize,
                    end: content_end,
                });

                let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
                let new_segment_id = self.alloc_block_id(Span::new(body_start, 0), Some(container));

                // Add the branch node for the new segment
                self.ops.push(TreeOp::AddBranchNode {
                    target: container,
                    tag: tag_name.to_string(),
                    marker_span: span,
                    body: new_segment_id,
                    kind: BranchKind::Segment,
                });

                self.stack.last_mut().unwrap().segment_body = new_segment_id;
            } else {
                let context = format_intermediate_context(possible_openers);
                self.ops
                    .push(TreeOp::AccumulateDiagnostic(ValidationError::OrphanedTag {
                        tag: tag_name.to_string(),
                        context,
                        span: marker_span,
                    }));
            }
        } else {
            let context = format_intermediate_context(possible_openers);
            self.ops
                .push(TreeOp::AccumulateDiagnostic(ValidationError::OrphanedTag {
                    tag: tag_name.to_string(),
                    context,
                    span: marker_span,
                }));
        }
    }

    fn finish(&mut self) {
        while let Some(frame) = self.stack.pop() {
            if self.index.is_end_optional(self.db, &frame.opener_name) {
                // No explicit closer: finalize last segment to end of input (best-effort)
                // We do not know the real end; leave as-is and extend container by opener span only.
                self.ops.push(TreeOp::ExtendBlockSpan {
                    id: frame.container_body,
                    span: frame.opener_span,
                });
            } else {
                self.ops
                    .push(TreeOp::AccumulateDiagnostic(ValidationError::UnclosedTag {
                        tag: frame.opener_name,
                        span: frame.opener_span,
                    }));
            }
        }
    }
}

fn expand_marker(span: Span) -> Span {
    span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
}

fn format_intermediate_context(possible_openers: &[String]) -> String {
    match possible_openers.len() {
        0 => "a valid parent block".to_string(),
        1 => format!("'{}' block", possible_openers[0]),
        2 => format!(
            "'{}' or '{}' block",
            possible_openers[0], possible_openers[1]
        ),
        _ => {
            let mut parts = possible_openers
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>();
            let last = parts.pop().unwrap_or_default();
            let prefix = parts.join(", ");
            format!("one of {prefix}, or {last} blocks")
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
}

impl<'db> SemanticModel<'db> for BlockTreeBuilder<'db> {
    type Model = BlockTree<'db>;

    fn observe(&mut self, node: Node) {
        match node {
            Node::Tag { name, bits, span } => {
                self.handle_tag(&name, &bits, span);
            }
            Node::Comment { span, .. } => {
                if let Some(parent) = get_active_segment(&self.stack) {
                    self.ops.push(TreeOp::AddLeafNode {
                        target: parent,
                        label: "<comment>".into(),
                        span,
                    });
                }
            }
            Node::Variable { span, .. } => {
                if let Some(parent) = get_active_segment(&self.stack) {
                    self.ops.push(TreeOp::AddLeafNode {
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
                    self.ops.push(TreeOp::AddLeafNode {
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
