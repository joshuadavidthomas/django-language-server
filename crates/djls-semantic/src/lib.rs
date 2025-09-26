mod blocks;
mod db;
mod errors;
pub mod semantic;
mod templatetags;
mod traits;

use blocks::BlockTree;
use blocks::BlockTreeBuilder;
pub use blocks::TagIndex;
pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
pub use semantic::mini_tree::SemanticNode;
pub use templatetags::django_builtin_specs;
pub use templatetags::EndTag;
pub use templatetags::TagArg;
pub use templatetags::TagSpec;
pub use templatetags::TagSpecs;
use traits::SemanticModel;

/// Validate a Django template node list and return validation errors.
///
/// This function builds a `BlockTree` from the parsed node list and, during
/// construction, accumulates semantic validation errors for issues such as:
/// - Unclosed block tags
/// - Mismatched tag pairs
/// - Orphaned intermediate tags
/// - Invalid argument counts
/// - Unmatched block names
#[salsa::tracked]
pub fn validate_nodelist(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    if nodelist.nodelist(db).is_empty() {
        return;
    }

    let block_tree = build_block_tree(db, nodelist);
    let forest = semantic::mini_tree::SemanticForest::from_block_tree(db, &block_tree, nodelist);
    semantic::args::validate_block_tags(db, &forest.roots);
    semantic::args::validate_non_block_tags(db, nodelist, &forest.tag_spans);
}

#[salsa::tracked]
fn build_block_tree(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) -> BlockTree {
    let builder = BlockTreeBuilder::new(db, db.tag_index());
    builder.model(db, nodelist)
}
