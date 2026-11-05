use djls_source::Span;
use djls_templates::Filter;
use djls_templates::NodeText;
use djls_templates::TagBit;
use serde::Serialize;

use crate::tags::BodyAnalysis;

#[salsa::tracked]
pub struct TemplateTree<'db> {
    #[returns(copy)]
    pub root: RegionId,
    #[returns(ref)]
    pub regions: Regions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct RegionId(usize);

impl RegionId {
    #[must_use]
    fn new(id: usize) -> Self {
        Self(id)
    }

    #[must_use]
    pub fn id(self) -> usize {
        self.0
    }

    #[must_use]
    fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Regions(Vec<TemplateRegion>);

impl Regions {
    #[must_use]
    pub fn get(&self, id: RegionId) -> &TemplateRegion {
        &self[id]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, TemplateRegion> {
        self.0.iter()
    }

    pub(crate) fn alloc(&mut self, span: Span, parent: Option<RegionId>) -> RegionId {
        let id = RegionId::new(self.0.len());
        self.0.push(TemplateRegion::new(span, parent));
        id
    }

    pub(crate) fn extend_region(&mut self, id: RegionId, span: Span) {
        self.region_mut(id).extend_span(span);
    }

    pub(crate) fn finalize_region_span(&mut self, id: RegionId, end: u32) {
        let region = self.region_mut(id);
        let start = region.span().start();
        region.set_span(Span::saturating_from_bounds_usize(
            start as usize,
            end as usize,
        ));
    }

    pub(crate) fn push_node(&mut self, target: RegionId, node: TemplateNode) {
        let span = node.span();
        self.extend_region(target, span);
        self.region_mut(target).nodes.push(node);
    }

    /// Regions and their node lists grow while building and are kept with the tree.
    pub(crate) fn shrink_to_fit(&mut self) {
        self.0.shrink_to_fit();
        for region in &mut self.0 {
            region.nodes.shrink_to_fit();
        }
    }

    fn region_mut(&mut self, id: RegionId) -> &mut TemplateRegion {
        let idx = id.index();
        &mut self.0[idx]
    }

    pub(crate) fn map_spans(&mut self, mut map: impl FnMut(Span) -> Span) {
        for region in &mut self.0 {
            region.span = map(region.span);
            for node in &mut region.nodes {
                match node {
                    TemplateNode::Block {
                        name_span,
                        bits,
                        full_span,
                        ..
                    }
                    | TemplateNode::StandaloneTag {
                        name_span,
                        bits,
                        full_span,
                        ..
                    } => {
                        *name_span = map(*name_span);
                        crate::layout::map_bits(bits, &mut map);
                        *full_span = map(*full_span);
                    }
                    TemplateNode::Opaque {
                        name_span,
                        bits,
                        full_span,
                        body_span,
                        ..
                    } => {
                        *name_span = map(*name_span);
                        crate::layout::map_bits(bits, &mut map);
                        *full_span = map(*full_span);
                        *body_span = map(*body_span);
                    }
                    TemplateNode::Variable {
                        var_span,
                        filters,
                        span,
                        ..
                    } => {
                        *var_span = map(*var_span);
                        crate::layout::map_filters(filters, &mut map);
                        *span = map(*span);
                    }
                    TemplateNode::Comment { span } => {
                        *span = map(*span);
                    }
                    TemplateNode::Error { span, full_span } => {
                        *span = map(*span);
                        *full_span = map(*full_span);
                    }
                }
            }
        }
    }
}

impl std::ops::Index<RegionId> for Regions {
    type Output = TemplateRegion;

    fn index(&self, id: RegionId) -> &Self::Output {
        &self.0[id.index()]
    }
}

impl<'a> IntoIterator for &'a Regions {
    type Item = &'a TemplateRegion;
    type IntoIter = std::slice::Iter<'a, TemplateRegion>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct TemplateRegion {
    span: Span,
    nodes: Vec<TemplateNode>,
    parent: Option<RegionId>,
}

impl TemplateRegion {
    fn new(span: Span, parent: Option<RegionId>) -> Self {
        Self {
            span,
            nodes: Vec::new(),
            parent,
        }
    }

    #[must_use]
    pub fn span(&self) -> &Span {
        &self.span
    }

    #[must_use]
    pub fn nodes(&self) -> &[TemplateNode] {
        &self.nodes
    }

    #[must_use]
    pub fn parent(&self) -> Option<RegionId> {
        self.parent
    }

    fn set_span(&mut self, span: Span) {
        self.span = span;
    }

    fn extend_span(&mut self, span: Span) {
        let opening = self.span.start().saturating_sub(span.start());
        let closing = span.end().saturating_sub(self.span.end());
        self.span = self.span.expand(opening, closing);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum BlockRole {
    /// A block tag attached to its parent region. Its body points to the
    /// container region that owns the block's segments.
    Opener,
    /// A block segment attached to a block container region. Its body points to
    /// the content region for that segment.
    Segment,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum TemplateNode {
    /// A structural block node.
    ///
    /// Blocks are represented in two arena hops: an `Opener` node appears in
    /// the parent content region and points to a container region; that
    /// container owns one or more `Segment` nodes, each pointing to its content
    /// region. This keeps intermediate tags like `elif`/`else` in source order
    /// without nested ownership inside the Salsa-tracked tree.
    Block {
        tag: NodeText,
        name_span: Span,
        bits: Box<[TagBit]>,
        full_span: Span,
        body: RegionId,
        role: BlockRole,
    },
    /// A paired opaque tag whose body is raw bytes with no internal structure.
    Opaque {
        tag: NodeText,
        name_span: Span,
        bits: Box<[TagBit]>,
        full_span: Span,
        body_span: Span,
        body_analysis: BodyAnalysis,
    },
    StandaloneTag {
        tag: NodeText,
        name_span: Span,
        bits: Box<[TagBit]>,
        full_span: Span,
    },
    Variable {
        var: NodeText,
        var_span: Span,
        filters: Box<[Filter]>,
        span: Span,
    },
    Comment {
        span: Span,
    },
    Error {
        span: Span,
        full_span: Span,
    },
}

impl TemplateNode {
    fn span(&self) -> Span {
        match self {
            TemplateNode::Block { full_span, .. }
            | TemplateNode::Opaque { full_span, .. }
            | TemplateNode::StandaloneTag { full_span, .. }
            | TemplateNode::Error { full_span, .. } => *full_span,
            TemplateNode::Variable { span, .. } | TemplateNode::Comment { span } => *span,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::RegionId;

    #[test]
    fn region_id_preserves_the_platform_index_range() {
        let id = RegionId::new(usize::MAX);

        assert_eq!(id.id(), usize::MAX);
        assert_eq!(size_of::<RegionId>(), size_of::<usize>());
    }
}
