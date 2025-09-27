pub(crate) mod args;
pub(crate) mod forest;

use crate::blocks::BlockTree;
use crate::blocks::BlockTreeInner;
use crate::traits::SemanticModel;
use crate::Db;
pub(crate) use args::validate_block_tags;
pub(crate) use args::validate_non_block_tags;
pub(crate) use forest::ForestBuilder;
pub(crate) use forest::SemanticForest;
pub use forest::SemanticForestInner;

#[salsa::tracked]
pub fn build_semantic_forest<'db>(
    db: &'db dyn Db,
    tree: BlockTree<'db>,
    nodelist: djls_templates::NodeList<'db>,
) -> SemanticForest<'db> {
    let tree_inner = BlockTreeInner {
        roots: tree.roots(db).to_vec(),
        blocks: tree.blocks(db).clone(),
    };
    let builder = ForestBuilder::new(tree_inner);
    let inner = builder.model(db, nodelist);
    SemanticForest::new(db, inner)
}

#[salsa::tracked]
pub fn compute_tag_spans<'db>(
    db: &'db dyn Db,
    forest: SemanticForest<'db>,
) -> Vec<djls_source::Span> {
    forest.compute_tag_spans(db)
}

