use std::sync::Arc;

use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;

use crate::specs::TagSpecs;

/// Semantic database trait extending the template and workspace databases
#[salsa::db]
pub trait SemanticDb: TemplateDb + WorkspaceDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> Arc<TagSpecs>;
}