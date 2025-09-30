use djls_source::Span;
use serde::Serialize;

#[salsa::tracked]
pub struct BlockTree<'db> {
    #[returns(ref)]
    pub roots: Vec<BlockId>,
    #[returns(ref)]
    pub blocks: Blocks,
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
    fn span(&self) -> Span {
        match self {
            BlockNode::Leaf { span, .. } => *span,
            BlockNode::Branch { marker_span, .. } => *marker_span,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::Span;
    use djls_templates::parse_template;
    use djls_templates::Node;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use crate::blocks::grammar::TagIndex;
    use crate::blocks::snapshot::BlockTreeSnapshot;
    use crate::build_block_tree;
    use crate::templatetags::django_builtin_specs;
    use crate::TagSpecs;

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
        fn tag_specs(&self) -> TagSpecs {
            django_builtin_specs()
        }

        fn tag_index(&self) -> TagIndex<'_> {
            TagIndex::from_specs(self)
        }

        fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
            None
        }
    }

    #[test]
    fn test_block_tree_building() {
        let db = TestDatabase::new();

        let source = r"
{% block header %}
    <h1>Title</h1>
{% endblock header %}

{% if user.is_authenticated %}
    <p>Welcome {{ user.name }}</p>
    {% if user.is_superuser %}
        <span>Admin</span>
    {% elif user.is_staff %}
        <span>Manager</span>
    {% else %}
        <span>Regular user</span>
    {% endif %}
{% else %}
    <p>Please log in</p>
{% endif %}

{% for item in items %}
    <li>{{ item }}</li>
{% endfor %}
";

        db.add_file("test.html", source);
        let file = File::new(&db, "test.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");

        let nodelist_view = {
            #[derive(serde::Serialize)]
            struct NodeListView {
                nodes: Vec<NodeView>,
            }
            #[derive(serde::Serialize)]
            #[serde(tag = "kind")]
            enum NodeView {
                Tag {
                    name: String,
                    bits: Vec<String>,
                    span: Span,
                },
                Variable {
                    var: String,
                    filters: Vec<String>,
                    span: Span,
                },
                Comment {
                    content: String,
                    span: Span,
                },
                Text {
                    span: Span,
                },
                Error {
                    span: Span,
                    full_span: Span,
                    error: String,
                },
            }

            let nodes = nodelist
                .nodelist(&db)
                .iter()
                .map(|n| match n {
                    Node::Tag { name, bits, span } => NodeView::Tag {
                        name: name.clone(),
                        bits: bits.clone(),
                        span: *span,
                    },
                    Node::Variable { var, filters, span } => NodeView::Variable {
                        var: var.clone(),
                        filters: filters.clone(),
                        span: *span,
                    },
                    Node::Comment { content, span } => NodeView::Comment {
                        content: content.clone(),
                        span: *span,
                    },
                    Node::Text { span } => NodeView::Text { span: *span },
                    Node::Error {
                        span,
                        full_span,
                        error,
                    } => NodeView::Error {
                        span: *span,
                        full_span: *full_span,
                        error: error.to_string(),
                    },
                })
                .collect();

            NodeListView { nodes }
        };
        insta::assert_yaml_snapshot!("nodelist", nodelist_view);
        let block_tree = build_block_tree(&db, nodelist);
        insta::assert_yaml_snapshot!("blocktree", BlockTreeSnapshot::from_tree(block_tree, &db));
    }
}
