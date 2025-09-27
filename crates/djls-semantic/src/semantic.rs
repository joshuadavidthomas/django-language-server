pub(crate) mod args;
pub(crate) mod forest;

use crate::blocks::BlockTree;
use crate::Db;
pub(crate) use forest::ForestBuilder;
pub(crate) use forest::SemanticForest;
pub use forest::SemanticForestInner;

// These functions are now replaced by the ones in queries.rs
// Keep them for backward compatibility but mark as deprecated

#[deprecated(note = "Use queries::build_semantic_forest instead")]
#[salsa::tracked]
pub fn build_semantic_forest<'db>(
    db: &'db dyn Db,
    tree: BlockTree<'db>,
    nodelist: djls_templates::NodeList<'db>,
) -> SemanticForest<'db> {
    // Just forward to the new query
    crate::queries::build_semantic_forest(db, nodelist)
}

#[deprecated(note = "Use queries::compute_tag_spans instead")]
#[salsa::tracked]
pub fn compute_tag_spans<'db>(
    db: &'db dyn Db,
    forest: SemanticForest<'db>,
) -> Vec<djls_source::Span> {
    forest.compute_tag_spans(db)
}

