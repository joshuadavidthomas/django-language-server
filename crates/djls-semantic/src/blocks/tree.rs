use djls_source::Span;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[doc(hidden)]
pub struct BlockTreeInner {
    pub roots: Vec<BlockId>,
    pub blocks: Blocks,
}

#[salsa::tracked]
pub struct BlockTree<'db> {
    #[returns(ref)]
    inner: BlockTreeInner,
}

impl<'db> BlockTree<'db> {
    pub fn roots(self, db: &'db dyn crate::Db) -> &'db [BlockId] {
        &self.inner(db).roots
    }
    
    pub fn blocks(self, db: &'db dyn crate::Db) -> &'db Blocks {
        &self.inner(db).blocks
    }

}

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

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
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
    pub fn alloc(&mut self, span: Span, parent: Option<BlockId>) -> BlockId {
        let id = BlockId(u32::try_from(self.0.len()).unwrap_or_default());
        self.0.push(Region::new(span, parent));
        id
    }

    pub fn extend_block(&mut self, id: BlockId, span: Span) {
        self.block_mut(id).extend_span(span);
    }

    pub fn set_block_span(&mut self, id: BlockId, span: Span) {
        self.block_mut(id).set_span(span);
    }

    pub fn finalize_block_span(&mut self, id: BlockId, end: u32) {
        let block = self.block_mut(id);
        let start = block.span().start();
        block.set_span(Span::saturating_from_bounds_usize(
            start as usize,
            end as usize,
        ));
    }

    pub fn push_node(&mut self, target: BlockId, node: BlockNode) {
        let span = node.span();
        self.extend_block(target, span);
        self.block_mut(target).nodes.push(node);
    }

    fn block_mut(&mut self, id: BlockId) -> &mut Region {
        let idx = id.index();
        &mut self.0[idx]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
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

    pub fn set_span(&mut self, span: Span) {
        self.span = span;
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum BranchKind {
    Opener,
    Segment,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
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
}

impl BlockNode {
    pub fn span(&self) -> Span {
        match self {
            Self::Leaf { span, .. } | Self::Branch { marker_span: span, .. } => *span,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;
    use insta::assert_yaml_snapshot;

    use super::*;
    use crate::build_block_tree;
    use crate::templatetags::django_builtin_specs;
    use crate::TagIndex;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            }
        }

        fn add_file(&self, path: &str, content: &str) {
            self.fs
                .lock()
                .unwrap()
                .add_file(path.into(), content.to_string());
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_source::Db for TestDatabase {
        fn read_file_source(&self, path: &Utf8Path) -> std::io::Result<String> {
            self.fs.lock().unwrap().read_to_string(path)
        }
    }

    #[salsa::db]
    impl djls_templates::Db for TestDatabase {}

    #[salsa::db]
    impl crate::Db for TestDatabase {
        fn tag_specs(&self) -> crate::templatetags::TagSpecs {
            django_builtin_specs()
        }

        fn tag_index(&self) -> TagIndex {
            TagIndex::from_specs(&self.tag_specs())
        }
    }

    #[derive(Clone, Debug, Serialize)]
    struct RootSnapshot {
        root_idx: usize,
        blocks: Vec<BlockSnapshot>,
    }

    #[derive(Clone, Debug, Serialize)]
    enum BlockSnapshot {
        Leaf {
            label: String,
            span: [u32; 2],
        },
        Branch {
            tag: String,
            marker_span: [u32; 2],
            kind: &'static str,
            nested_blocks: Vec<BlockSnapshot>,
        },
    }

    #[derive(Clone, Debug, Serialize)]
    struct TreeSnapshot {
        roots: Vec<RootSnapshot>,
    }

    impl TreeSnapshot {
        fn extract_region(blocks: &Blocks, id: usize) -> Vec<BlockSnapshot> {
            let block = blocks.get(id);
            let mut snapshots = Vec::new();

            for node in block.nodes() {
                let snapshot = match node {
                    BlockNode::Leaf { label, span } => BlockSnapshot::Leaf {
                        label: label.clone(),
                        span: [span.start(), span.end()],
                    },
                    BlockNode::Branch {
                        tag,
                        marker_span,
                        body,
                        kind,
                    } => BlockSnapshot::Branch {
                        tag: tag.clone(),
                        marker_span: [marker_span.start(), marker_span.end()],
                        kind: match kind {
                            BranchKind::Opener => "Opener",
                            BranchKind::Segment => "Segment",
                        },
                        nested_blocks: Self::extract_region(blocks, body.index()),
                    },
                };
                snapshots.push(snapshot);
            }

            snapshots
        }

        fn capture(db: &dyn crate::Db, tree: BlockTree) -> Self {
            let roots = tree
                .roots(db)
                .iter()
                .map(|root| {
                    let blocks = Self::extract_region(tree.blocks(db), root.index());
                    RootSnapshot {
                        root_idx: root.index(),
                        blocks,
                    }
                })
                .collect();

            Self { roots }
        }
    }

    #[test]
    fn test_block_tree_building() {
        let db = TestDatabase::new();
        let content = r"
{% block header %}
    <h1>{{ title }}</h1>
{% endblock %}

<main>
    {% if user.is_authenticated %}
        <p>Welcome {{ user.name }}!</p>
    {% else %}
        <p>Please log in</p>
    {% endif %}

    {% for item in items %}
        <div>{{ item }}</div>
    {% empty %}
        <div>No items</div>
    {% endfor %}
</main>

{% comment %}
    This is a comment block that should be captured
{% endcomment %}
";

        db.add_file("test.html", content);
        let file = File::new(&db, "test.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");

        let block_tree = build_block_tree(&db, nodelist);
        assert_yaml_snapshot!(TreeSnapshot::capture(&db, block_tree));
    }
}