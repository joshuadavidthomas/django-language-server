use djls_source::Span;
use djls_templates::Node;
use djls_templates::NodeList;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;

use crate::db::Db;
use crate::errors::ValidationError;
use crate::structure::active::CapturedClosingTag;
use crate::structure::grammar::CloseValidation;
use crate::structure::grammar::OpeningContract;
use crate::structure::grammar::SparseTagGrammar;
use crate::structure::grammar::TagClassification;
use crate::structure::tree::BlockRole;
use crate::structure::tree::RegionId;
use crate::structure::tree::Regions;
use crate::structure::tree::TemplateNode;
use crate::structure::tree::TemplateTree;
use crate::tags::BodyAnalysis;

pub(crate) struct TemplateTreeData {
    pub(crate) root: RegionId,
    pub(crate) regions: Regions,
    pub(crate) captured_closers: Vec<CapturedClosingTag>,
    pub(crate) diagnostics: Vec<ValidationError>,
}

impl TemplateTreeData {
    pub(crate) fn into_tree(self, db: &dyn Db) -> TemplateTree<'_> {
        TemplateTree::new(db, self.root, self.regions)
    }
}

/// Frames borrow the opener's name and bits from the `NodeList` (`'n`) and its contract from
/// the grammar, so pushing a frame copies nothing.
pub(crate) struct TemplateTreeBuilder<'grammar, 'n> {
    grammar: &'grammar SparseTagGrammar,
    root: RegionId,
    stack: OpenFrames<'grammar, 'n>,
    regions: Regions,
    captured_closers: Vec<CapturedClosingTag>,
    diagnostics: Vec<ValidationError>,
}

impl<'grammar, 'n> TemplateTreeBuilder<'grammar, 'n> {
    pub(crate) fn new(grammar: &'grammar SparseTagGrammar) -> Self {
        let mut regions = Regions::default();
        let root = regions.alloc(Span::new(0, 0), None);
        Self {
            grammar,
            root,
            stack: OpenFrames::default(),
            regions,
            captured_closers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn model_data(mut self, db: &'n dyn Db, nodelist: NodeList<'n>) -> TemplateTreeData {
        for node in nodelist.nodelist(db) {
            self.handle_node(node);
        }
        self.finish();
        TemplateTreeData {
            root: self.root,
            regions: self.regions,
            captured_closers: self.captured_closers,
            diagnostics: self.diagnostics,
        }
    }

    fn handle_node(&mut self, node: &'n Node) {
        // Tags still need dispatch so they can close the opaque region.
        if self.in_opaque_content() && !matches!(node, Node::Tag { .. }) {
            return;
        }
        let node = match node {
            Node::Tag {
                name,
                name_span,
                bits,
                span,
            } => {
                self.handle_tag(name, *name_span, bits, *span);
                return;
            }
            Node::Comment { span, .. } => TemplateNode::Comment { span: *span },
            Node::Text { span } => TemplateNode::Text { span: *span },
            Node::Variable {
                var,
                var_span,
                filters,
                span,
            } => TemplateNode::Variable {
                var: var.to_string(),
                var_span: *var_span,
                filters: filters.clone(),
                span: *span,
            },
            Node::Error {
                span, full_span, ..
            } => TemplateNode::Error {
                span: *span,
                full_span: *full_span,
            },
        };
        self.regions.push_node(self.active_region(), node);
    }

    fn alloc_region(&mut self, span: Span, parent: RegionId) -> RegionId {
        self.regions.alloc(span, Some(parent))
    }

    fn alloc_block_regions(
        &mut self,
        container_span: Span,
        segment_span: Span,
        parent: RegionId,
    ) -> (RegionId, RegionId) {
        let container_id = self.alloc_region(container_span, parent);
        let segment_id = self.alloc_region(segment_span, container_id);
        (container_id, segment_id)
    }

    fn active_region(&self) -> RegionId {
        self.stack.last().map_or(self.root, |frame| match frame {
            TreeFrame::Block(frame) => frame.segment_body,
            TreeFrame::Opaque(frame) => frame.parent_region,
        })
    }

    fn in_opaque_content(&self) -> bool {
        matches!(self.stack.last(), Some(TreeFrame::Opaque(_)))
    }

    fn handle_tag(&mut self, name: &'n str, name_span: Span, bits: &'n [TagBit], span: Span) {
        let full_span = span.expand_template_tag_marker();
        if self.close_active_opaque_if_closer(name, name_span, bits, span, full_span) {
            return;
        }
        if self.in_opaque_content() {
            return;
        }

        let grammar = self.grammar.for_name_span(name_span);
        if let Some(frame_idx) = self.stack.innermost_closed_by(name) {
            self.close_block_at(name, name_span, frame_idx, bits, span);
            return;
        }

        if matches!(self.stack.last(), Some(frame) if frame.accepts_intermediate(name)) {
            self.add_intermediate(name, name_span, bits, span);
            return;
        }

        match grammar.map(|fact| &fact.classification) {
            Some(TagClassification::Opener(contract)) => {
                let parent = self.active_region();

                if contract.body_analysis.is_opaque() {
                    self.stack.push(TreeFrame::Opaque(OpaqueFrame {
                        opener_name: name,
                        contract,
                        name_span,
                        bits,
                        opener_span: full_span,
                        parent_region: parent,
                        body_start: span.end().saturating_add(TagDelimiter::LENGTH_U32),
                    }));
                    return;
                }

                let segment_span =
                    Span::new(span.end().saturating_add(TagDelimiter::LENGTH_U32), 0);
                let (container, segment) = self.alloc_block_regions(span, segment_span, parent);

                self.regions.push_node(
                    parent,
                    TemplateNode::Block {
                        tag: name.to_string(),
                        name_span,
                        bits: bits.to_vec(),
                        full_span,
                        body: container,
                        role: BlockRole::Opener,
                    },
                );
                self.regions.push_node(
                    container,
                    TemplateNode::Block {
                        tag: name.to_string(),
                        name_span,
                        bits: bits.to_vec(),
                        full_span,
                        body: segment,
                        role: BlockRole::Segment,
                    },
                );

                self.stack.push(TreeFrame::Block(BlockFrame {
                    opener_name: name,
                    contract,
                    opener_bits: bits,
                    opener_span: full_span,
                    container_body: container,
                    parent_region: parent,
                    segment_body: segment,
                }));
            }
            Some(TagClassification::Closer { possible_openers }) => {
                self.accumulate_orphaned_closer(name, possible_openers, full_span);
            }
            Some(TagClassification::Intermediate { possible_openers }) => {
                self.accumulate_orphaned_intermediate(name, possible_openers, full_span);
            }
            Some(
                TagClassification::Standalone
                | TagClassification::Inconclusive
                | TagClassification::Unknown,
            )
            | None => {
                self.add_standalone_tag(name, name_span, bits, full_span);
            }
        }
    }

    fn close_active_opaque_if_closer(
        &mut self,
        closer_name: &str,
        name_span: Span,
        closer_bits: &[TagBit],
        span: Span,
        full_span: Span,
    ) -> bool {
        let frame = match self.stack.last() {
            Some(TreeFrame::Opaque(frame)) if frame.contract.closer == closer_name => *frame,
            // Returning false promises the caller that this tag did not change frame state.
            _ => return false,
        };
        if frame.contract.body_analysis == BodyAnalysis::Verbatim
            && !OpeningContract::verbatim_closes(
                frame.opener_name,
                frame.name_span,
                frame.bits,
                closer_name,
                name_span,
                closer_bits,
            )
        {
            // Django's Lexer keeps any other spelling of the closer as body text.
            return true;
        }
        self.stack.pop();
        self.capture_closer(closer_name, name_span, closer_bits, full_span);
        match frame.contract.validate_close(frame.bits, closer_bits) {
            CloseValidation::Valid => {
                self.finalize_frame(TreeFrame::Opaque(frame), span, full_span);
            }
            CloseValidation::ArgumentMismatch {
                expected,
                got,
                got_span,
            } => {
                self.diagnostics.push(ValidationError::UnmatchedBlockName {
                    expected,
                    got,
                    got_span,
                    span: full_span,
                    opener_span: frame.opener_span,
                });
                self.finalize_frame(TreeFrame::Opaque(frame), span, full_span);
            }
        }

        true
    }

    fn add_standalone_tag(
        &mut self,
        tag_name: &str,
        name_span: Span,
        bits: &[TagBit],
        full_span: Span,
    ) {
        self.regions.push_node(
            self.active_region(),
            TemplateNode::StandaloneTag {
                tag: tag_name.to_string(),
                name_span,
                bits: bits.to_vec(),
                full_span,
            },
        );
    }

    fn accumulate_orphaned_closer(
        &mut self,
        closer_name: &str,
        possible_openers: &[String],
        span: Span,
    ) {
        self.diagnostics.push(ValidationError::OrphanedClosingTag {
            tag: closer_name.to_string(),
            expected_opener: possible_openers
                .first()
                .cloned()
                .unwrap_or_else(|| "matching opener".to_string()),
            span,
        });
    }

    fn close_block_at(
        &mut self,
        closer_name: &str,
        name_span: Span,
        frame_idx: usize,
        closer_bits: &[TagBit],
        span: Span,
    ) {
        let full_span = span.expand_template_tag_marker();
        while self.stack.len() > frame_idx + 1 {
            if let Some(unclosed) = self.stack.pop() {
                self.accumulate_unclosed(&unclosed);
            }
        }

        let Some(frame) = self.stack.pop() else {
            return;
        };
        self.capture_closer(closer_name, name_span, closer_bits, full_span);
        match frame
            .contract()
            .validate_close(frame.opener_bits(), closer_bits)
        {
            CloseValidation::Valid => {
                self.finalize_frame(frame, span, full_span);
            }
            CloseValidation::ArgumentMismatch {
                expected,
                got,
                got_span,
            } => {
                self.diagnostics.push(ValidationError::UnmatchedBlockName {
                    expected,
                    got,
                    got_span,
                    span: full_span,
                    opener_span: frame.opener_span(),
                });
                self.finalize_frame(frame, span, full_span);
            }
        }
    }

    fn capture_closer(&mut self, tag: &str, name_span: Span, bits: &[TagBit], full_span: Span) {
        self.captured_closers.push(CapturedClosingTag {
            tag: tag.to_string(),
            name_span,
            bits: bits.to_vec(),
            full_span,
        });
    }

    fn finalize_frame(
        &mut self,
        frame: TreeFrame<'grammar, 'n>,
        closer_span: Span,
        closer_full_span: Span,
    ) {
        match frame {
            TreeFrame::Block(frame) => {
                let content_end = closer_span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                self.regions
                    .finalize_region_span(frame.segment_body, content_end);
                self.regions
                    .extend_region(frame.container_body, closer_full_span);
                self.regions
                    .extend_region(frame.parent_region, closer_full_span);
            }
            TreeFrame::Opaque(frame) => {
                let body_end = closer_span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                let body_span = Span::saturating_from_bounds_usize(
                    frame.body_start as usize,
                    body_end as usize,
                );
                let full_span = Span::saturating_from_bounds_usize(
                    frame.opener_span.start_usize(),
                    closer_full_span.end_usize(),
                );
                self.regions.push_node(
                    frame.parent_region,
                    TemplateNode::Opaque {
                        tag: frame.opener_name.to_string(),
                        name_span: frame.name_span,
                        bits: frame.bits.to_vec(),
                        full_span,
                        body_span,
                        body_analysis: frame.contract.body_analysis,
                    },
                );
            }
        }
    }

    fn add_intermediate(&mut self, tag_name: &str, name_span: Span, bits: &[TagBit], span: Span) {
        let full_span = span.expand_template_tag_marker();

        if let Some(TreeFrame::Block(frame)) = self.stack.last()
            && frame
                .contract
                .intermediates
                .iter()
                .any(|name| name == tag_name)
        {
            let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
            let segment_to_finalize = frame.segment_body;
            let container = frame.container_body;

            let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
            let new_segment_id = self.alloc_region(Span::new(body_start, 0), container);

            self.regions
                .finalize_region_span(segment_to_finalize, content_end);
            self.regions.push_node(
                container,
                TemplateNode::Block {
                    tag: tag_name.to_string(),
                    name_span,
                    bits: bits.to_vec(),
                    full_span,
                    body: new_segment_id,
                    role: BlockRole::Segment,
                },
            );

            if let Some(TreeFrame::Block(frame)) = self.stack.last_mut() {
                frame.segment_body = new_segment_id;
            }
        }
    }

    fn accumulate_orphaned_intermediate(
        &mut self,
        tag_name: &str,
        possible_openers: &[String],
        span: Span,
    ) {
        self.diagnostics.push(ValidationError::OrphanedTag {
            tag: tag_name.to_string(),
            context: describe_intermediate_parent(possible_openers),
            span,
        });
    }

    fn finish(&mut self) {
        while let Some(frame) = self.stack.pop() {
            match frame {
                TreeFrame::Opaque(_) => self.accumulate_unclosed(&frame),
                TreeFrame::Block(frame) if frame.contract.end_required => {
                    self.accumulate_unclosed(&TreeFrame::Block(frame));
                }
                TreeFrame::Block(frame) => {
                    self.regions
                        .extend_region(frame.container_body, frame.opener_span);
                }
            }
        }
    }

    fn accumulate_unclosed(&mut self, frame: &TreeFrame<'_, '_>) {
        let span = frame.opener_span();
        self.diagnostics.push(ValidationError::UnclosedTag {
            tag: frame.opener_name().to_string(),
            span,
        });
    }
}

trait TemplateTagSpanExt {
    fn expand_template_tag_marker(self) -> Span;
}

impl TemplateTagSpanExt for Span {
    fn expand_template_tag_marker(self) -> Span {
        self.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
    }
}

fn describe_intermediate_parent(possible_openers: &[String]) -> String {
    match possible_openers.len() {
        0 => "an open parent block".to_string(),
        1 => format!("an open '{}' block", possible_openers[0]),
        2 => format!(
            "an open '{}' or '{}' block",
            possible_openers[0], possible_openers[1]
        ),
        _ => {
            let mut parts = possible_openers
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>();
            let last = parts.pop().unwrap_or_default();
            let prefix = parts.join(", ");
            format!("one of these open blocks: {prefix}, or {last}")
        }
    }
}

enum TreeFrame<'grammar, 'n> {
    Block(BlockFrame<'grammar, 'n>),
    Opaque(OpaqueFrame<'grammar, 'n>),
}

impl<'grammar, 'n> TreeFrame<'grammar, 'n> {
    fn closer_name(&self) -> &'grammar str {
        &self.contract().closer
    }

    fn accepts_intermediate(&self, name: &str) -> bool {
        matches!(self, TreeFrame::Block(frame) if frame.contract.intermediates.iter().any(|candidate| candidate == name))
    }

    fn contract(&self) -> &'grammar OpeningContract {
        match self {
            TreeFrame::Block(frame) => frame.contract,
            TreeFrame::Opaque(frame) => frame.contract,
        }
    }

    fn opener_name(&self) -> &'n str {
        match self {
            TreeFrame::Block(frame) => frame.opener_name,
            TreeFrame::Opaque(frame) => frame.opener_name,
        }
    }

    fn opener_bits(&self) -> &'n [TagBit] {
        match self {
            TreeFrame::Block(frame) => frame.opener_bits,
            TreeFrame::Opaque(frame) => frame.bits,
        }
    }

    fn opener_span(&self) -> Span {
        match self {
            TreeFrame::Block(frame) => frame.opener_span,
            TreeFrame::Opaque(frame) => frame.opener_span,
        }
    }
}

/// Open frames, innermost last, indexed by the closer that ends each one.
///
/// Scanning every open frame for each tag's closer made deeply nested unclosed blocks
/// quadratic. A Template opens few distinct closers however deep it nests, so the index is a
/// short list rather than a hash map.
#[derive(Default)]
struct OpenFrames<'grammar, 'n> {
    frames: Vec<OpenFrame<'grammar, 'n>>,
    innermost_by_closer: Vec<(&'grammar str, usize)>,
}

struct OpenFrame<'grammar, 'n> {
    frame: TreeFrame<'grammar, 'n>,
    /// The next frame outward that the same closer ends.
    outer_with_same_closer: Option<usize>,
}

impl<'grammar, 'n> OpenFrames<'grammar, 'n> {
    fn push(&mut self, frame: TreeFrame<'grammar, 'n>) {
        let closer = frame.closer_name();
        let index = self.frames.len();
        let outer_with_same_closer = if let Some(entry) = self.innermost_entry(closer) {
            Some(std::mem::replace(
                &mut self.innermost_by_closer[entry].1,
                index,
            ))
        } else {
            self.innermost_by_closer.push((closer, index));
            None
        };
        self.frames.push(OpenFrame {
            frame,
            outer_with_same_closer,
        });
    }

    fn pop(&mut self) -> Option<TreeFrame<'grammar, 'n>> {
        let OpenFrame {
            frame,
            outer_with_same_closer,
        } = self.frames.pop()?;
        if let Some(entry) = self.innermost_entry(frame.closer_name()) {
            match outer_with_same_closer {
                Some(outer) => self.innermost_by_closer[entry].1 = outer,
                None => {
                    self.innermost_by_closer.swap_remove(entry);
                }
            }
        }
        Some(frame)
    }

    fn last(&self) -> Option<&TreeFrame<'grammar, 'n>> {
        self.frames.last().map(|open| &open.frame)
    }

    fn last_mut(&mut self) -> Option<&mut TreeFrame<'grammar, 'n>> {
        self.frames.last_mut().map(|open| &mut open.frame)
    }

    fn len(&self) -> usize {
        self.frames.len()
    }

    /// The index of the innermost open frame that `closer` ends.
    fn innermost_closed_by(&self, closer: &str) -> Option<usize> {
        self.innermost_entry(closer)
            .map(|entry| self.innermost_by_closer[entry].1)
    }

    fn innermost_entry(&self, closer: &str) -> Option<usize> {
        self.innermost_by_closer
            .iter()
            .position(|(open_closer, _)| *open_closer == closer)
    }
}

struct BlockFrame<'grammar, 'n> {
    opener_name: &'n str,
    contract: &'grammar OpeningContract,
    opener_bits: &'n [TagBit],
    opener_span: Span,
    container_body: RegionId,
    parent_region: RegionId,
    segment_body: RegionId,
}

#[derive(Clone, Copy)]
struct OpaqueFrame<'grammar, 'n> {
    opener_name: &'n str,
    contract: &'grammar OpeningContract,
    name_span: Span,
    bits: &'n [TagBit],
    opener_span: Span,
    parent_region: RegionId,
    body_start: u32,
}
