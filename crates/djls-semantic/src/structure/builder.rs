use djls_source::Span;
use djls_templates::Filter;
use djls_templates::ParseError;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;
use djls_templates::Visitor;
use salsa::Accumulator;

use crate::structure::grammar::CloseValidation;
use crate::structure::grammar::TagClass;
use crate::structure::grammar::TagIndex;
use crate::structure::tree::BlockRole;
use crate::structure::tree::RegionId;
use crate::structure::tree::Regions;
use crate::structure::tree::TemplateNode;
use crate::structure::tree::TemplateTree;
use crate::traits::SemanticModel;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

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

pub(crate) struct TemplateTreeBuilder<'db> {
    db: &'db dyn Db,
    index: TagIndex<'db>,
    root: RegionId,
    stack: Vec<TreeFrame>,
    region_allocs: Vec<(Span, Option<RegionId>)>,
    ops: Vec<TreeOp>,
}

impl<'db> TemplateTreeBuilder<'db> {
    pub(crate) fn new(db: &'db dyn Db, index: TagIndex<'db>) -> Self {
        let mut builder = Self {
            db,
            index,
            root: RegionId::new(0),
            stack: Vec::new(),
            region_allocs: Vec::new(),
            ops: Vec::new(),
        };
        builder.root = builder.alloc_region_id(Span::new(0, 0), None);
        builder
    }

    fn alloc_region_id(&mut self, span: Span, parent: Option<RegionId>) -> RegionId {
        let id = RegionId::new(
            u32::try_from(self.region_allocs.len()).expect("template region count overflow"),
        );
        self.region_allocs.push((span, parent));
        id
    }

    fn apply_operations(self) -> TemplateTree<'db> {
        let TemplateTreeBuilder {
            db,
            root,
            region_allocs,
            ops,
            ..
        } = self;

        let mut regions = Regions::default();

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
                TreeOp::AccumulateDiagnostic(error) => {
                    ValidationErrorAccumulator(error).accumulate(db);
                }
            }
        }

        TemplateTree::new(db, root, regions)
    }

    fn active_region(&self) -> RegionId {
        self.stack
            .last()
            .map_or(self.root, |frame| frame.segment_body)
    }

    fn handle_tag(&mut self, name: &str, name_span: Span, bits: &[TagBit], span: Span) {
        let full_span = span.expand_template_tag_marker();
        match self.index.classify(self.db, name) {
            TagClass::Opener => {
                let parent = self.active_region();

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

                self.stack.push(TreeFrame {
                    opener_name: name.to_string(),
                    opener_bits: bits.to_vec(),
                    opener_span: full_span,
                    container_body: container,
                    parent_region: parent,
                    segment_body: segment,
                });
            }
            TagClass::Closer { opener_name } => {
                self.close_block(name, opener_name, bits, span);
            }
            TagClass::Intermediate { possible_openers } => {
                self.add_intermediate(name, possible_openers, name_span, bits, span);
            }
            TagClass::Unknown => {
                self.ops.push(TreeOp::AddNode {
                    target: self.active_region(),
                    node: TemplateNode::StandaloneTag {
                        tag: name.to_string(),
                        name_span,
                        bits: bits.to_vec(),
                        full_span,
                    },
                });
            }
        }
    }

    fn close_block(
        &mut self,
        closer_name: &str,
        opener_name: &str,
        closer_bits: &[TagBit],
        span: Span,
    ) {
        let full_span = span.expand_template_tag_marker();

        let Some(frame_idx) = self
            .stack
            .iter()
            .rposition(|frame| frame.opener_name == opener_name)
        else {
            self.ops.push(TreeOp::AccumulateDiagnostic(
                ValidationError::OrphanedClosingTag {
                    tag: closer_name.to_string(),
                    expected_opener: opener_name.to_string(),
                    span: full_span,
                },
            ));
            return;
        };

        while self.stack.len() > frame_idx + 1 {
            if let Some(unclosed) = self.stack.pop() {
                self.ops
                    .push(TreeOp::AccumulateDiagnostic(ValidationError::UnclosedTag {
                        tag: unclosed.opener_name,
                        span: unclosed.opener_span,
                    }));
            }
        }

        let frame = self.stack.pop().unwrap();
        match self
            .index
            .validate_close(self.db, opener_name, &frame.opener_bits, closer_bits)
        {
            CloseValidation::Valid => {
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                self.ops.push(TreeOp::FinalizeSpanTo {
                    id: frame.segment_body,
                    end: content_end,
                });
                self.ops.push(TreeOp::ExtendRegionSpan {
                    id: frame.container_body,
                    span: full_span,
                });
                self.ops.push(TreeOp::ExtendRegionSpan {
                    id: frame.parent_region,
                    span: full_span,
                });
            }
            CloseValidation::ArgumentMismatch { expected, got } => {
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnmatchedBlockName {
                        expected,
                        got,
                        span: full_span,
                    },
                ));
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                self.ops.push(TreeOp::FinalizeSpanTo {
                    id: frame.segment_body,
                    end: content_end,
                });
                self.ops.push(TreeOp::ExtendRegionSpan {
                    id: frame.container_body,
                    span: full_span,
                });
                self.ops.push(TreeOp::ExtendRegionSpan {
                    id: frame.parent_region,
                    span: full_span,
                });
            }
            CloseValidation::NotABlock => {
                self.ops.push(TreeOp::AccumulateDiagnostic(
                    ValidationError::UnbalancedStructure {
                        opening_tag: opener_name.to_string(),
                        expected_closing: opener_name.to_string(),
                        opening_span: frame.opener_span,
                        closing_span: Some(full_span),
                    },
                ));
                self.stack.push(frame);
            }
        }
    }

    fn add_intermediate(
        &mut self,
        tag_name: &str,
        possible_openers: &[String],
        name_span: Span,
        bits: &[TagBit],
        span: Span,
    ) {
        let full_span = span.expand_template_tag_marker();

        if let Some(frame) = self.stack.last() {
            if possible_openers.contains(&frame.opener_name) {
                let content_end = span.start().saturating_sub(TagDelimiter::LENGTH_U32);
                let segment_to_finalize = frame.segment_body;
                let container = frame.container_body;

                self.ops.push(TreeOp::FinalizeSpanTo {
                    id: segment_to_finalize,
                    end: content_end,
                });

                let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
                let new_segment_id =
                    self.alloc_region_id(Span::new(body_start, 0), Some(container));

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

                self.stack.last_mut().unwrap().segment_body = new_segment_id;
            } else {
                self.accumulate_orphaned_intermediate(tag_name, possible_openers, full_span);
            }
        } else {
            self.accumulate_orphaned_intermediate(tag_name, possible_openers, full_span);
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
            if self.index.is_end_required(self.db, &frame.opener_name) {
                self.ops
                    .push(TreeOp::AccumulateDiagnostic(ValidationError::UnclosedTag {
                        tag: frame.opener_name,
                        span: frame.opener_span,
                    }));
            } else {
                self.ops.push(TreeOp::ExtendRegionSpan {
                    id: frame.container_body,
                    span: frame.opener_span,
                });
            }
        }
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

struct TreeFrame {
    opener_name: String,
    opener_bits: Vec<TagBit>,
    opener_span: Span,
    container_body: RegionId,
    parent_region: RegionId,
    segment_body: RegionId,
}

impl Visitor for TemplateTreeBuilder<'_> {
    fn visit_tag(&mut self, name: &str, name_span: Span, bits: &[TagBit], span: Span) {
        self.handle_tag(name, name_span, bits, span);
    }

    fn visit_comment(&mut self, _content: &str, span: Span) {
        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::Comment { span },
        });
    }

    fn visit_variable(&mut self, var: &str, var_span: Span, filters: &[Filter], span: Span) {
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
        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::Error { span, full_span },
        });
    }

    fn visit_text(&mut self, span: Span) {
        self.ops.push(TreeOp::AddNode {
            target: self.active_region(),
            node: TemplateNode::Text { span },
        });
    }
}

impl<'db> SemanticModel<'db> for TemplateTreeBuilder<'db> {
    type Model = TemplateTree<'db>;

    fn construct(mut self) -> Self::Model {
        self.finish();
        self.apply_operations()
    }
}
