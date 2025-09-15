use std::sync::Arc;

use djls_templates::Db as TemplateDb;
use djls_templates::TemplateDiagnostic;
use djls_workspace::Db as WorkspaceDb;
use salsa::Accumulator;
use tower_lsp_server::lsp_types;

use crate::specs::TagSpecs;
use crate::validation::TagValidator;

/// Semantic database trait extending the template and workspace databases
#[salsa::db]
pub trait SemanticDb: TemplateDb + WorkspaceDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> Arc<TagSpecs>;
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
pub fn validate_nodelist(db: &dyn SemanticDb, ast: djls_templates::NodeList<'_>) -> Vec<djls_templates::nodelist::NodeListError> {
    // Skip validation if node list is empty (likely due to parse errors)
    if ast.nodelist(db).is_empty() {
        return vec![];
    }

    // Run semantic validation and return errors
    TagValidator::new(db, ast).validate()
}

/// Validate a template file and accumulate diagnostics.
///
/// This is the entry point for semantic validation that handles
/// the file -> node list -> validation -> diagnostic accumulation pipeline.
#[salsa::tracked]
pub fn validate_template(db: &dyn SemanticDb, file: djls_workspace::SourceFile) {
    // Only validate template files
    if file.kind(db) != djls_workspace::FileKind::Template {
        return;
    }

    // Get the parsed node list from templates crate
    let Some(ast) = djls_templates::parse_template(db, file) else {
        return;
    };

    // Run semantic validation on the node list
    let validation_errors = validate_nodelist(db, ast);

    // Convert validation errors to diagnostics and accumulate
    for error in validation_errors {
        let code = error.diagnostic_code();
        let line_offsets = ast.line_offsets(db);

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

        TemplateDiagnostic(diagnostic).accumulate(db);
    }
}
