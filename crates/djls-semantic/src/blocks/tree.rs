use serde::Serialize;

use super::nodes::BlockId;
use super::nodes::Blocks;

#[derive(Clone, Debug, Serialize)]
pub struct BlockTree {
    roots: Vec<BlockId>,
    blocks: Blocks,
}

impl BlockTree {
    pub fn new() -> Self {
        Self {
            roots: Vec::new(),
            blocks: Blocks::default(),
        }
    }

    pub fn roots(&self) -> &Vec<BlockId> {
        &self.roots
    }

    pub fn roots_mut(self) -> Vec<BlockId> {
        self.roots
    }

    pub fn blocks(&self) -> &Blocks {
        &self.blocks
    }

    pub fn blocks_mut(self) -> Blocks {
        self.blocks
    }
}

impl Default for BlockTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::snapshot::BlockTreeSnapshot;
    use crate::{templatetags::django_builtin_specs, TagSpecs};
    use camino::Utf8Path;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;
    use std::sync::Arc;
    use std::sync::Mutex;

    impl BlockTree {
        pub fn to_snapshot(&self) -> BlockTreeSnapshot {
            BlockTreeSnapshot::from(self)
        }
    }

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
    impl crate::db::Db for TestDatabase {
        fn tag_specs(&self) -> TagSpecs {
            django_builtin_specs()
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
                        name: name.text(&db).to_string(),
                        bits: bits.iter().map(|b| b.text(&db).to_string()).collect(),
                        span: *span,
                    },
                    Node::Variable { var, filters, span } => NodeView::Variable {
                        var: var.text(&db).to_string(),
                        filters: filters.iter().map(|f| f.text(&db).to_string()).collect(),
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
        let tag_shapes = TagShapes::from(&db.tag_specs());
        let block_tree = BlockTree::build(&db, nodelist, &tag_shapes);
        insta::assert_yaml_snapshot!("blocktree", block_tree.to_snapshot());
    }
}
