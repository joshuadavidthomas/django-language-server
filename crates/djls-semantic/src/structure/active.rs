use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;

use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StructuralOccurrenceMeaning {
    /// The occurrence uses its own opening or standalone Tag Definition.
    Definition,
    /// The occurrence is an intermediate captured by an already-open block contract.
    CapturedIntermediate,
    /// The occurrence is a closer captured by an already-open block contract.
    CapturedCloser,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CapturedClosingTag {
    pub tag: String,
    pub name_span: Span,
    pub bits: Vec<TagBit>,
    pub full_span: Span,
}

impl CapturedClosingTag {
    pub(crate) fn as_active(&self) -> ActiveTemplateTag<'_> {
        ActiveTemplateTag::new(
            &self.tag,
            self.name_span,
            &self.bits,
            self.full_span,
            StructuralOccurrenceMeaning::CapturedCloser,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActiveTemplateTag<'a> {
    pub tag: &'a str,
    pub name_span: Span,
    pub bits: &'a [TagBit],
    pub span: Span,
    pub full_span: Span,
    pub structural_meaning: StructuralOccurrenceMeaning,
}

impl<'a> ActiveTemplateTag<'a> {
    fn new(
        tag: &'a str,
        name_span: Span,
        bits: &'a [TagBit],
        full_span: Span,
        structural_meaning: StructuralOccurrenceMeaning,
    ) -> Self {
        Self {
            tag,
            name_span,
            bits,
            span: Span::saturating_from_bounds_usize(
                full_span
                    .start_usize()
                    .saturating_add(TagDelimiter::LENGTH_U32 as usize),
                full_span
                    .end_usize()
                    .saturating_sub(TagDelimiter::LENGTH_U32 as usize),
            ),
            full_span,
            structural_meaning,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActiveTemplateVariable<'a> {
    pub var: &'a str,
    pub var_span: Span,
    pub filters: &'a [Filter],
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveTemplateNode<'a> {
    Tag(ActiveTemplateTag<'a>),
    Variable(ActiveTemplateVariable<'a>),
}

impl<'a> ActiveTemplateNode<'a> {
    fn tag(
        tag: &'a str,
        name_span: Span,
        bits: &'a [TagBit],
        full_span: Span,
        structural_meaning: StructuralOccurrenceMeaning,
    ) -> Self {
        Self::Tag(ActiveTemplateTag::new(
            tag,
            name_span,
            bits,
            full_span,
            structural_meaning,
        ))
    }

    fn variable(var: &'a str, var_span: Span, filters: &'a [Filter], span: Span) -> Self {
        Self::Variable(ActiveTemplateVariable {
            var,
            var_span,
            filters,
            span,
        })
    }
}

#[must_use]
pub(crate) fn active_template_nodes(
    regions: &Regions,
    root: RegionId,
) -> Vec<ActiveTemplateNode<'_>> {
    let mut nodes = Vec::new();
    collect_active_nodes_for_region(regions, root, &mut nodes);
    nodes
}

#[must_use]
pub(crate) fn active_template_tags(
    regions: &Regions,
    root: RegionId,
) -> Vec<ActiveTemplateTag<'_>> {
    active_template_nodes(regions, root)
        .into_iter()
        .filter_map(|node| match node {
            ActiveTemplateNode::Tag(tag) => Some(tag),
            ActiveTemplateNode::Variable(_) => None,
        })
        .collect()
}

fn collect_active_nodes_for_region<'a>(
    regions: &'a Regions,
    region: RegionId,
    nodes: &mut Vec<ActiveTemplateNode<'a>>,
) {
    for node in regions.get(region).nodes() {
        collect_active_nodes_for_node(regions, node, nodes);
    }
}

fn collect_active_nodes_for_node<'a>(
    regions: &'a Regions,
    node: &'a TemplateNode,
    nodes: &mut Vec<ActiveTemplateNode<'a>>,
) {
    match node {
        TemplateNode::Block {
            tag,
            name_span,
            bits,
            full_span,
            body,
            role: BlockRole::Opener,
        } => {
            nodes.push(ActiveTemplateNode::tag(
                tag,
                *name_span,
                bits,
                *full_span,
                StructuralOccurrenceMeaning::Definition,
            ));
            collect_active_nodes_for_block_body(regions, *body, *full_span, nodes);
        }
        TemplateNode::Block {
            tag,
            name_span,
            bits,
            full_span,
            body,
            role: BlockRole::Segment,
        } => {
            nodes.push(ActiveTemplateNode::tag(
                tag,
                *name_span,
                bits,
                *full_span,
                StructuralOccurrenceMeaning::CapturedIntermediate,
            ));
            collect_active_nodes_for_region(regions, *body, nodes);
        }
        TemplateNode::StandaloneTag {
            tag,
            name_span,
            bits,
            full_span,
        } => nodes.push(ActiveTemplateNode::tag(
            tag,
            *name_span,
            bits,
            *full_span,
            StructuralOccurrenceMeaning::Definition,
        )),
        TemplateNode::Variable {
            var,
            var_span,
            filters,
            span,
        } => nodes.push(ActiveTemplateNode::variable(var, *var_span, filters, *span)),
        TemplateNode::Opaque {
            tag,
            name_span,
            bits,
            full_span,
            body_span,
            ..
        } => {
            let opener_full_span = Span::saturating_from_bounds_usize(
                full_span.start_usize(),
                body_span.start_usize(),
            );
            nodes.push(ActiveTemplateNode::tag(
                tag,
                *name_span,
                bits,
                opener_full_span,
                StructuralOccurrenceMeaning::Definition,
            ));
        }
        TemplateNode::Comment { .. } | TemplateNode::Text { .. } | TemplateNode::Error { .. } => {}
    }
}

fn collect_active_nodes_for_block_body<'a>(
    regions: &'a Regions,
    body: RegionId,
    opener_span: Span,
    nodes: &mut Vec<ActiveTemplateNode<'a>>,
) {
    for node in regions.get(body).nodes() {
        match node {
            TemplateNode::Block {
                body: segment_body,
                full_span,
                role: BlockRole::Segment,
                ..
            } if *full_span == opener_span => {
                collect_active_nodes_for_region(regions, *segment_body, nodes);
            }
            TemplateNode::Block { .. }
            | TemplateNode::Opaque { .. }
            | TemplateNode::StandaloneTag { .. }
            | TemplateNode::Variable { .. }
            | TemplateNode::Comment { .. }
            | TemplateNode::Text { .. }
            | TemplateNode::Error { .. } => {
                collect_active_nodes_for_node(regions, node, nodes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_bits() -> Vec<TagBit> {
        Vec::new()
    }

    #[test]
    fn active_template_nodes_preserve_source_order() {
        let root = RegionId::new(0);
        let if_container = RegionId::new(1);
        let if_segment = RegionId::new(2);
        let mut regions = Regions::from_allocations([
            (Span::new(0, 0), None),
            (Span::new(20, 0), Some(root)),
            (Span::new(30, 0), Some(if_container)),
        ]);
        let bits = empty_bits();

        regions.push_node(
            root,
            TemplateNode::StandaloneTag {
                tag: "load".to_string(),
                name_span: Span::new(3, 4),
                bits: bits.clone(),
                full_span: Span::new(0, 15),
            },
        );
        regions.push_node(
            root,
            TemplateNode::Block {
                tag: "if".to_string(),
                name_span: Span::new(18, 2),
                bits: bits.clone(),
                full_span: Span::new(15, 12),
                body: if_container,
                role: BlockRole::Opener,
            },
        );
        regions.push_node(
            if_container,
            TemplateNode::Block {
                tag: "if".to_string(),
                name_span: Span::new(18, 2),
                bits: bits.clone(),
                full_span: Span::new(15, 12),
                body: if_segment,
                role: BlockRole::Segment,
            },
        );
        regions.push_node(
            if_segment,
            TemplateNode::Variable {
                var: "value".to_string(),
                var_span: Span::new(30, 5),
                filters: Vec::new(),
                span: Span::new(27, 11),
            },
        );
        regions.push_node(
            root,
            TemplateNode::StandaloneTag {
                tag: "include".to_string(),
                name_span: Span::new(53, 7),
                bits,
                full_span: Span::new(50, 25),
            },
        );

        let labels = active_template_nodes(&regions, root)
            .into_iter()
            .map(|node| match node {
                ActiveTemplateNode::Tag(tag) => format!("tag:{}", tag.tag),
                ActiveTemplateNode::Variable(variable) => format!("var:{}", variable.var),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec!["tag:load", "tag:if", "var:value", "tag:include"]
        );
    }

    #[test]
    fn active_template_queries_skip_opaque_body_content() {
        let root = RegionId::new(0);
        let mut regions = Regions::from_allocations([(Span::new(0, 0), None)]);
        let bits = empty_bits();

        regions.push_node(
            root,
            TemplateNode::Opaque {
                tag: "verbatim".to_string(),
                name_span: Span::new(3, 8),
                bits,
                full_span: Span::new(0, 54),
                body_span: Span::new(14, 24),
            },
        );
        regions.push_node(
            root,
            TemplateNode::Variable {
                var: "active".to_string(),
                var_span: Span::new(59, 6),
                filters: Vec::new(),
                span: Span::new(56, 12),
            },
        );

        let tags = active_template_tags(&regions, root)
            .into_iter()
            .map(|tag| tag.tag.to_string())
            .collect::<Vec<_>>();
        let variables = active_template_nodes(&regions, root)
            .into_iter()
            .filter_map(|node| match node {
                ActiveTemplateNode::Tag(_) => None,
                ActiveTemplateNode::Variable(variable) => Some(variable.var.to_string()),
            })
            .collect::<Vec<_>>();

        assert_eq!(tags, vec!["verbatim"]);
        assert_eq!(variables, vec!["active"]);
    }
}
