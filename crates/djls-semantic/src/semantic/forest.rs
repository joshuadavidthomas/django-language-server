use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::blocks::BlockId;
use crate::blocks::BlockNode;
use crate::blocks::BlockTreeInner;
use crate::blocks::BranchKind;
use crate::traits::SemanticModel;
use crate::Db;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[doc(hidden)]
pub struct SemanticForestInner {
    pub roots: Vec<SemanticNode>,
}

#[salsa::tracked]
pub struct SemanticForest<'db> {
    #[returns(ref)]
    inner: SemanticForestInner,
}

impl<'db> SemanticForest<'db> {
    pub fn roots(self, db: &'db dyn Db) -> &'db [SemanticNode] {
        &self.inner(db).roots
    }

    pub fn compute_tag_spans(self, db: &'db dyn Db) -> Vec<Span> {
        compute_tag_spans(&self.inner(db).roots)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SemanticSegment {
    pub kind: SegmentKind,
    pub marker_span: Span,
    pub content_span: Span,
    pub arguments: Vec<String>,
    pub children: Vec<SemanticNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum SegmentKind {
    Main,
    Intermediate { tag: String },
}

type ArgumentIndex = FxHashMap<Span, Vec<String>>;

pub(crate) struct ForestBuilder {
    tree_inner: BlockTreeInner,
    arg_index: ArgumentIndex,
}

impl SemanticModel<'_> for ForestBuilder {
    type Model = SemanticForestInner;

    fn observe(&mut self, node: Node) {
        if let Node::Tag { bits, span, .. } = node {
            self.arg_index.insert(span, bits);
        }
    }

    fn construct(self) -> Self::Model {
        let roots = self.build_roots();
        SemanticForestInner { roots }
    }
}

impl ForestBuilder {
    pub fn new(tree_inner: BlockTreeInner) -> Self {
        Self {
            tree_inner,
            arg_index: ArgumentIndex::default(),
        }
    }

    fn build_roots(&self) -> Vec<SemanticNode> {
        self.tree_inner
            .roots
            .iter()
            .filter_map(|root| self.build_root_tag(*root))
            .collect()
    }

    fn build_root_tag(&self, container_id: BlockId) -> Option<SemanticNode> {
        let container = self.tree_inner.blocks.get(container_id.index());
        for node in container.nodes() {
            if let BlockNode::Branch {
                tag,
                marker_span,
                kind: BranchKind::Segment,
                ..
            } = node
            {
                return Some(self.build_tag_from_container(
                    container_id,
                    tag.clone(),
                    *marker_span,
                ));
            }
        }
        None
    }

    fn build_tag_from_container(
        &self,
        container_id: BlockId,
        tag_name: String,
        opener_marker_span: Span,
    ) -> SemanticNode {
        let segments = self.build_segments(container_id, opener_marker_span);
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
        &self,
        container_id: BlockId,
        opener_marker_span: Span,
    ) -> Vec<SemanticSegment> {
        let mut segments = Vec::new();
        let container = self.tree_inner.blocks.get(container_id.index());

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

                let content_block = self.tree_inner.blocks.get(body.index());
                let arguments = self.lookup_arguments(marker);
                let children = self.build_children(*body);

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

    fn build_children(&self, block_id: BlockId) -> Vec<SemanticNode> {
        let mut children = Vec::new();
        let block = self.tree_inner.blocks.get(block_id.index());

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
                    children.push(self.build_tag_from_container(*body, tag.clone(), *marker_span));
                }
            }
        }

        children
    }

    fn lookup_arguments(&self, marker_span: Span) -> Vec<String> {
        self.arg_index
            .get(&marker_span)
            .cloned()
            .unwrap_or_default()
    }
}

#[must_use]
pub fn compute_tag_spans(roots: &[SemanticNode]) -> Vec<Span> {
    let mut spans = Vec::new();
    collect_spans_from_roots(roots, &mut spans);
    spans.sort_unstable_by_key(|s| (s.start(), s.end()));
    spans.dedup();
    spans
}

fn collect_spans_from_roots(roots: &[SemanticNode], spans: &mut Vec<Span>) {
    for node in roots {
        collect_spans_from_node(node, spans);
    }
}

fn collect_spans_from_node(node: &SemanticNode, spans: &mut Vec<Span>) {
    match node {
        SemanticNode::Tag {
            marker_span,
            segments,
            ..
        } => {
            spans.push({
                let span = *marker_span;
                span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
            });
            for segment in segments {
                if let SegmentKind::Intermediate { .. } = segment.kind {
                    spans.push({
                        segment
                            .marker_span
                            .expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
                    });
                }
                collect_spans_from_roots(&segment.children, spans);
            }
        }
        SemanticNode::Leaf { .. } => {}
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
    use crate::blocks::build_block_tree;
    use crate::build_semantic_forest;
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

        let block_tree = build_block_tree(&db, nodelist);
        let forest = build_semantic_forest(&db, block_tree, nodelist);

        assert_yaml_snapshot!(ForestSnapshot::capture(forest, &db));
    }

    #[test]
    fn semantic_forest_intermediate_snapshot() {
        let db = TestDatabase::new();
        let source = r"
{% if user.is_staff %}
    <span>Staff</span>
{% elif user.is_manager %}
    <span>Manager</span>
{% else %}
    <span>Regular</span>
{% endif %}
";

        db.add_file("intermediate.html", source);
        let file = File::new(&db, "intermediate.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");

        let block_tree = build_block_tree(&db, nodelist);
        let forest = build_semantic_forest(&db, block_tree, nodelist);

        assert_yaml_snapshot!("intermediate", ForestSnapshot::capture(forest, &db));
    }

    #[test]
    fn test_pure_forest_operations() {
        // Create a forest without any database
        let inner = SemanticForestInner {
            roots: vec![SemanticNode::Tag {
                name: "if".into(),
                marker_span: Span::new(0, 10),
                arguments: vec!["user.is_authenticated".into()],
                segments: vec![SemanticSegment {
                    kind: SegmentKind::Main,
                    marker_span: Span::new(0, 10),
                    content_span: Span::new(10, 50),
                    arguments: vec!["user.is_authenticated".into()],
                    children: vec![SemanticNode::Leaf {
                        label: "text".into(),
                        span: Span::new(15, 45),
                    }],
                }],
            }],
        };

        // Test compute_tag_spans without db
        let spans = compute_tag_spans(&inner.roots);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0], Span::new(0, 10).expand(2, 2));

        // Test pure validation without db
        let specs = django_builtin_specs();
        let errors = super::super::args::validate_block_tags_pure(&specs, &inner.roots);
        // The if tag should validate successfully with one argument
        assert!(errors.is_empty());
    }

    #[derive(Serialize)]
    struct ForestSnapshot {
        roots: Vec<SemanticNode>,
        tag_spans: Vec<Span>,
    }

    impl ForestSnapshot {
        fn capture(forest: SemanticForest, db: &dyn crate::Db) -> Self {
            let roots = forest.roots(db).to_vec();
            let tag_spans = compute_tag_spans(&roots);

            Self { roots, tag_spans }
        }
    }
}

