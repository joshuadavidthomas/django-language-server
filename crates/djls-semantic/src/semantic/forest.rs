use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use rustc_hash::FxHashSet;
use serde::Serialize;

use crate::blocks::BlockId;
use crate::blocks::BlockNode;
use crate::blocks::BlockTree;
use crate::blocks::BranchKind;
use crate::Db;

#[salsa::tracked]
pub struct SemanticForest<'db> {
    #[returns(ref)]
    pub roots: Vec<SemanticNode>,
    #[returns(ref)]
    pub tag_spans: Vec<Span>,
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

pub fn build_root_tag(
    db: &dyn Db,
    tree: BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    container_id: BlockId,
    spans: &mut FxHashSet<Span>,
) -> Option<SemanticNode> {
    let container = tree.blocks(db).get(container_id.index());
    for node in container.nodes() {
        if let BlockNode::Branch {
            tag,
            marker_span,
            kind: BranchKind::Segment,
            ..
        } = node
        {
            spans.insert(expand_marker(*marker_span));
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
    tree: BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    container_id: BlockId,
    tag_name: String,
    opener_marker_span: Span,
    spans: &mut FxHashSet<Span>,
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
    tree: BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    container_id: BlockId,
    opener_marker_span: Span,
    spans: &mut FxHashSet<Span>,
) -> Vec<SemanticSegment> {
    let mut segments = Vec::new();
    let container = tree.blocks(db).get(container_id.index());

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

            spans.insert(expand_marker(marker));

            let content_block = tree.blocks(db).get(body.index());
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
    tree: BlockTree,
    nodelist: djls_templates::NodeList<'_>,
    block_id: BlockId,
    spans: &mut FxHashSet<Span>,
) -> Vec<SemanticNode> {
    let mut children = Vec::new();
    let block = tree.blocks(db).get(block_id.index());

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
                spans.insert(expand_marker(*marker_span));
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

fn expand_marker(span: Span) -> Span {
    span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
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

        fn tag_index(&self) -> TagIndex<'_> {
            TagIndex::from_specs(self)
        }

        fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
            None
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

    #[derive(Serialize)]
    struct ForestSnapshot {
        roots: Vec<SemanticNode>,
        tag_spans: Vec<Span>,
    }

    impl ForestSnapshot {
        fn capture(forest: SemanticForest, db: &dyn crate::Db) -> Self {
            let mut tag_spans = forest.tag_spans(db).clone();
            tag_spans.sort_by_key(|span| (span.start(), span.end()));

            Self {
                roots: forest.roots(db).clone(),
                tag_spans,
            }
        }
    }
}
