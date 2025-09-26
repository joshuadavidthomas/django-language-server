use std::collections::HashSet;

use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;

use crate::blocks::BlockId;
use crate::blocks::BlockNode;
use crate::blocks::BlockTree;
use crate::blocks::BranchKind;
use crate::Db;

#[derive(Debug, Clone)]
pub struct SemanticForest {
    pub roots: Vec<SemanticNode>,
    pub tag_spans: HashSet<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub enum SemanticNode {
    Tag {
        name: String,
        marker_span: Span,
        arguments: Vec<String>,
        segments: Vec<SemanticSegment>,
    },
    Leaf {
        label: String,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct SemanticSegment {
    pub kind: SegmentKind,
    pub marker_span: Span,
    pub content_span: Span,
    pub arguments: Vec<String>,
    pub children: Vec<SemanticNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentKind {
    Main,
    Intermediate { tag: String },
}

impl SemanticForest {
    #[must_use]
    pub fn from_block_tree(
        db: &dyn Db,
        tree: &BlockTree,
        nodelist: djls_templates::NodeList<'_>,
    ) -> Self {
        let mut tag_spans = HashSet::new();
        let roots = tree
            .roots()
            .iter()
            .filter_map(|root| build_root_tag(db, tree, nodelist, *root, &mut tag_spans))
            .collect();

        SemanticForest { roots, tag_spans }
    }
}

fn build_root_tag(
    db: &dyn Db,
    tree: &BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    container_id: BlockId,
    spans: &mut HashSet<(u32, u32)>,
) -> Option<SemanticNode> {
    let container = tree.blocks().get(container_id.index());
    for node in container.nodes() {
        if let BlockNode::Branch {
            tag,
            marker_span,
            kind: BranchKind::Segment,
            ..
        } = node
        {
            spans.insert(span_key(expand_marker(*marker_span)));
            return Some(build_tag_from_container(
                db,
                tree,
                nodelist,
                container_id,
                tag.clone(),
                *marker_span,
                spans,
            ));
        }
    }
    None
}

fn build_tag_from_container(
    db: &dyn Db,
    tree: &BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    container_id: BlockId,
    tag_name: String,
    opener_marker_span: Span,
    spans: &mut HashSet<(u32, u32)>,
) -> SemanticNode {
    let segments = build_segments(db, tree, nodelist, container_id, opener_marker_span, spans);
    let arguments = segments
        .first()
        .map(|segment| segment.arguments.clone())
        .unwrap_or_default();

    SemanticNode::Tag {
        name: tag_name,
        marker_span: opener_marker_span,
        arguments,
        segments,
    }
}

fn build_segments(
    db: &dyn Db,
    tree: &BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    container_id: BlockId,
    opener_marker_span: Span,
    spans: &mut HashSet<(u32, u32)>,
) -> Vec<SemanticSegment> {
    let mut segments = Vec::new();
    let container = tree.blocks().get(container_id.index());

    for (idx, node) in container.nodes().iter().enumerate() {
        if let BlockNode::Branch {
            tag,
            marker_span,
            body,
            kind: BranchKind::Segment,
        } = node
        {
            let kind = if idx == 0 {
                SegmentKind::Main
            } else {
                SegmentKind::Intermediate { tag: tag.clone() }
            };

            let marker = if idx == 0 {
                opener_marker_span
            } else {
                *marker_span
            };

            spans.insert(span_key(expand_marker(marker)));

            let content_block = tree.blocks().get(body.index());
            let arguments = lookup_arguments(db, nodelist, marker);
            let children = build_children(db, tree, nodelist, *body, spans);

            segments.push(SemanticSegment {
                kind,
                marker_span: marker,
                content_span: *content_block.span(),
                arguments,
                children,
            });
        }
    }

    segments
}

fn build_children(
    db: &dyn Db,
    tree: &BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    block_id: BlockId,
    spans: &mut HashSet<(u32, u32)>,
) -> Vec<SemanticNode> {
    let mut children = Vec::new();
    let block = tree.blocks().get(block_id.index());

    for node in block.nodes() {
        match node {
            BlockNode::Leaf { label, span } => {
                children.push(SemanticNode::Leaf {
                    label: label.clone(),
                    span: *span,
                });
            }
            BlockNode::Branch {
                tag,
                marker_span,
                body,
                kind: BranchKind::Opener | BranchKind::Segment,
            } => {
                spans.insert(span_key(expand_marker(*marker_span)));
                children.push(build_tag_from_container(
                    db,
                    tree,
                    nodelist,
                    *body,
                    tag.clone(),
                    *marker_span,
                    spans,
                ));
            }
        }
    }

    children
}

fn lookup_arguments(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    marker_span: Span,
) -> Vec<String> {
    nodelist
        .nodelist(db)
        .iter()
        .find_map(|node| match node {
            Node::Tag { bits, span, .. } if *span == marker_span => Some(bits.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn span_key(span: Span) -> (u32, u32) {
    (span.start(), span.end())
}

fn expand_marker(span: Span) -> Span {
    span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
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
    use insta::assert_debug_snapshot;

    use super::*;
    use crate::blocks::BlockTreeBuilder;
    use crate::templatetags::django_builtin_specs;
    use crate::traits::SemanticModel;
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

        fn tag_index(&self) -> TagIndex<'_> {
            TagIndex::from_specs(self)
        }
    }

    #[test]
    fn semantic_forest_snapshot() {
        let db = TestDatabase::new();
        let source = r"
{% block header %}
    <h1>Title</h1>
{% endblock header %}

{% if user.is_authenticated %}
    {% for item in items %}
        <li>{{ item }}</li>
    {% empty %}
        <li>No items</li>
    {% endfor %}
{% else %}
    <p>Please log in</p>
{% endif %}
";

        db.add_file("template.html", source);
        let file = File::new(&db, "template.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");

        let builder = BlockTreeBuilder::new(&db, db.tag_index());
        let block_tree = builder.model(&db, nodelist);
        let forest = SemanticForest::from_block_tree(&db, &block_tree, nodelist);

        assert_debug_snapshot!(normalize_forest(&forest));
    }

    fn normalize_forest(forest: &SemanticForest) -> (Vec<String>, Vec<(u32, u32)>) {
        let mut spans: Vec<_> = forest.tag_spans.iter().copied().collect();
        spans.sort_unstable();

        let mut nodes = Vec::new();
        for node in &forest.roots {
            nodes.push(format_node(node));
        }

        (nodes, spans)
    }

    fn format_node(node: &SemanticNode) -> String {
        match node {
            SemanticNode::Tag {
                name,
                marker_span,
                arguments,
                segments,
            } => format!(
                "Tag(name={name}, span={:?}, args={arguments:?}, segments={:?})",
                marker_span, segments
            ),
            SemanticNode::Leaf { label, span } => {
                format!("Leaf(label={label}, span={:?})", span)
            }
        }
    }
}
