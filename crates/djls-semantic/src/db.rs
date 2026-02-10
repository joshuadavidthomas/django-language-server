use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_python::EnvironmentInventory;
use djls_project::TemplateTags;
use djls_templates::Db as TemplateDb;

use crate::blocks::TagIndex;
use crate::errors::ValidationError;
use crate::filters::arity::FilterAritySpecs;
use crate::templatetags::TagSpecs;

#[salsa::db]
pub trait Db: TemplateDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> TagSpecs;

    fn tag_index(&self) -> TagIndex<'_>;

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>>;

    /// Get the diagnostics configuration
    fn diagnostics_config(&self) -> DiagnosticsConfig;

    /// Get the inspector inventory (template tags from the Python inspector).
    ///
    /// Returns `None` when the inspector is unavailable or hasn't been queried yet.
    /// When `None`, load-scoping diagnostics (S108/S109/S110) are suppressed entirely.
    fn inspector_inventory(&self) -> Option<TemplateTags>;

    /// Get the filter arity specifications for filter argument validation.
    ///
    /// Built from extraction results. Returns empty specs when no extraction
    /// data is available.
    fn filter_arity_specs(&self) -> FilterAritySpecs;

    /// Get the environment inventory from scanning `sys.path` for templatetag modules.
    ///
    /// Returns `None` when the environment hasn't been scanned yet.
    /// Used for three-layer resolution (environment → `INSTALLED_APPS` → load).
    fn environment_inventory(&self) -> Option<EnvironmentInventory>;
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
