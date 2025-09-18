use djls_source::Span;
use djls_templates::{
    nodelist::{TagBit, TagName},
    tokens::TagDelimiter,
    Node, NodeList,
};

use crate::Db;

use super::{
    nodes::BlockId,
    shapes::{CloseValidation, TagClass, TagShape, TagShapes},
    tree::BlockTree,
};

pub struct TreeBuilder<'db> {
    tree: BlockTree,
    ctx: BuildContext<'db>,
}

impl<'db> TreeBuilder<'db> {
    pub fn new(db: &'db dyn Db, shapes: &'db TagShapes) -> Self {
        Self {
            tree: BlockTree::new(),
            ctx: BuildContext::new(db, shapes),
        }
    }

    pub fn build(mut self, db: &'db dyn Db, nodelist: NodeList<'db>) -> BlockTree {
        for node in nodelist.nodelist(db).iter().cloned() {
            self.handle_node(node);
        }
        self.finish();
        self.tree
    }

    fn handle_node(&mut self, node: Node<'db>) {
        match node {
            Node::Tag { name, bits, span } => {
                self.handle_tag(name, bits, span);
            }
            Node::Comment { span, .. } => {
                if let Some(parent) = self.ctx.active_segment() {
                    self.tree
                        .blocks()
                        .add_leaf(parent, "<comment>".into(), span);
                }
                // Skip comments at top level - we only care about block structure
            }
            Node::Variable { span, .. } => {
                if let Some(parent) = self.ctx.active_segment() {
                    self.tree.blocks().add_leaf(parent, "<var>".into(), span);
                }
                // Skip variables at top level - they'd be orphaned anyway
            }
            Node::Text { .. } => {
                // Skip text nodes - we only care about Django constructs
            }
            Node::Error {
                full_span, error, ..
            } => {
                if let Some(parent) = self.ctx.active_segment() {
                    self.tree
                        .blocks()
                        .add_leaf(parent, error.to_string(), full_span);
                } else {
                    // Top-level errors should still be tracked somehow
                    // For now, skip them - might want to add a separate error list later
                }
            }
        }
    }

    fn handle_tag(&mut self, name: TagName<'db>, bits: Vec<TagBit<'db>>, span: Span) {
        let tag_name = name.text(self.ctx.db);
        match self.ctx.shapes.classify(&tag_name) {
            TagClass::Opener { .. } => {
                let parent = self.ctx.active_segment();
                let (container, segment) = if let Some(parent_id) = parent {
                    // Nested block - add to parent
                    self.tree.blocks_mut().add_block(parent_id, &tag_name, span)
                } else {
                    // Root block - create container directly as a root
                    let container = self.tree.blocks_mut().alloc(span);
                    // Create the first segment under this container
                    let segment =
                        self.tree
                            .blocks_mut()
                            .add_segment(container, tag_name.clone(), span);
                    self.tree.roots_mut().push(container);
                    (container, segment)
                };
                self.ctx.stack.push(TreeFrame {
                    opener_name: tag_name,
                    opener_bits: bits,
                    opener_span: span,
                    container_body: container,
                    segment_body: segment,
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
                if let Some(segment) = self.ctx.active_segment() {
                    self.tree.blocks_mut().add_leaf(segment, tag_name, span);
                }
            }
        }
    }

    fn close_block(&mut self, opener_name: &str, closer_bits: &[TagBit<'db>], span: Span) {
        // Find the matching frame
        if let Some(frame_idx) = self.ctx.find_frame(opener_name) {
            // Pop any unclosed blocks above this one
            while self.ctx.stack.len() > frame_idx + 1 {
                if let Some(unclosed) = self.ctx.stack.pop() {
                    if let Some(parent) = unclosed.parent_body {
                        self.tree.blocks_mut().add_error(
                            parent,
                            format!("Unclosed block '{}'", unclosed.opener_name),
                            unclosed.opener_span,
                        );
                    }
                    // If no parent, this was a root block that wasn't closed - we could track this separately
                }
            }

            // Now validate and close
            let frame = self.ctx.stack.pop().unwrap();
            match self.ctx.shapes.validate_close(
                opener_name,
                &frame.opener_bits,
                closer_bits,
                self.ctx.db,
            ) {
                CloseValidation::Valid => {
                    // Finalize the last segment body to end just before the closer marker
                    let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                    self.tree
                        .blocks_mut()
                        .finalize_body_to(frame.segment_body, content_end);
                    // Extend container to include the closer
                    self.tree.blocks_mut().extend(frame.container_body, span);
                }
                CloseValidation::ArgumentMismatch { arg, expected, got } => {
                    self.tree.blocks_mut().add_error(
                        frame.segment_body,
                        format!("Argument '{arg}' mismatch: expected '{expected}', got '{got}'",),
                        span,
                    );
                    self.ctx.stack.push(frame); // Restore frame
                }
                CloseValidation::MissingRequiredArg { arg, expected } => {
                    self.tree.blocks_mut().add_error(
                        frame.segment_body,
                        format!("Missing required argument '{arg}': expected '{expected}'",),
                        span,
                    );
                    self.ctx.stack.push(frame);
                }
                CloseValidation::UnexpectedArg { arg, got } => {
                    self.tree.blocks_mut().add_error(
                        frame.segment_body,
                        format!("Unexpected argument '{arg}' with value '{got}'"),
                        span,
                    );
                    self.ctx.stack.push(frame);
                }
                CloseValidation::NotABlock => {
                    // Should not happen as we already classified it
                    if let Some(segment) = self.ctx.active_segment() {
                        self.tree.blocks_mut().add_error(
                            segment,
                            format!("Internal error: {opener_name} is not a block"),
                            span,
                        );
                    }
                }
            }
        } else {
            if let Some(segment) = self.ctx.active_segment() {
                self.tree.blocks_mut().add_error(
                    segment,
                    format!("Unexpected closing tag '{opener_name}'"),
                    span,
                );
            }
            // Top-level closing tags without openers could be tracked separately
        }
    }

    fn add_intermediate(&mut self, tag_name: &str, possible_openers: &[String], span: Span) {
        if let Some(frame) = self.ctx.stack.last_mut() {
            if possible_openers.contains(&frame.opener_name) {
                // Finalize previous segment body to just before this marker (full start)
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                self.tree
                    .blocks_mut()
                    .finalize_body_to(frame.segment_body, content_end);
                // Start a new segment; its body begins after the new marker
                frame.segment_body = self.tree.blocks_mut().add_segment(
                    frame.container_body,
                    tag_name.to_string(),
                    span,
                );
            } else {
                self.tree.blocks_mut().add_error(
                    frame.segment_body,
                    format!("'{}' is not valid in '{}'", tag_name, frame.opener_name),
                    span,
                );
            }
        } else {
            // Intermediate tag at top level - this is an error but we have nowhere to put it
            // Could track this in a separate error list
        }
    }

    fn finish(&mut self) {
        // Close any remaining open blocks
        while let Some(frame) = self.ctx.stack.pop() {
            // Check if this block's end tag was optional
            if let Some(TagShape::Block { end, .. }) = self.ctx.shapes.get(&frame.opener_name) {
                if end.optional {
                    // No explicit closer: finalize last segment to end of input (best-effort)
                    // We do not know the real end; leave as-is and extend container by opener span only.
                    self.tree
                        .blocks_mut()
                        .extend(frame.container_body, frame.opener_span);
                } else {
                    if let Some(parent) = frame.parent_body {
                        self.tree.blocks_mut().add_error(
                            parent,
                            format!("Unclosed block '{}'", frame.opener_name),
                            frame.opener_span,
                        );
                    }
                    // Unclosed root blocks could be tracked separately
                }
            }
        }
    }
}

struct BuildContext<'db> {
    db: &'db dyn Db,
    shapes: &'db TagShapes,
    stack: Vec<TreeFrame<'db>>,
}

impl<'db> BuildContext<'db> {
    fn new(db: &'db dyn Db, shapes: &'db TagShapes) -> Self {
        Self {
            db,
            shapes,
            stack: Vec::new(),
        }
    }

    fn active_segment(&self) -> Option<BlockId> {
        self.stack.last().map(|frame| frame.segment_body)
    }

    fn find_frame(&self, opener_name: &str) -> Option<usize> {
        self.stack
            .iter()
            .rposition(|f| f.opener_name == opener_name)
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
