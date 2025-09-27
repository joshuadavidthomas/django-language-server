mod builder;
mod grammar;
mod snapshot;
mod tree;

use builder::BlockTreeBuilder;
pub use grammar::TagIndex;
use salsa::Accumulator;
pub(crate) use tree::BlockId;
pub(crate) use tree::BlockNode;
pub(crate) use tree::BlockTree;
pub use tree::BlockTreeInner;
pub(crate) use tree::BranchKind;

use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
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

#[salsa::tracked]
pub fn build_block_tree<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> BlockTree<'db> {
    // Extract pure data
    let nodes = nodelist.nodelist(db).to_vec();
    let specs = db.tag_specs();
    
    // Build using pure function
    let (inner, errors) = build_block_tree_from_parts(&specs, &nodes);
    
    // Accumulate errors at the edge
    for error in errors {
        ValidationErrorAccumulator(error).accumulate(db);
    }
    
    BlockTree::new(db, inner)
}