use std::sync::Arc;

use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use tower_lsp_server::lsp_types;

use crate::specs::TagSpecs;

#[salsa::db]
pub trait SemanticDb: TemplateDb + WorkspaceDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> Arc<TagSpecs>;
}

#[salsa::accumulator]
pub struct SemanticDiagnostic(pub lsp_types::Diagnostic);

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
