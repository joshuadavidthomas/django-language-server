mod builder;
mod grammar;
mod snapshot;
mod tree;

pub(crate) use builder::BlockTreeBuilder;
pub use grammar::TagIndex;
pub(crate) use tree::BlockId;
pub(crate) use tree::BlockNode;
pub(crate) use tree::BlockTree;
pub use tree::BlockTreeInner;
pub(crate) use tree::BranchKind;

use crate::db::Db;
use crate::traits::SemanticModel;

/// Build a block tree from pure data without database
#[doc(hidden)]
pub fn build_block_tree_from_parts(
    specs: &crate::TagSpecs,
    nodes: &[djls_templates::Node],
) -> (BlockTreeInner, Vec<crate::ValidationError>) {
    let index = TagIndex::from_specs(specs);
    let mut builder = BlockTreeBuilder::new(index);
    for node in nodes {
        builder.observe(node.clone());
    }
    builder.construct()
}

// This function is now replaced by the one in queries.rs
// Keep it for backward compatibility but mark as deprecated

#[deprecated(note = "Use queries::build_block_tree instead")]
#[salsa::tracked]
pub fn build_block_tree<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> BlockTree<'db> {
    // Forward to the new query
    crate::queries::build_block_tree(db, nodelist)
}