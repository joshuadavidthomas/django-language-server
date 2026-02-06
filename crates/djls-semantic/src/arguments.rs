use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;

use crate::Db;

/// Validate arguments for all tags in the template.
///
/// Performs a single pass over the flat `NodeList`, validating each tag's arguments
/// against its `TagSpec` definition. This is independent of block structure - we validate
/// opening tags, closing tags, intermediate tags, and standalone tags all the same way.
///
/// # Parameters
/// - `db`: The Salsa database containing tag specifications
/// - `nodelist`: The parsed template `NodeList` containing all tags
pub fn validate_all_tag_arguments(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            validate_tag_arguments(db, name, bits, marker_span);
        }
    }
}

/// Validate a single tag's arguments against its `TagSpec` definition.
///
/// This is the main entry point for tag argument validation. It looks up the tag
/// in the `TagSpecs` (checking opening tags, closing tags, and intermediate tags)
/// and delegates to the appropriate validation logic.
///
/// # Parameters
/// - `db`: The Salsa database containing tag specifications
/// - `tag_name`: The name of the tag (e.g., "if", "for", "endfor", "elif")
/// - `bits`: The tokenized arguments from the tag
/// - `span`: The span of the tag for error reporting
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    // Try to find spec for: opening tag, closing tag, or intermediate tag
    if let Some(spec) = tag_specs.get(tag_name) {
        // Primary path: use extracted rules from Python AST when available
        if !spec.extracted_rules.is_empty() {
            crate::rule_evaluation::evaluate_extracted_rules(
                db,
                tag_name,
                bits,
                &spec.extracted_rules,
                span,
            );
        }
        // Empty extracted_rules = no argument validation (conservative)
    }

    // Unknown tag - no validation (could be custom tag from unloaded library)
}
