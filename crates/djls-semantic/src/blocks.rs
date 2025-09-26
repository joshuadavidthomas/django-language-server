mod builder;
mod grammar;
mod snapshot;
mod tree;

use crate::db::Db;
use crate::traits::SemanticModel;
pub use builder::BlockTreeBuilder;
pub use grammar::TagIndex;
pub use tree::BlockId;
pub use tree::BlockNode;
pub use tree::BlockTree;
pub use tree::BranchKind;

#[salsa::tracked]
pub fn build_block_tree(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) -> BlockTree {
    let builder = BlockTreeBuilder::new(db, db.tag_index());
    builder.model(db, nodelist)
}
