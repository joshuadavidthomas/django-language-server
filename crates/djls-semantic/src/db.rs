use std::sync::Arc;

use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use salsa::Accumulator;
use tower_lsp_server::lsp_types;

use crate::specs::TagSpecs;
use crate::validation::TagValidator;

/// Accumulator for semantic validation diagnostics
#[salsa::accumulator]
pub struct SemanticDiagnostic(pub lsp_types::Diagnostic);

/// Semantic database trait extending the template and workspace databases
#[salsa::db]
pub trait SemanticDb: TemplateDb + WorkspaceDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> Arc<TagSpecs>;
}

impl From<SemanticDiagnostic> for lsp_types::Diagnostic {
    fn from(diagnostic: SemanticDiagnostic) -> Self {
        diagnostic.0
    }
}

impl From<&SemanticDiagnostic> for lsp_types::Diagnostic {
    fn from(diagnostic: &SemanticDiagnostic) -> Self {
        diagnostic.0.clone()
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
