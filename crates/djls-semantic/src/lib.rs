mod blocks;
mod db;
mod errors;
mod semantic;
mod templatetags;
mod traits;

use blocks::build_block_tree;
pub use blocks::TagIndex;
pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
use semantic::validate_block_tags;
use semantic::validate_non_block_tags;
pub use templatetags::EndTag;
pub use templatetags::TagArg;
pub use templatetags::TagSpec;
pub use templatetags::TagSpecs;

use crate::semantic::build_semantic_forest;

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
    let forest = build_semantic_forest(db, &block_tree, nodelist);
    validate_block_tags(db, &forest.roots);
    validate_non_block_tags(db, nodelist, &forest.tag_spans);
}
