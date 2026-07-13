use djls_source::Span;
use djls_templates::Filter;
use djls_templates::NodeList;
use djls_templates::ParseError;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;
use djls_templates::Visitor;
use salsa::Accumulator;

use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::structure::grammar::CloseValidation;
use crate::structure::grammar::ScopedTagIndex;
use crate::structure::grammar::TagClass;
use crate::structure::tree::BlockRole;
use crate::structure::tree::RegionId;
use crate::structure::tree::Regions;
use crate::structure::tree::TemplateNode;
use crate::structure::tree::TemplateTree;

#[derive(Debug, Clone)]
enum TreeOp {
    AddNode {
        target: RegionId,
        node: TemplateNode,
    },
    ExtendRegionSpan {
        id: RegionId,
        span: Span,
    },
    FinalizeSpanTo {
        id: RegionId,
        end: u32,
    },
    AccumulateDiagnostic(ValidationError),
}

pub(crate) struct TemplateTreeBuilder<'db, 'index> {
    db: &'db dyn Db,
    index: &'index ScopedTagIndex,
    root: RegionId,
    stack: Vec<TreeFrame>,
    region_allocs: Vec<(Span, Option<RegionId>)>,
    ops: Vec<TreeOp>,
    emit_diagnostics: bool,
}

impl<'db, 'index> TemplateTreeBuilder<'db, 'index> {
    pub(crate) fn new(db: &'db dyn Db, index: &'index ScopedTagIndex) -> Self {
        let mut builder = Self {
            db,
            index,
            root: RegionId::new(0),
            stack: Vec::new(),
            region_allocs: Vec::new(),
            ops: Vec::new(),
            emit_diagnostics: true,
        };
        builder.root = builder.alloc_region_id(Span::new(0, 0), None);
        builder
    }

    pub(crate) fn without_diagnostics(mut self) -> Self {
        self.emit_diagnostics = false;
        self
    }

    pub(crate) fn model(self, db: &'db dyn Db, nodelist: NodeList<'db>) -> TemplateTree<'db> {
        let emit_diagnostics = self.emit_diagnostics;
        let (tree, diagnostics) = self.model_deferred(db, nodelist);
        if emit_diagnostics {
            for error in diagnostics {
                ValidationErrorAccumulator(error).accumulate(db);
            }
        }
        tree
    }

    pub(crate) fn model_deferred(
        mut self,
        db: &'db dyn Db,
        nodelist: NodeList<'db>,
    ) -> (TemplateTree<'db>, Vec<ValidationError>) {
        for node in nodelist.nodelist(db) {
            self.visit_node(node);
        }
        self.finish();
        self.apply_operations()
    }

    fn alloc_region_id(&mut self, span: Span, parent: Option<RegionId>) -> RegionId {
        let id = RegionId::new(
            u32::try_from(self.region_allocs.len()).expect("template region count overflow"),
        );
        self.region_allocs.push((span, parent));
        id
    }

    fn apply_operations(self) -> (TemplateTree<'db>, Vec<ValidationError>) {
        let TemplateTreeBuilder {
            db,
            root,
            region_allocs,
            ops,
            ..
        } = self;

        let mut regions = Regions::default();
        let mut diagnostics = Vec::new();

        for (span, parent) in region_allocs {
            regions.alloc(span, parent);
        }

        for op in ops {
            match op {
                TreeOp::AddNode { target, node } => {
                    regions.push_node(target, node);
                }
                TreeOp::ExtendRegionSpan { id, span } => {
                    regions.extend_region(id, span);
                }
                TreeOp::FinalizeSpanTo { id, end } => {
                    regions.finalize_region_span(id, end);
                }
                TreeOp::AccumulateDiagnostic(error) => diagnostics.push(error),
            }
        }

        (TemplateTree::new(db, root, regions), diagnostics)
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

    fn handle_tag(&mut self, name: &str, name_span: Span, bits: &[TagBit], span: Span) {
        let full_span = span.expand_template_tag_marker();
        if self.close_active_opaque_if_closer(name, bits, span, full_span) {
            return;
        }
        if self.in_opaque_content() {
            return;
        }

        let index = self.index.at(span.start());
        if let Some(frame_idx) = self
            .stack
            .iter()
            .rposition(|frame| frame.closer_name() == name)
        {
            self.close_block_at(name, frame_idx, bits, span);
            return;
        }

        if matches!(self.stack.last(), Some(frame) if frame.accepts_intermediate(name)) {
            self.add_intermediate(name, name_span, bits, span);
            return;
        }

        match index.classify(name) {
            TagClass::Opener => {
                let parent = self.active_region();

                if index.is_opaque(name) {
                    self.stack.push(TreeFrame::Opaque(OpaqueFrame {
                        opener_name: name.to_string(),
                        closer_name: self
                            .index
                            .at(span.start())
                            .closer_name(name)
                            .expect("opaque opener should have closer")
                            .to_string(),
                        name_span,
                        bits: bits.to_vec(),
                        opener_span: full_span,
                        parent_region: parent,
                        body_start: span.end().saturating_add(TagDelimiter::LENGTH_U32),
                    }));
                    return;
                }

                let container = self.alloc_region_id(span, Some(parent));
                let segment = self.alloc_region_id(
                    Span::new(span.end().saturating_add(TagDelimiter::LENGTH_U32), 0),
                    Some(container),
                );

                self.ops.push(TreeOp::AddNode {
                    target: parent,
                    node: TemplateNode::Block {
                        tag: name.to_string(),
                        name_span,
                        bits: bits.to_vec(),
                        full_span,
                        body: container,
                        role: BlockRole::Opener,
                    },
                });
                self.ops.push(TreeOp::AddNode {
                    target: container,
                    node: TemplateNode::Block {
                        tag: name.to_string(),
                        name_span,
                        bits: bits.to_vec(),
                        full_span,
                        body: segment,
                        role: BlockRole::Segment,
                    },
                });

                self.stack.push(TreeFrame::Block(BlockFrame {
                    opener_name: name.to_string(),
                    closer_name: index
                        .closer_name(name)
                        .expect("block opener should have closer")
                        .to_string(),
                    intermediate_names: index.intermediate_names(name),
                    end_required: index.is_end_required(name),
                    opener_bits: bits.to_vec(),
                    opener_span: full_span,
                    container_body: container,
                    parent_region: parent,
                    segment_body: segment,
                }));
            }
            TagClass::Closer { possible_openers } => {
                self.accumulate_orphaned_closer(name, possible_openers, full_span);
            }
            TagClass::Intermediate { possible_openers } => {
                self.accumulate_orphaned_intermediate(name, possible_openers, full_span);
            }
            TagClass::Standalone | TagClass::Unknown => {
                self.add_standalone_tag(name, name_span, bits, full_span);
            }
        }
    }

    fn close_active_opaque_if_closer(
        &mut self,
        closer_name: &str,
        closer_bits: &[TagBit],
        span: Span,
        full_span: Span,
    ) -> bool {
        let should_close = matches!(
            self.stack.last(),
            Some(TreeFrame::Opaque(frame)) if frame.closer_name == closer_name
        );
        if !should_close {
            return false;
        }

        let Some(TreeFrame::Opaque(frame)) = self.stack.pop() else {
            unreachable!("checked active opaque frame before pop")
        };
        let opener_name = frame.opener_name.clone();
        match self.index.at(frame.opener_span.start()).validate_close(
            &opener_name,
            &frame.bits,
            closer_bits,
        ) {
            CloseValidation::Valid => {
                self.finalize_frame(TreeFrame::Opaque(frame), span, full_span);
            }
            CloseValidation::ArgumentMismatch {
                expected,
                got,
                got_span,
            } => {
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnmatchedBlockName {
                        expected,
                        got,
                        got_span,
                        span: full_span,
                        opener_span: frame.opener_span,
                    },
                ));
                self.finalize_frame(TreeFrame::Opaque(frame), span, full_span);
            }
            CloseValidation::NotABlock => {
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnbalancedStructure {
                        opening_tag: opener_name.clone(),
                        expected_closing: opener_name,
                        opening_span: frame.opener_span,
                        closing_span: Some(full_span),
                    },
                ));
                self.stack.push(TreeFrame::Opaque(frame));
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
        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::StandaloneTag {
                tag: tag_name.to_string(),
                name_span,
                bits: bits.to_vec(),
                full_span,
            },
        });
    }

    fn accumulate_orphaned_closer(
        &mut self,
        closer_name: &str,
        possible_openers: &[String],
        span: Span,
    ) {
        self.ops.push(TreeOp::AccumulateDiagnostic(
            ValidationError::OrphanedClosingTag {
                tag: closer_name.to_string(),
                expected_opener: possible_openers
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "matching opener".to_string()),
                span,
            },
        ));
    }

    fn close_block_at(
        &mut self,
        _closer_name: &str,
        frame_idx: usize,
        closer_bits: &[TagBit],
        span: Span,
    ) {
        let full_span = span.expand_template_tag_marker();
        let opener_name = self.stack[frame_idx].opener_name().to_string();

        while self.stack.len() > frame_idx + 1 {
            if let Some(unclosed) = self.stack.pop() {
                self.accumulate_unclosed(unclosed);
            }
        }

        let frame = self.stack.pop().unwrap();
        match self.index.at(frame.opener_span().start()).validate_close(
            &opener_name,
            frame.opener_bits(),
            closer_bits,
        ) {
            CloseValidation::Valid => {
                self.finalize_frame(frame, span, full_span);
            }
            CloseValidation::ArgumentMismatch {
                expected,
                got,
                got_span,
            } => {
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnmatchedBlockName {
                        expected,
                        got,
                        got_span,
                        span: full_span,
                        opener_span: frame.opener_span(),
                    },
                ));
                self.finalize_frame(frame, span, full_span);
            }
            CloseValidation::NotABlock => {
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnbalancedStructure {
                        opening_tag: opener_name.clone(),
                        expected_closing: opener_name,
                        opening_span: frame.opener_span(),
                        closing_span: Some(full_span),
                    },
                ));
                self.stack.push(frame);
            }
        }
    }

    fn finalize_frame(&mut self, frame: TreeFrame, closer_span: Span, closer_full_span: Span) {
        match frame {
            TreeFrame::Block(frame) => {
                let content_end = closer_span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                self.ops.push(TreeOp::FinalizeSpanTo {
                    id: frame.segment_body,
                    end: content_end,
                });
                self.ops.push(TreeOp::ExtendRegionSpan {
                    id: frame.container_body,
                    span: closer_full_span,
                });
                self.ops.push(TreeOp::ExtendRegionSpan {
                    id: frame.parent_region,
                    span: closer_full_span,
                });
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
                self.ops.push(TreeOp::AddNode {
                    target: frame.parent_region,
                    node: TemplateNode::Opaque {
                        tag: frame.opener_name,
                        name_span: frame.name_span,
                        bits: frame.bits,
                        full_span,
                        body_span,
                    },
                });
            }
        }
    }

    fn add_intermediate(&mut self, tag_name: &str, name_span: Span, bits: &[TagBit], span: Span) {
        let full_span = span.expand_template_tag_marker();

        if let Some(TreeFrame::Block(frame)) = self.stack.last()
            && frame.intermediate_names.iter().any(|name| name == tag_name)
        {
            let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
            let segment_to_finalize = frame.segment_body;
            let container = frame.container_body;

            self.ops.push(TreeOp::FinalizeSpanTo {
                id: segment_to_finalize,
                end: content_end,
            });

            let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
            let new_segment_id = self.alloc_region_id(Span::new(body_start, 0), Some(container));

            self.ops.push(TreeOp::AddNode {
                target: container,
                node: TemplateNode::Block {
                    tag: tag_name.to_string(),
                    name_span,
                    bits: bits.to_vec(),
                    full_span,
                    body: new_segment_id,
                    role: BlockRole::Segment,
                },
            });

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
        self.ops
            .push(TreeOp::AccumulateDiagnostic(ValidationError::OrphanedTag {
                tag: tag_name.to_string(),
                context: describe_intermediate_parent(possible_openers),
                span,
            }));
    }

    fn finish(&mut self) {
        while let Some(frame) = self.stack.pop() {
            match frame {
                TreeFrame::Opaque(_) => self.accumulate_unclosed(frame),
                TreeFrame::Block(frame) if frame.end_required => {
                    self.accumulate_unclosed(TreeFrame::Block(frame));
                }
                TreeFrame::Block(frame) => {
                    self.ops.push(TreeOp::ExtendRegionSpan {
                        id: frame.container_body,
                        span: frame.opener_span,
                    });
                }
            }
        }
    }

    fn accumulate_unclosed(&mut self, frame: TreeFrame) {
        let span = frame.opener_span();
        self.ops
            .push(TreeOp::AccumulateDiagnostic(ValidationError::UnclosedTag {
                tag: frame.into_opener_name(),
                span,
            }));
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

enum TreeFrame {
    Block(BlockFrame),
    Opaque(OpaqueFrame),
}

impl TreeFrame {
    fn opener_name(&self) -> &str {
        match self {
            TreeFrame::Block(frame) => &frame.opener_name,
            TreeFrame::Opaque(frame) => &frame.opener_name,
        }
    }

    fn closer_name(&self) -> &str {
        match self {
            TreeFrame::Block(frame) => &frame.closer_name,
            TreeFrame::Opaque(frame) => &frame.closer_name,
        }
    }

    fn accepts_intermediate(&self, name: &str) -> bool {
        matches!(self, TreeFrame::Block(frame) if frame.intermediate_names.iter().any(|candidate| candidate == name))
    }

    fn into_opener_name(self) -> String {
        match self {
            TreeFrame::Block(frame) => frame.opener_name,
            TreeFrame::Opaque(frame) => frame.opener_name,
        }
    }

    fn opener_bits(&self) -> &[TagBit] {
        match self {
            TreeFrame::Block(frame) => &frame.opener_bits,
            TreeFrame::Opaque(frame) => &frame.bits,
        }
    }

    fn opener_span(&self) -> Span {
        match self {
            TreeFrame::Block(frame) => frame.opener_span,
            TreeFrame::Opaque(frame) => frame.opener_span,
        }
    }
}

struct BlockFrame {
    opener_name: String,
    closer_name: String,
    intermediate_names: Vec<String>,
    end_required: bool,
    opener_bits: Vec<TagBit>,
    opener_span: Span,
    container_body: RegionId,
    parent_region: RegionId,
    segment_body: RegionId,
}

struct OpaqueFrame {
    opener_name: String,
    closer_name: String,
    name_span: Span,
    bits: Vec<TagBit>,
    opener_span: Span,
    parent_region: RegionId,
    body_start: u32,
}

impl Visitor for TemplateTreeBuilder<'_, '_> {
    fn visit_tag(&mut self, name: &str, name_span: Span, bits: &[TagBit], span: Span) {
        self.handle_tag(name, name_span, bits, span);
    }

    fn visit_comment(&mut self, _content: &str, span: Span) {
        if self.in_opaque_content() {
            return;
        }

        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::Comment { span },
        });
    }

    fn visit_variable(&mut self, var: &str, var_span: Span, filters: &[Filter], span: Span) {
        if self.in_opaque_content() {
            return;
        }

        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::Variable {
                var: var.to_string(),
                var_span,
                filters: filters.to_vec(),
                span,
            },
        });
    }

    fn visit_error(&mut self, span: Span, full_span: Span, _error: &ParseError) {
        if self.in_opaque_content() {
            return;
        }

        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::Error { span, full_span },
        });
    }

    fn visit_text(&mut self, span: Span) {
        if self.in_opaque_content() {
            return;
        }

        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::Text { span },
        });
    }
}
