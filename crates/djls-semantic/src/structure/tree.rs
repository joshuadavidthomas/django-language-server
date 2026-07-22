use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagBit;
use serde::Serialize;

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
    pub(crate) fn new(id: usize) -> Self {
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

    pub(crate) fn from_allocations(
        allocations: impl IntoIterator<Item = (Span, Option<RegionId>)>,
    ) -> Self {
        Self(
            allocations
                .into_iter()
                .map(|(span, parent)| TemplateRegion::new(span, parent))
                .collect(),
        )
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

    fn region_mut(&mut self, id: RegionId) -> &mut TemplateRegion {
        let idx = id.index();
        &mut self.0[idx]
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
        tag: String,
        name_span: Span,
        bits: Vec<TagBit>,
        full_span: Span,
        body: RegionId,
        role: BlockRole,
    },
    /// A paired opaque tag whose body is raw bytes with no internal structure.
    Opaque {
        tag: String,
        name_span: Span,
        bits: Vec<TagBit>,
        full_span: Span,
        body_span: Span,
    },
    StandaloneTag {
        tag: String,
        name_span: Span,
        bits: Vec<TagBit>,
        full_span: Span,
    },
    Variable {
        var: String,
        var_span: Span,
        filters: Vec<Filter>,
        span: Span,
    },
    Comment {
        span: Span,
    },
    Text {
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
            TemplateNode::Variable { span, .. }
            | TemplateNode::Comment { span }
            | TemplateNode::Text { span } => *span,
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
