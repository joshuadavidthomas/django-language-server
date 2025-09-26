use djls_templates::Db as TemplateDb;

use crate::blocks::TagIndex;
use crate::errors::ValidationError;
use crate::templatetags::TagSpecs;

#[salsa::db]
pub trait Db: TemplateDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> TagSpecs;
    fn tag_index(&self) -> TagIndex<'_>;
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
