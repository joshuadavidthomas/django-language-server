mod builder;
mod grammar;
mod snapshot;
mod tree;

use builder::BlockTreeBuilder;
pub use grammar::TagIndex;
pub(crate) use tree::BlockId;
pub(crate) use tree::BlockNode;
pub(crate) use tree::BlockTree;
pub(crate) use tree::BranchKind;

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
