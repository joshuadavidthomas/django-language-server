mod arguments;
mod blocks;
mod db;
mod errors;
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
pub use templatetags::TagArg;
pub use templatetags::TagSpec;
pub use templatetags::TagSpecs;

/// Validate a Django template node list.
///
/// This performs two types of validation:
///
/// 1. **Block Structure Validation** (via `build_block_tree`):
///    - Unclosed block tags
///    - Mismatched tag pairs
///    - Orphaned intermediate tags
///    - Unmatched block names
///
/// 2. **Argument Validation** (via `validate_all_tag_arguments`):
///    - Missing required arguments
///    - Too many arguments
///    - Invalid argument values (choices, literals)
///
/// These are independent concerns: structure validation happens during tree building,
/// while argument validation is a simple pass over all tags regardless of structure.
#[salsa::tracked]
pub fn validate_nodelist(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    if nodelist.nodelist(db).is_empty() {
        return;
    }

    // Validate block structure (unclosed tags, mismatched pairs, etc.)
    let block_tree = build_block_tree(db, nodelist);

    // Build semantic forest (for IDE features, code navigation, etc.)
    let _forest = build_semantic_forest(db, block_tree, nodelist);

    // Validate tag arguments (independent of structure)
    validate_all_tag_arguments(db, nodelist);
}
