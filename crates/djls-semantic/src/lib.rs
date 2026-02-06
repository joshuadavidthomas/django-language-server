mod arguments;
mod blocks;
mod db;
mod errors;
mod load_resolution;
mod primitives;
mod resolution;
mod semantic;
mod templatetags;
mod traits;

use arguments::validate_all_tag_arguments;
pub use blocks::build_block_tree;
pub use blocks::TagIndex;
pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
pub use load_resolution::available_filters_at;
pub use load_resolution::available_tags_at;
pub use load_resolution::compute_loaded_libraries;
pub use load_resolution::parse_load_bits;
pub use load_resolution::validate_tag_scoping;
pub use load_resolution::AvailableFilters;
pub use load_resolution::AvailableSymbols;
pub use load_resolution::LoadKind;
pub use load_resolution::LoadStatement;
pub use load_resolution::LoadedLibraries;
pub use primitives::Tag;
pub use primitives::Template;
pub use primitives::TemplateName;
pub use resolution::find_references_to_template;
pub use resolution::resolve_template;
pub use resolution::ResolveResult;
pub use resolution::TemplateReference;
pub use semantic::build_semantic_forest;
pub use templatetags::django_builtin_specs;
pub use templatetags::EndTag;
pub use templatetags::LiteralKind;
pub use templatetags::TagArg;
pub use templatetags::TagSpec;
pub use templatetags::TagSpecs;
pub use templatetags::TokenCount;

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
    let _forest = build_semantic_forest(db, block_tree, nodelist);
    validate_all_tag_arguments(db, nodelist);
    load_resolution::validate_tag_scoping(db, nodelist);
}
