use std::sync::Arc;

use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;

use crate::errors::ValidationError;
use crate::templatetags::TagSpecs;

#[salsa::db]
pub trait Db: TemplateDb + WorkspaceDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> Arc<TagSpecs>;
}

/// Accumulator for validation errors
#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
