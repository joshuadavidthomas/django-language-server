use std::collections::HashSet;

use djls_source::Span;
use serde::Serialize;

use super::tree::BlockId;
use super::tree::BlockNode;
use super::tree::BlockTree;
use super::tree::BranchKind;

// TODO: centralize salsa struct snapshots so this mess can be shared

#[derive(Serialize)]
#[allow(dead_code)]
pub struct BlockTreeSnapshot {
    roots: Vec<u32>,
    root_ids: Vec<u32>,
    blocks: Vec<BlockSnapshot>,
}

impl BlockTreeSnapshot {
    #[allow(clippy::too_many_lines)]
    #[allow(dead_code)]
    pub fn from_tree(tree: BlockTree<'_>, db: &dyn crate::Db) -> Self {
        let roots = tree.roots(db);
        let blocks_ref = tree.blocks(db);

        let mut container_ids: HashSet<u32> = HashSet::new();
        let mut body_ids: HashSet<u32> = HashSet::new();

        for r in roots {
            container_ids.insert(r.id());
        }

        for (i, region) in blocks_ref.into_iter().enumerate() {
            let i_u = u32::try_from(i).unwrap_or(u32::MAX);
            for node in region.nodes() {
                match node {
                    BlockNode::Leaf { .. } => {}
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

        let blocks: Vec<BlockSnapshot> = blocks_ref
            .into_iter()
            .enumerate()
            .map(|(i, region)| {
                let id_u = u32::try_from(i).unwrap_or(u32::MAX);
                let nodes: Vec<BlockNodeSnapshot> = region
                    .nodes()
                    .iter()
                    .map(|node| match node {
                        BlockNode::Leaf { label, span } => BlockNodeSnapshot::Leaf {
                            label: label.clone(),
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
                            content_span: *blocks_ref.get(body.index()).span(),
                        },
                    })
                    .collect();

                if container_ids.contains(&id_u) {
                    BlockSnapshot::Container {
                        container_span: *region.span(),
                        nodes,
                    }
                } else {
                    BlockSnapshot::Body {
                        content_span: *region.span(),
                        nodes,
                    }
                }
            })
            .collect();

        let root_ids: Vec<u32> = blocks_ref
            .into_iter()
            .enumerate()
            .map(|(i, _)| {
                let mut cur = BlockId::new(u32::try_from(i).unwrap_or(u32::MAX));
                loop {
                    let mut parent: Option<BlockId> = None;
                    for (j, region) in blocks_ref.into_iter().enumerate() {
                        for node in region.nodes() {
                            if let BlockNode::Branch { body, .. } = node {
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
            roots: roots.iter().map(|r| r.id()).collect(),
            blocks,
            root_ids,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind")]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
}
