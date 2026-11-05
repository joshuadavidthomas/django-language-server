use djls_source::Span;
use djls_templates::Filter;
use djls_templates::NodeText;
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
    pub tag: NodeText,
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
    /// For a captured intermediate, the name span of the opener whose contract captured it.
    pub opener_name_span: Option<Span>,
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
            opener_name_span: None,
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
pub(crate) fn active_template_nodes(regions: &Regions, root: RegionId) -> ActiveTemplateNodes<'_> {
    let mut nodes = ActiveTemplateNodes {
        regions,
        stack: Vec::new(),
    };
    nodes.descend(root, None);
    nodes
}

pub(crate) fn active_template_tags(
    regions: &Regions,
    root: RegionId,
) -> impl Iterator<Item = ActiveTemplateTag<'_>> {
    active_template_nodes(regions, root).filter_map(|node| match node {
        ActiveTemplateNode::Tag(tag) => Some(tag),
        ActiveTemplateNode::Variable(_) => None,
    })
}

/// Template nodes whose meaning is active, in source order.
///
/// Walks the region tree with an explicit stack, so deeply nested blocks cannot overflow the
/// thread's call stack.
pub(crate) struct ActiveTemplateNodes<'a> {
    regions: &'a Regions,
    stack: Vec<ActiveWalkFrame<'a>>,
}

struct ActiveWalkFrame<'a> {
    nodes: std::slice::Iter<'a, TemplateNode>,
    /// The opener's full and name spans when these nodes are a block's segments.
    opener: Option<(Span, Span)>,
}

impl ActiveTemplateNodes<'_> {
    fn descend(&mut self, region: RegionId, opener: Option<(Span, Span)>) {
        self.stack.push(ActiveWalkFrame {
            nodes: self.regions.get(region).nodes().iter(),
            opener,
        });
    }
}

impl<'a> Iterator for ActiveTemplateNodes<'a> {
    type Item = ActiveTemplateNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.stack.last_mut()?;
            let opener = frame.opener;
            let Some(node) = frame.nodes.next() else {
                self.stack.pop();
                continue;
            };
            match node {
                TemplateNode::Block {
                    tag,
                    name_span,
                    bits,
                    full_span,
                    body,
                    role: BlockRole::Opener,
                } => {
                    self.descend(*body, Some((*full_span, *name_span)));
                    return Some(ActiveTemplateNode::tag(
                        tag,
                        *name_span,
                        bits,
                        *full_span,
                        StructuralOccurrenceMeaning::Definition,
                    ));
                }
                TemplateNode::Block {
                    tag,
                    name_span,
                    bits,
                    full_span,
                    body,
                    role: BlockRole::Segment,
                } => {
                    self.descend(*body, None);
                    // A block's first segment is the opener itself.
                    if opener.is_some_and(|(opener_span, _)| opener_span == *full_span) {
                        continue;
                    }
                    let mut intermediate = ActiveTemplateTag::new(
                        tag,
                        *name_span,
                        bits,
                        *full_span,
                        StructuralOccurrenceMeaning::CapturedIntermediate,
                    );
                    intermediate.opener_name_span = opener.map(|(_, name_span)| name_span);
                    return Some(ActiveTemplateNode::Tag(intermediate));
                }
                TemplateNode::StandaloneTag {
                    tag,
                    name_span,
                    bits,
                    full_span,
                } => {
                    return Some(ActiveTemplateNode::tag(
                        tag,
                        *name_span,
                        bits,
                        *full_span,
                        StructuralOccurrenceMeaning::Definition,
                    ));
                }
                TemplateNode::Variable {
                    var,
                    var_span,
                    filters,
                    span,
                } => {
                    return Some(ActiveTemplateNode::variable(var, *var_span, filters, *span));
                }
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
                    return Some(ActiveTemplateNode::tag(
                        tag,
                        *name_span,
                        bits,
                        opener_full_span,
                        StructuralOccurrenceMeaning::Definition,
                    ));
                }
                TemplateNode::Comment { .. } | TemplateNode::Error { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_bits() -> Box<[TagBit]> {
        Box::default()
    }

    #[test]
    fn active_template_nodes_preserve_source_order() {
        let mut regions = Regions::default();
        let root = regions.alloc(Span::new(0, 0));
        let if_container = regions.alloc(Span::new(20, 0));
        let if_segment = regions.alloc(Span::new(30, 0));
        let bits = empty_bits();

        regions.push_node(
            root,
            TemplateNode::StandaloneTag {
                tag: "load".into(),
                name_span: Span::new(3, 4),
                bits: bits.clone(),
                full_span: Span::new(0, 15),
            },
        );
        regions.push_node(
            root,
            TemplateNode::Block {
                tag: "if".into(),
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
                tag: "if".into(),
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
                var: "value".into(),
                var_span: Span::new(30, 5),
                filters: Box::default(),
                span: Span::new(27, 11),
            },
        );
        regions.push_node(
            root,
            TemplateNode::StandaloneTag {
                tag: "include".into(),
                name_span: Span::new(53, 7),
                bits,
                full_span: Span::new(50, 25),
            },
        );

        let labels = active_template_nodes(&regions, root)
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
        let mut regions = Regions::default();
        let root = regions.alloc(Span::new(0, 0));
        let bits = empty_bits();

        regions.push_node(
            root,
            TemplateNode::Opaque {
                tag: "verbatim".into(),
                name_span: Span::new(3, 8),
                bits,
                full_span: Span::new(0, 54),
                body_span: Span::new(14, 24),
                body_analysis: crate::tags::BodyAnalysis::Verbatim,
            },
        );
        regions.push_node(
            root,
            TemplateNode::Variable {
                var: "active".into(),
                var_span: Span::new(59, 6),
                filters: Box::default(),
                span: Span::new(56, 12),
            },
        );

        let tags = active_template_tags(&regions, root)
            .map(|tag| tag.tag.to_string())
            .collect::<Vec<_>>();
        let variables = active_template_nodes(&regions, root)
            .filter_map(|node| match node {
                ActiveTemplateNode::Tag(_) => None,
                ActiveTemplateNode::Variable(variable) => Some(variable.var.to_string()),
            })
            .collect::<Vec<_>>();

        assert_eq!(tags, vec!["verbatim"]);
        assert_eq!(variables, vec!["active"]);
    }
}
