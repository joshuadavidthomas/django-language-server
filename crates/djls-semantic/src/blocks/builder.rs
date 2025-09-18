use djls_source::Span;
use djls_templates::{
    nodelist::{TagBit, TagName},
    tokens::TagDelimiter,
    Node, NodeList,
};

use crate::Db;

use super::{
    nodes::{BlockId, BlockNode, BranchKind},
    shapes::{CloseValidation, TagClass, TagShape, TagShapes},
    traits::SemanticModel,
    tree::BlockTree,
};

/// Semantic operations that represent the block structure of a Django template.
/// These are the semantic facts we discover while analyzing the template.
#[derive(Debug, Clone)]
enum BlockSemantics {
    /// Allocate a new block/region with optional parent
    AllocBlock {
        id: BlockId,
        span: Span,
        parent: Option<BlockId>,
    },
    /// Add a branch node (opener or segment) to a block
    AddBranchNode {
        target: BlockId, // Block to add the node to
        tag: String,
        marker_span: Span,
        body: BlockId, // The block this branch points to
        kind: BranchKind,
    },
    /// Add an error node to a block
    AddErrorNode {
        target: BlockId, // Block to add the node to
        message: String,
        span: Span,
    },
    /// Add a leaf node to a block
    AddLeafNode {
        target: BlockId, // Block to add the node to
        label: String,
        span: Span,
    },
    /// Add a block as a root
    AddRoot { id: BlockId },
    /// Extend a block's span to include additional content
    ExtendSpan { id: BlockId, span: Span },
    /// Set a block's span to specific bounds
    SetSpan { id: BlockId, start: u32, end: u32 },
}

/// Semantic model builder for Django template block structure.
/// Builds a BlockTree that represents the hierarchical block structure
/// and control flow of Django templates.
pub struct BlockModelBuilder<'db> {
    db: &'db dyn Db,
    shapes: &'db TagShapes,
    stack: Vec<TreeFrame<'db>>,
    semantic_ops: Vec<BlockSemantics>,
    next_id: u32,
}

impl<'db> BlockModelBuilder<'db> {
    pub fn new(db: &'db dyn Db, shapes: &'db TagShapes) -> Self {
        Self {
            db,
            shapes,
            stack: Vec::new(),
            semantic_ops: Vec::new(),
            next_id: 0,
        }
    }

    /// Allocate a new BlockId without creating the block yet
    fn alloc_block_id(&mut self) -> BlockId {
        let id = BlockId::new(self.next_id);
        self.next_id += 1;
        id
    }

    /// Record a semantic operation to be applied later
    fn record(&mut self, op: BlockSemantics) {
        self.semantic_ops.push(op);
    }

    /// Get the currently active segment (the innermost block we're in)
    fn active_segment(&self) -> Option<BlockId> {
        self.stack.last().map(|frame| frame.segment_body)
    }

    /// Find a frame in the stack by opener name
    fn find_frame(&self, opener_name: &str) -> Option<usize> {
        self.stack
            .iter()
            .rposition(|f| f.opener_name == opener_name)
    }

    /// Apply all semantic operations to build a BlockTree
    fn apply_operations(self) -> BlockTree {
        let mut tree = BlockTree::new();

        // Optimize with pre-sizing

        // Count operations by type for pre-allocation
        let mut block_count = 0;
        let mut root_count = 0;
        for op in &self.semantic_ops {
            match op {
                BlockSemantics::AllocBlock { .. } => block_count += 1,
                BlockSemantics::AddRoot { .. } => root_count += 1,
                _ => {}
            }
        }

        // Pre-allocate capacity (this would require adding methods to BlockTree)
        // tree.blocks_mut().reserve(block_count);
        // tree.roots_mut().reserve_exact(root_count);

        // First pass: Collect and sort AllocBlock semantics
        let mut alloc_ops = Vec::with_capacity(block_count);
        for op in &self.semantic_ops {
            if let BlockSemantics::AllocBlock { id, span, parent } = op {
                alloc_ops.push((*id, *span, *parent));
            }
        }
        alloc_ops.sort_unstable_by_key(|(id, _, _)| id.id());

        // Allocate all blocks in order
        for (expected_id, span, parent) in alloc_ops {
            let actual_id = if let Some(p) = parent {
                tree.blocks_mut().alloc_with_parent(span, Some(p))
            } else {
                tree.blocks_mut().alloc(span)
            };
            // In release mode, we can skip this check for performance
            debug_assert_eq!(expected_id.id(), actual_id.id(), "BlockId mismatch");
        }

        // Second pass: Apply all other semantics
        for op in self.semantic_ops {
            match op {
                BlockSemantics::AllocBlock { .. } => {
                    // Already handled in first pass
                }
                BlockSemantics::AddRoot { id } => {
                    tree.roots_mut().push(id);
                }
                BlockSemantics::AddBranchNode {
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
                BlockSemantics::AddLeafNode {
                    target,
                    label,
                    span,
                } => {
                    tree.blocks_mut().add_leaf(target, label, span);
                }
                BlockSemantics::AddErrorNode {
                    target,
                    message,
                    span,
                } => {
                    tree.blocks_mut().add_error(target, message, span);
                }
                BlockSemantics::ExtendSpan { id, span } => {
                    tree.blocks_mut().extend(id, span);
                }
                BlockSemantics::SetSpan { id, start: _, end } => {
                    tree.blocks_mut().finalize_body_to(id, end);
                }
            }
        }

        tree
    }

    fn handle_tag(&mut self, name: TagName<'db>, bits: Vec<TagBit<'db>>, span: Span) {
        let tag_name = name.text(self.db);
        match self.shapes.classify(&tag_name) {
            TagClass::Opener { .. } => {
                let parent = self.active_segment();

                // Phase 3: Pre-allocate BlockIds
                let container = self.alloc_block_id();
                let segment = self.alloc_block_id();

                if let Some(parent_id) = parent {
                    // Nested block - decompose add_block into primitive operations
                    self.record(BlockSemantics::AllocBlock {
                        id: container,
                        span,
                        parent: Some(parent_id),
                    });
                    self.record(BlockSemantics::AddBranchNode {
                        target: parent_id,
                        tag: tag_name.clone(),
                        marker_span: span,
                        body: container,
                        kind: BranchKind::Opener,
                    });
                    self.record(BlockSemantics::AllocBlock {
                        id: segment,
                        span: Span::new(span.end().saturating_add(TagDelimiter::LENGTH_U32), 0),
                        parent: Some(container),
                    });
                    self.record(BlockSemantics::AddBranchNode {
                        target: container,
                        tag: tag_name.clone(),
                        marker_span: span,
                        body: segment,
                        kind: BranchKind::Segment,
                    });
                } else {
                    // Root block
                    self.record(BlockSemantics::AllocBlock {
                        id: container,
                        span,
                        parent: None,
                    });
                    self.record(BlockSemantics::AddRoot { id: container });
                    self.record(BlockSemantics::AllocBlock {
                        id: segment,
                        span: Span::new(span.end().saturating_add(TagDelimiter::LENGTH_U32), 0),
                        parent: Some(container),
                    });
                    self.record(BlockSemantics::AddBranchNode {
                        target: container,
                        tag: tag_name.clone(),
                        marker_span: span,
                        body: segment,
                        kind: BranchKind::Segment,
                    });
                }

                // Phase 3: Use our pre-allocated IDs in the stack frame
                self.stack.push(TreeFrame {
                    opener_name: tag_name,
                    opener_bits: bits,
                    opener_span: span,
                    container_body: container, // Use our pre-allocated ID
                    segment_body: segment,     // Use our pre-allocated ID
                    parent_body: parent,
                });
            }
            TagClass::Closer { opener_name } => {
                self.close_block(&opener_name, &bits, span);
            }
            TagClass::Intermediate { possible_openers } => {
                self.add_intermediate(&tag_name, &possible_openers, span);
            }
            TagClass::Unknown => {
                if let Some(segment) = self.active_segment() {
                    self.record(BlockSemantics::AddLeafNode {
                        target: segment,
                        label: tag_name,
                        span,
                    });
                }
            }
        }
    }

    fn close_block(&mut self, opener_name: &str, closer_bits: &[TagBit<'db>], span: Span) {
        // Find the matching frame
        if let Some(frame_idx) = self.find_frame(opener_name) {
            // Pop any unclosed blocks above this one
            while self.stack.len() > frame_idx + 1 {
                if let Some(unclosed) = self.stack.pop() {
                    if let Some(parent) = unclosed.parent_body {
                        self.record(BlockSemantics::AddErrorNode {
                            target: parent,
                            message: format!("Unclosed block '{}'", unclosed.opener_name),
                            span: unclosed.opener_span,
                        });
                    }
                    // If no parent, this was a root block that wasn't closed - we could track this separately
                }
            }

            // Now validate and close
            let frame = self.stack.pop().unwrap();
            match self
                .shapes
                .validate_close(opener_name, &frame.opener_bits, closer_bits, self.db)
            {
                CloseValidation::Valid => {
                    // Finalize the last segment body to end just before the closer marker
                    let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                    self.record(BlockSemantics::SetSpan {
                        id: frame.segment_body,
                        start: 0, // This will be ignored, only end is used in finalize_body_to
                        end: content_end,
                    });
                    // Extend container to include the closer
                    self.record(BlockSemantics::ExtendSpan {
                        id: frame.container_body,
                        span,
                    });
                }
                CloseValidation::ArgumentMismatch { arg, expected, got } => {
                    self.record(BlockSemantics::AddErrorNode {
                        target: frame.segment_body,
                        message: format!(
                            "Argument '{arg}' mismatch: expected '{expected}', got '{got}'"
                        ),
                        span,
                    });
                    self.stack.push(frame); // Restore frame
                }
                CloseValidation::MissingRequiredArg { arg, expected } => {
                    self.record(BlockSemantics::AddErrorNode {
                        target: frame.segment_body,
                        message: format!(
                            "Missing required argument '{arg}': expected '{expected}'"
                        ),
                        span,
                    });
                    self.stack.push(frame);
                }
                CloseValidation::UnexpectedArg { arg, got } => {
                    self.record(BlockSemantics::AddErrorNode {
                        target: frame.segment_body,
                        message: format!("Unexpected argument '{arg}' with value '{got}'"),
                        span,
                    });
                    self.stack.push(frame);
                }
                CloseValidation::NotABlock => {
                    // Should not happen as we already classified it
                    if let Some(segment) = self.active_segment() {
                        self.record(BlockSemantics::AddErrorNode {
                            target: segment,
                            message: format!("Internal error: {opener_name} is not a block"),
                            span,
                        });
                    }
                }
            }
        } else {
            if let Some(segment) = self.active_segment() {
                self.record(BlockSemantics::AddErrorNode {
                    target: segment,
                    message: format!("Unexpected closing tag '{opener_name}'"),
                    span,
                });
            }
            // Top-level closing tags without openers could be tracked separately
        }
    }

    fn add_intermediate(&mut self, tag_name: &str, possible_openers: &[String], span: Span) {
        if let Some(frame) = self.stack.last() {
            if possible_openers.contains(&frame.opener_name) {
                // Finalize previous segment body to just before this marker (full start)
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                let segment_to_finalize = frame.segment_body;
                let container = frame.container_body;

                self.record(BlockSemantics::SetSpan {
                    id: segment_to_finalize,
                    start: 0, // Ignored
                    end: content_end,
                });

                // Phase 3: Pre-allocate ID for new segment
                let new_segment_id = self.alloc_block_id();

                // Decompose add_segment into primitive operations
                let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
                self.record(BlockSemantics::AllocBlock {
                    id: new_segment_id,
                    span: Span::new(body_start, 0),
                    parent: Some(container),
                });
                self.record(BlockSemantics::AddBranchNode {
                    target: container,
                    tag: tag_name.to_string(),
                    marker_span: span,
                    body: new_segment_id,
                    kind: BranchKind::Segment,
                });

                // Phase 3: Update the frame with our pre-allocated ID
                self.stack.last_mut().unwrap().segment_body = new_segment_id;
            } else {
                let segment = frame.segment_body;
                let opener_name = frame.opener_name.clone();

                self.record(BlockSemantics::AddErrorNode {
                    target: segment,
                    message: format!("'{}' is not valid in '{}'", tag_name, opener_name),
                    span,
                });
            }
        } else {
            // Intermediate tag at top level - this is an error but we have nowhere to put it
            // Could track this in a separate error list
        }
    }

    fn finish(&mut self) {
        // Close any remaining open blocks
        while let Some(frame) = self.stack.pop() {
            // Check if this block's end tag was optional
            if let Some(TagShape::Block { end, .. }) = self.shapes.get(&frame.opener_name) {
                if end.optional {
                    // No explicit closer: finalize last segment to end of input (best-effort)
                    // We do not know the real end; leave as-is and extend container by opener span only.
                    self.record(BlockSemantics::ExtendSpan {
                        id: frame.container_body,
                        span: frame.opener_span,
                    });
                } else {
                    if let Some(parent) = frame.parent_body {
                        self.record(BlockSemantics::AddErrorNode {
                            target: parent,
                            message: format!("Unclosed block '{}'", frame.opener_name),
                            span: frame.opener_span,
                        });
                    }
                    // Unclosed root blocks could be tracked separately
                }
            }
        }
    }
}

struct TreeFrame<'db> {
    opener_name: String,
    opener_bits: Vec<TagBit<'db>>,
    opener_span: Span,
    container_body: BlockId,
    segment_body: BlockId,
    parent_body: Option<BlockId>, // Can be None for root blocks
}

// Implement the SemanticModel trait for BlockModelBuilder
impl<'db> SemanticModel<'db> for BlockModelBuilder<'db> {
    type Model = BlockTree;

    fn observe(&mut self, node: Node<'db>) {
        match node {
            Node::Tag { name, bits, span } => {
                self.handle_tag(name, bits, span);
            }
            Node::Comment { span, .. } => {
                if let Some(parent) = self.active_segment() {
                    self.record(BlockSemantics::AddLeafNode {
                        target: parent,
                        label: "<comment>".into(),
                        span,
                    });
                }
            }
            Node::Variable { span, .. } => {
                if let Some(parent) = self.active_segment() {
                    self.record(BlockSemantics::AddLeafNode {
                        target: parent,
                        label: "<var>".into(),
                        span,
                    });
                }
            }
            Node::Text { .. } => {
                // Skip text nodes - we only care about Django constructs
            }
            Node::Error {
                full_span, error, ..
            } => {
                if let Some(parent) = self.active_segment() {
                    self.record(BlockSemantics::AddLeafNode {
                        target: parent,
                        label: error.to_string(),
                        span: full_span,
                    });
                }
            }
        }
    }

    fn construct(mut self) -> Self::Model {
        self.finish();
        self.apply_operations()
    }
}
