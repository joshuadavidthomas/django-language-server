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
    let container = tree.blocks().get(container_id.index());
    let mut segments = Vec::new();

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
    let block = tree.blocks().get(block_id.index());
    let mut children = Vec::new();

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
                kind: BranchKind::Opener,
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
            BlockNode::Branch {
                tag,
                marker_span,
                body,
                kind: BranchKind::Segment,
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
