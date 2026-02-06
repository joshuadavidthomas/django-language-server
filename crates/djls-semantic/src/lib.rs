mod arguments;
mod blocks;
mod db;
mod errors;
mod filter_arity;
mod if_expression;
mod load_resolution;
mod opaque;
mod primitives;
mod resolution;
mod semantic;
mod templatetags;
mod traits;

use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use salsa::Accumulator;

use arguments::validate_all_tag_arguments;
pub use blocks::build_block_tree;
pub use blocks::TagIndex;
pub use db::Db;
pub use db::FilterAritySpecs;
pub use db::OpaqueTagMap;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
pub use filter_arity::validate_filter_arity;
pub use if_expression::validate_if_expression;
pub use load_resolution::available_filters_at;
pub use load_resolution::available_tags_at;
pub use load_resolution::compute_loaded_libraries;
pub use load_resolution::parse_load_bits;
pub use load_resolution::validate_filter_scoping;
pub use load_resolution::validate_tag_scoping;
pub use load_resolution::AvailableFilters;
pub use load_resolution::AvailableSymbols;
pub use load_resolution::LoadKind;
pub use load_resolution::LoadStatement;
pub use load_resolution::LoadedLibraries;
pub use opaque::compute_opaque_regions;
pub use opaque::OpaqueRegions;
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
/// - Unknown/unloaded tags and filters (S108-S113)
/// - Invalid `{% if %}`/`{% elif %}` expressions (S114)
/// - Filter arity mismatches (S115/S116)
///
/// Opaque regions (e.g., `{% verbatim %}...{% endverbatim %}`) are computed
/// first and passed to all validation passes so nodes inside them are skipped.
#[salsa::tracked]
pub fn validate_nodelist(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    if nodelist.nodelist(db).is_empty() {
        return;
    }

    // Opaque regions (e.g., {% verbatim %}...{% endverbatim %}) are computed
    // inside each validation pass via compute_opaque_regions(). Each pass
    // independently skips nodes inside opaque regions to avoid false positives.

    let block_tree = build_block_tree(db, nodelist);
    let _forest = build_semantic_forest(db, block_tree, nodelist);
    validate_all_tag_arguments(db, nodelist);
    load_resolution::validate_tag_scoping(db, nodelist);
    load_resolution::validate_filter_scoping(db, nodelist);

    // M6: Expression validation
    validate_if_expressions(db, nodelist);

    // M6: Filter arity validation
    filter_arity::validate_filter_arity(db, nodelist);
}

/// Validate `{% if %}`/`{% elif %}` expression syntax.
///
/// Uses the Pratt parser from `if_expression` module to catch compile-time
/// expression syntax errors (misplaced operators, dangling operators, etc.).
fn validate_if_expressions(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    let opaque = compute_opaque_regions(db, nodelist);
    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            if opaque.is_opaque(*span) {
                continue;
            }

            if name == "if" || name == "elif" {
                if let Some(message) = validate_if_expression(bits) {
                    let marker_span =
                        span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
                    ValidationErrorAccumulator(ValidationError::ExpressionSyntaxError {
                        tag: name.to_string(),
                        message,
                        span: marker_span,
                    })
                    .accumulate(db);
                }
            }
        }
    }
}
