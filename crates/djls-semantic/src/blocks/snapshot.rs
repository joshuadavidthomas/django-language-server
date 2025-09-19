use djls_source::Span;
use serde::Serialize;
use std::collections::HashSet;

use super::nodes::{BlockId, BlockNode, BranchKind};
use super::tree::BlockTree;

#[derive(Serialize)]
pub struct BlockTreeSnapshot {
    roots: Vec<u32>,
    root_ids: Vec<u32>,
    blocks: Vec<BlockSnapshot>,
}

impl From<&BlockTree> for BlockTreeSnapshot {
    #[allow(clippy::too_many_lines)]
    fn from(tree: &BlockTree) -> Self {
        let mut container_ids: HashSet<u32> = HashSet::new();
        let mut body_ids: HashSet<u32> = HashSet::new();

        for r in tree.roots() {
            container_ids.insert(r.id());
        }
        for (i, b) in tree.blocks().into_iter().enumerate() {
            let i_u = u32::try_from(i).unwrap_or(u32::MAX);
            for n in b.nodes() {
                match n {
                    BlockNode::Leaf { .. } | BlockNode::Error { .. } => {}
                    BlockNode::Branch {
                        body,
                        kind: BranchKind::Opener,
                        ..
                    } => {
                        container_ids.insert(body.id());
                    }
                    BlockNode::Branch {
                        body,
                        kind: BranchKind::Segment,
                        ..
                    } => {
                        body_ids.insert(body.id());
                    }
                }
            }
            if container_ids.contains(&i_u) {
                body_ids.remove(&i_u);
            }
        }

        let blocks = tree
            .blocks()
            .into_iter()
            .enumerate()
            .map(|(i, b)| {
                let id_u = u32::try_from(i).unwrap_or(u32::MAX);
                let nodes: Vec<BlockNodeSnapshot> = b
                    .nodes()
                    .iter()
                    .map(|n| match n {
                        BlockNode::Leaf { label, span } => BlockNodeSnapshot::Leaf {
                            label: label.clone(),
                            span: *span,
                        },
                        BlockNode::Error { message, span } => BlockNodeSnapshot::Error {
                            message: message.clone(),
                            span: *span,
                        },
                        BlockNode::Branch {
                            tag,
                            marker_span,
                            body,
                            ..
                        } => BlockNodeSnapshot::Branch {
                            block_id: body.id(),
                            tag: tag.clone(),
                            marker_span: *marker_span,
                            content_span: *tree.blocks().get(body.index()).span(),
                        },
                    })
                    .collect();

                if container_ids.contains(&id_u) {
                    BlockSnapshot::Container {
                        container_span: *b.span(),
                        nodes,
                    }
                } else {
                    BlockSnapshot::Body {
                        content_span: *b.span(),
                        nodes,
                    }
                }
            })
            .collect();

        // Also compute root_id for every block/region
        let root_ids: Vec<u32> = tree
            .blocks()
            .into_iter()
            .enumerate()
            .map(|(i, _)| {
                let mut cur = BlockId::new(u32::try_from(i).unwrap_or(u32::MAX));
                // climb via snapshot-internal parent pointers
                loop {
                    // safety: we have no direct parent access in snapshot; infer by scanning containers
                    // If any Branch points to `cur` as body, that region's parent is its container id
                    let mut parent: Option<BlockId> = None;
                    for (j, b) in tree.blocks().into_iter().enumerate() {
                        for n in b.nodes() {
                            if let BlockNode::Branch { body, .. } = n {
                                if body.index() == cur.index() {
                                    parent =
                                        Some(BlockId::new(u32::try_from(j).unwrap_or(u32::MAX)));
                                    break;
                                }
                            }
                        }
                        if parent.is_some() {
                            break;
                        }
                    }
                    if let Some(p) = parent {
                        cur = p;
                    } else {
                        break cur.id();
                    }
                }
            })
            .collect();

        Self {
            roots: tree.roots().iter().map(|r| r.id()).collect(),
            blocks,
            root_ids,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind")]
pub enum BlockSnapshot {
    Container {
        container_span: Span,
        nodes: Vec<BlockNodeSnapshot>,
    },
    Body {
        content_span: Span,
        nodes: Vec<BlockNodeSnapshot>,
    },
}

#[derive(Serialize)]
#[serde(tag = "node")]
pub enum BlockNodeSnapshot {
    Branch {
        block_id: u32,
        tag: String,
        marker_span: Span,
        content_span: Span,
    },
    Leaf {
        label: String,
        span: Span,
    },
    Error {
        message: String,
        span: Span,
    },
}
