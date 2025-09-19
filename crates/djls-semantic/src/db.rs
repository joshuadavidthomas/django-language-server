use djls_templates::Db as TemplateDb;

use crate::errors::ValidationError;
use crate::templatetags::TagSpecs;

#[salsa::db]
pub trait Db: TemplateDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> TagSpecs;
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
