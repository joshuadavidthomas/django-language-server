mod blocks;
mod db;
mod errors;
mod semantic;
mod templatetags;
mod traits;

pub use blocks::build_block_tree;
pub use blocks::TagIndex;
pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
pub use semantic::build_semantic_forest;
pub use semantic::compute_tag_spans;
use semantic::validate_block_tags;
use semantic::validate_non_block_tags;
pub use templatetags::django_builtin_specs;
pub use templatetags::EndTag;
pub use templatetags::TagArg;
pub use templatetags::TagSpec;
pub use templatetags::TagSpecs;

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
    let forest = build_semantic_forest(db, block_tree, nodelist);
    let tag_spans = compute_tag_spans(db, forest);
    validate_block_tags(db, forest.roots(db));
    validate_non_block_tags(db, nodelist, &tag_spans);
}

// Minimal API for benchmarking without database
#[doc(hidden)]
pub fn build_forest_from_parts(
    tree_inner: blocks::BlockTreeInner,
    nodes: &[djls_templates::Node],
) -> semantic::SemanticForestInner {
    use traits::SemanticModel;
    
    let mut builder = semantic::ForestBuilder::new(tree_inner);
    for node in nodes {
        builder.observe(node.clone());
    }
    builder.construct()
}

// Export pure block tree builder for benchmarks
#[doc(hidden)]
pub use blocks::build_block_tree_from_parts;

// Export inner types for benchmarks
#[doc(hidden)]
pub use blocks::BlockTreeInner;
#[doc(hidden)]
pub use semantic::SemanticForestInner;

// Export pure validation functions for benchmarks
#[doc(hidden)]
pub mod benchmark_helpers {
    pub use crate::semantic::args::validate_block_tags_pure;
    pub use crate::semantic::args::validate_non_block_tags_pure;
    pub use crate::semantic::forest::compute_tag_spans;
}