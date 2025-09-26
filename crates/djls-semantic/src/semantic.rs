mod args;
mod forest;

use crate::blocks::BlockTree;
use crate::Db;
pub(crate) use args::validate_block_tags;
pub(crate) use args::validate_non_block_tags;
use forest::build_root_tag;
pub(crate) use forest::SemanticForest;
use rustc_hash::FxHashSet;

pub fn build_semantic_forest(
    db: &dyn Db,
    tree: &BlockTree,
    nodelist: djls_templates::NodeList<'_>,
) -> SemanticForest {
    let mut tag_spans = FxHashSet::default();
    let roots = tree
        .roots()
        .iter()
        .filter_map(|root| build_root_tag(db, tree, nodelist, *root, &mut tag_spans))
        .collect();

    SemanticForest { roots, tag_spans }
}
