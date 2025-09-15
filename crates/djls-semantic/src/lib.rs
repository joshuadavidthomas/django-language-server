pub mod builtins;
pub mod db;
pub mod snippets;
pub mod specs;
pub mod validation;

pub use builtins::django_builtin_specs;
pub use db::SemanticDb;
pub use db::SemanticDiagnostic;
use salsa::Accumulator;
pub use snippets::generate_partial_snippet;
pub use snippets::generate_snippet_for_tag;
pub use snippets::generate_snippet_for_tag_with_end;
pub use snippets::generate_snippet_from_args;
pub use specs::ArgType;
pub use specs::EndTag;
pub use specs::IntermediateTag;
pub use specs::SimpleArgType;
pub use specs::TagArg;
pub use specs::TagSpec;
pub use specs::TagSpecs;
use tower_lsp_server::lsp_types;
pub use validation::TagValidator;

pub enum TagType {
    Opener,
    Intermediate,
    Closer,
    Standalone,
}

impl TagType {
    #[must_use]
    pub fn for_name(name: &str, tag_specs: &TagSpecs) -> TagType {
        if tag_specs.is_opener(name) {
            TagType::Opener
        } else if tag_specs.is_closer(name) {
            TagType::Closer
        } else if tag_specs.is_intermediate(name) {
            TagType::Intermediate
        } else {
            TagType::Standalone
        }
    }
}

/// Validate a Django template node list and return validation errors.
///
/// This function runs the TagValidator on the parsed node list to check for:
/// - Unclosed block tags
/// - Mismatched tag pairs
/// - Orphaned intermediate tags
/// - Invalid argument counts
/// - Unmatched block names
#[salsa::tracked]
pub fn validate_nodelist(db: &dyn SemanticDb, nodelist: djls_templates::NodeList<'_>) {
    // Skip validation if node list is empty (likely due to parse errors)
    if nodelist.nodelist(db).is_empty() {
        return;
    }

    // Run semantic validation
    let validation_errors = TagValidator::new(db, nodelist).validate();

    // Accumulate errors as diagnostics
    let line_offsets = nodelist.line_offsets(db);
    for error in validation_errors {
        let code = error.diagnostic_code();
        let range = error
            .span()
            .map(|(start, length)| {
                let span = djls_templates::nodelist::Span::new(start, length);
                span.to_lsp_range(line_offsets)
            })
            .unwrap_or_default();

        let diagnostic = lsp_types::Diagnostic {
            range,
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::String(code.to_string())),
            code_description: None,
            source: Some("Django Language Server".to_string()),
            message: error.to_string(),
            related_information: None,
            tags: None,
            data: None,
        };

        SemanticDiagnostic(diagnostic).accumulate(db);
    }
}
