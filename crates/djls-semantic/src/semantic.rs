mod args;
mod forest;

pub(crate) use args::validate_block_tags;
pub(crate) use args::validate_non_block_tags;
use forest::build_root_tag;
pub(crate) use forest::SemanticForest;
use rustc_hash::FxHashSet;

use crate::blocks::BlockTree;
use crate::Db;

#[salsa::tracked]
pub fn build_semantic_forest<'db>(
    db: &'db dyn Db,
    tree: BlockTree<'db>,
    nodelist: djls_templates::NodeList<'db>,
) -> SemanticForest<'db> {
    let mut tag_spans_set = FxHashSet::default();
    let roots = tree
        .roots(db)
        .iter()
        .filter_map(|root| build_root_tag(db, tree, nodelist, *root, &mut tag_spans_set))
        .collect();

    let mut tag_spans: Vec<_> = tag_spans_set.into_iter().collect();
    tag_spans.sort_by_key(|span| (span.start(), span.end()));

    SemanticForest::new(db, roots, tag_spans)
}
