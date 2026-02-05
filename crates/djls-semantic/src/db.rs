use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_project::TemplateTags;
use djls_templates::Db as TemplateDb;

use crate::blocks::TagIndex;
use crate::errors::ValidationError;
use crate::templatetags::TagSpecs;

#[salsa::db]
pub trait Db: TemplateDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> TagSpecs;

    fn tag_index(&self) -> TagIndex<'_>;

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>>;

    /// Get the diagnostics configuration
    fn diagnostics_config(&self) -> DiagnosticsConfig;

    /// Get the inspector inventory of template tags (from Python runtime).
    ///
    /// Returns `None` when the inspector is unavailable (Django not initialized,
    /// Python env not configured, inspector crashed).
    fn inspector_inventory(&self) -> Option<TemplateTags>;
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
