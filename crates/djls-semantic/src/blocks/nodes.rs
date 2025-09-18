use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct BlockId(u32);

impl BlockId {
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    pub fn id(self) -> u32 {
        self.0
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Blocks(Vec<Region>);

impl Blocks {
    pub fn get(&self, id: usize) -> &Region {
        &self.0[id]
    }
}

impl IntoIterator for Blocks {
    type Item = Region;
    type IntoIter = std::vec::IntoIter<Region>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Blocks {
    type Item = &'a Region;
    type IntoIter = std::slice::Iter<'a, Region>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a mut Blocks {
    type Item = &'a mut Region;
    type IntoIter = std::slice::IterMut<'a, Region>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

impl Blocks {
    pub fn alloc_with_parent(&mut self, span: Span, parent: Option<BlockId>) -> BlockId {
        let id = BlockId(self.0.len() as u32);
        self.0.push(Region::new(span, parent));
        id
    }

    pub fn alloc(&mut self, span: Span) -> BlockId {
        self.alloc_with_parent(span, None)
    }

    pub fn add_block(
        &mut self,
        parent: BlockId,
        tag: &str,
        marker_span: Span,
    ) -> (BlockId, BlockId) {
        // Container region begins at opener marker; expands to include closer
        let container = self.alloc_with_parent(marker_span, Some(parent));

        self.push_node(
            parent,
            BlockNode::Branch {
                tag: tag.to_string(),
                marker_span,
                body: container,
                kind: BranchKind::Opener,
            },
        );
        let body = self.add_segment(container, tag.to_string(), marker_span);

        (container, body)
    }

    pub fn add_segment(&mut self, container: BlockId, tag: String, marker_span: Span) -> BlockId {
        // Body represents the content region after the marker; start at marker.full_end(), length 0
        let body_start = marker_span.end().saturating_add(TagDelimiter::LENGTH_U32);
        let body = self.alloc_with_parent(Span::new(body_start, 0), Some(container));

        self.push_node(
            container,
            BlockNode::Branch {
                tag,
                marker_span,
                body,
                kind: BranchKind::Segment,
            },
        );

        body
    }

    pub fn add_leaf(&mut self, target: BlockId, label: String, span: Span) {
        self.push_node(target, BlockNode::Leaf { label, span });
    }

    pub fn add_error(&mut self, target: BlockId, message: String, span: Span) {
        self.push_node(target, BlockNode::Error { message, span });
    }

    pub fn extend(&mut self, id: BlockId, span: Span) {
        self.block_mut(id).extend_span(span);
    }

    pub fn finalize_body_to(&mut self, id: BlockId, content_end: u32) {
        let block = self.block_mut(id);
        let start = block.span.start();
        block.span = Span::saturating_from_bounds_usize(start as usize, content_end as usize);
    }

    fn push_node(&mut self, target: BlockId, node: BlockNode) {
        let span = node.span();
        self.extend(target, span);
        self.block_mut(target).nodes.push(node);
    }

    fn block_mut(&mut self, id: BlockId) -> &mut Region {
        let idx = id.index();
        &mut self.0[idx]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Region {
    span: Span,
    nodes: Vec<BlockNode>,
    parent: Option<BlockId>,
}

impl Region {
    fn new(span: Span, parent: Option<BlockId>) -> Self {
        Self {
            span,
            nodes: Vec::new(),
            parent,
        }
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub fn nodes(&self) -> &Vec<BlockNode> {
        &self.nodes
    }

    fn extend_span(&mut self, span: Span) {
        let opening = self.span.start().saturating_sub(span.start());
        let closing = span.end().saturating_sub(self.span.end());
        self.span = self.span.expand(opening, closing);
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum BranchKind {
    Opener,
    Segment,
}

#[derive(Clone, Debug, Serialize)]
pub enum BlockNode {
    Leaf {
        label: String,
        span: Span,
    },
    Branch {
        tag: String,
        marker_span: Span,
        body: BlockId,
        kind: BranchKind,
    },
    Error {
        message: String,
        span: Span,
    },
}

impl BlockNode {
    fn span(&self) -> Span {
        match self {
            BlockNode::Leaf { span, .. } | BlockNode::Error { span, .. } => *span,
            BlockNode::Branch { marker_span, .. } => *marker_span,
        }
    }
}
