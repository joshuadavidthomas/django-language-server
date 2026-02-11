pub mod builder;
pub mod forest;
pub mod grammar;
pub mod opaque;
pub mod snapshot;
pub mod tree;

pub use builder::BlockTreeBuilder;
pub use forest::build_semantic_forest;
pub use grammar::TagIndex;
pub use opaque::compute_opaque_regions;
pub use opaque::OpaqueRegions;
pub(crate) use tree::BlockId;
pub(crate) use tree::BlockNode;
pub(crate) use tree::BlockTree;
pub(crate) use tree::Blocks;
pub(crate) use tree::BranchKind;
pub(crate) use tree::Region;

use crate::db::Db;
use crate::traits::SemanticModel;

#[salsa::tracked]
pub fn build_block_tree<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> BlockTree<'db> {
    let builder = BlockTreeBuilder::new(db, db.tag_index());
    builder.model(db, nodelist)
}
