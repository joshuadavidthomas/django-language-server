use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;

use super::grammar::CloseValidation;
use super::grammar::TagClass;
use super::grammar::TagIndex;
use super::tree::BlockId;
use super::tree::BlockNode;
use super::tree::BlockTreeInner;
use super::tree::Blocks;
use super::tree::BranchKind;
use crate::traits::SemanticModel;
use crate::ValidationError;

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
}

pub struct BlockTreeBuilder {
    index: TagIndex,
    stack: Vec<TreeFrame>,
    block_allocs: Vec<(Span, Option<BlockId>)>,
    ops: Vec<TreeOp>,
    errors: Vec<ValidationError>,
}

impl BlockTreeBuilder {
    pub fn new(index: TagIndex) -> Self {
        Self {
            index,
            stack: Vec::new(),
            block_allocs: Vec::new(),
            ops: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Allocate a new `BlockId` and track its metadata for later creation
    fn alloc_block_id(&mut self, span: Span, parent: Option<BlockId>) -> BlockId {
        let id = BlockId::new(u32::try_from(self.block_allocs.len()).unwrap_or_default());
        self.block_allocs.push((span, parent));
        id
    }

    /// Apply all semantic operations to build a BlockTreeInner
    fn apply_operations(self) -> (BlockTreeInner, Vec<ValidationError>) {
        let BlockTreeBuilder {
            block_allocs,
            ops,
            errors,
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
            }
        }

        (BlockTreeInner { roots, blocks }, errors)
    }

    fn handle_tag(&mut self, name: &str, bits: &[String], span: Span) {
        let full_span = expand_marker(span);
        match self.index.classify(name) {
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
                    self.ops.push(TreeOp::AddBranchNode {
                        target: container,
                        tag: name.to_string(),
                        marker_span: span,
                        body: segment,
                        kind: BranchKind::Segment,
                    });
                    self.ops.push(TreeOp::AddRoot { id: container });
                }

                // Initialize the segment
                // By allocating the container, we've essentially placed our tag for the node container.
                // Placing the marker span at the true tag's span instead of the full span
                // allows us to perform incremental edits without re-traversing the entire tag's contents.
                self.ops.push(TreeOp::ExtendBlockSpan { id: segment, span });

                self.stack.push(TreeFrame {
                    opener_name: name.to_string(),
                    opener_bits: bits.to_vec(),
                    opener_span: span,
                    container_body: container,
                    segment_body: segment,
                });
            }
            TagClass::Intermediate { possible_openers } => {
                // Find the outermost matching opener
                let maybe_frame_idx = possible_openers
                    .iter()
                    .find_map(|opener| find_frame_from_opener(&self.stack, opener));

                if let Some(frame_idx) = maybe_frame_idx {
                    // Pop intermediates off the stack
                    let intermediates: Vec<_> = self.stack.drain((frame_idx + 1)..).collect();
                    
                    // Finalize any currently active segments
                    for frame in intermediates {
                        self.finalize_segments(&frame);
                    }
                    
                    // Get parent container and segment bodies before mutable borrow
                    let parent_container_body = self.stack.last().expect("parent frame exists").container_body;
                    let parent_segment_body = self.stack.last().expect("parent frame exists").segment_body;

                    // Create new segment
                    let new_segment_start =
                        Span::new(span.end().saturating_add(TagDelimiter::LENGTH_U32), 0);
                    let new_segment =
                        self.alloc_block_id(new_segment_start, Some(parent_container_body));

                    // Finalize the parent's current segment
                    self.ops.push(TreeOp::FinalizeSpanTo {
                        id: parent_segment_body,
                        end: span.start(),
                    });

                    // Add the new segment to parent's container
                    self.ops.push(TreeOp::AddBranchNode {
                        target: parent_container_body,
                        tag: name.to_string(),
                        marker_span: full_span,
                        body: new_segment,
                        kind: BranchKind::Segment,
                    });

                    // Update parent with new segment
                    self.stack.last_mut().expect("parent frame exists").segment_body = new_segment;

                    // Initialize the new segment with its full span
                    self.ops.push(TreeOp::ExtendBlockSpan {
                        id: new_segment,
                        span,
                    });

                    // Resize the stack back to the parent
                    self.stack.truncate(frame_idx + 1);
                } else {
                    // Orphaned intermediate - no matching opener found
                    self.errors.push(ValidationError::OrphanedTag {
                        tag: name.to_string(),
                        context: "intermediate tag without matching opener".to_string(),
                        span: full_span,
                    });
                }
            }
            TagClass::Closer { opener_name } => {
                self.handle_closer(&opener_name, name, bits, span);
            }
            TagClass::Unknown => {
                // Unknown tag - treat as a leaf node
                if let Some(parent) = get_active_segment(&self.stack) {
                    self.ops.push(TreeOp::AddLeafNode {
                        target: parent,
                        label: name.to_string(),
                        span: full_span,
                    });
                }
            }
        }
    }

    fn handle_closer(&mut self, opener_name: &str, closer_name: &str, closer_bits: &[String], span: Span) {
        let frame_idx = match find_frame_from_opener(&self.stack, opener_name) {
            Some(idx) => idx,
            None => {
                self.errors.push(ValidationError::UnmatchedClosingTag {
                    tag: closer_name.to_string(),
                    span,
                });
                return;
            }
        };

        // Pop all frames from the found index onwards
        let frames_to_close: Vec<_> = self.stack.drain(frame_idx..).collect();
        let frame = frames_to_close.first().unwrap();

        // Validate the close
        match self
            .index
            .validate_close(opener_name, &frame.opener_bits, closer_bits)
        {
            CloseValidation::Valid => {
                // Finalize the last segment body to end just before the closer marker
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                self.ops.push(TreeOp::FinalizeSpanTo {
                    id: frame.segment_body,
                    end: content_end,
                });
            }
            CloseValidation::ArgumentMismatch { expected, got, .. } => {
                let name = if got.is_empty() { expected } else { got };
                self.errors.push(ValidationError::UnmatchedBlockName {
                    name,
                    span: expand_marker(span),
                });
                self.errors.push(ValidationError::UnclosedTag {
                    tag: frame.opener_name.clone(),
                    span: frame.opener_span,
                });
                self.stack.push(frame.clone());
            }
            CloseValidation::MissingRequiredArg { expected, .. } => {
                let expected_closing = format!("{} {}", frame.opener_name, expected);
                self.errors.push(ValidationError::UnbalancedStructure {
                    opening_tag: frame.opener_name.clone(),
                    expected_closing,
                    opening_span: frame.opener_span,
                    closing_span: Some(expand_marker(span)),
                });
                self.errors.push(ValidationError::UnclosedTag {
                    tag: frame.opener_name.clone(),
                    span: frame.opener_span,
                });
                self.stack.push(frame.clone());
            }
            CloseValidation::UnexpectedArg { arg, got } => {
                let name = if got.is_empty() { arg } else { got };
                self.errors.push(ValidationError::UnmatchedBlockName {
                    name,
                    span: expand_marker(span),
                });
                self.errors.push(ValidationError::UnclosedTag {
                    tag: frame.opener_name.clone(),
                    span: frame.opener_span,
                });
                self.stack.push(frame.clone());
            }
            CloseValidation::NotABlock => {
                self.errors.push(ValidationError::UnbalancedStructure {
                    opening_tag: opener_name.to_string(),
                    expected_closing: closer_name.to_string(),
                    opening_span: frame.opener_span,
                    closing_span: Some(expand_marker(span)),
                });
                self.errors.push(ValidationError::UnclosedTag {
                    tag: frame.opener_name.clone(),
                    span: frame.opener_span,
                });
                self.stack.push(frame.clone());
            }
        }

        // Check for unclosed inner frames
        if frames_to_close.len() > 1 {
            let unclosed: Vec<String> = frames_to_close[1..]
                .iter()
                .map(|f| format!("{{%% {} %%}}", f.opener_name))
                .collect();

            self.errors.push(ValidationError::MismatchedClosingTag {
                expected: format!("{{%% {} %%}}", frame.opener_name),
                found: format!("{{%% {} %%}}", closer_name),
                unclosed,
                span: expand_marker(span),
            });

            // Finalize segments for unclosed frames
            for inner_frame in &frames_to_close[1..] {
                self.finalize_segments(inner_frame);
            }
        }
    }

    /// Close out the remaining frames
    fn finish(&mut self) {
        let stack = std::mem::take(&mut self.stack);
        for frame in &stack {
            // Check if the tag's end is optional
            if self.index.is_end_optional(&frame.opener_name) {
                // Optional end tag - finalize normally
                self.finalize_segments(frame);
            } else {
                // Required end tag - report error
                self.errors.push(ValidationError::UnclosedTag {
                    tag: frame.opener_name.clone(),
                    span: frame.opener_span,
                });
                self.finalize_segments(frame);
            }
        }
    }

    /// Finalize segments for a frame
    fn finalize_segments(&mut self, frame: &TreeFrame) {
        // Get the last offset we have
        let end = self
            .ops
            .iter()
            .rev()
            .find_map(|op| match op {
                TreeOp::FinalizeSpanTo { end, .. } => Some(*end),
                TreeOp::ExtendBlockSpan { span, .. } => Some(span.end()),
                TreeOp::AddLeafNode { span, .. } => Some(span.end()),
                _ => None,
            })
            .unwrap_or(frame.opener_span.end());

        self.ops.push(TreeOp::FinalizeSpanTo {
            id: frame.segment_body,
            end,
        });
    }
}

/// Expand a marker span to include its delimiters
fn expand_marker(span: Span) -> Span {
    span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
}

type TreeStack = Vec<TreeFrame>;

/// Get the currently active segment body from the stack
fn get_active_segment(stack: &TreeStack) -> Option<BlockId> {
    stack.last().map(|frame| frame.segment_body)
}

/// Find a frame in the stack by name
fn find_frame_from_opener(stack: &TreeStack, opener_name: &str) -> Option<usize> {
    stack.iter().rposition(|f| f.opener_name == opener_name)
}

#[derive(Clone)]
struct TreeFrame {
    opener_name: String,
    opener_bits: Vec<String>,
    opener_span: Span,
    container_body: BlockId,
    segment_body: BlockId,
}

impl SemanticModel<'_> for BlockTreeBuilder {
    type Model = (BlockTreeInner, Vec<ValidationError>);

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