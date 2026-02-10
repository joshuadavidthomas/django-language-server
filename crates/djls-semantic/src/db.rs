use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_project::TemplateLibraries;
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

    /// Get template libraries for the current project.
    ///
    /// This includes:
    /// - discovered libraries from scanning `sys.path`
    /// - installed libraries/symbols from the Django inspector (when available)
    fn template_libraries(&self) -> TemplateLibraries;

    /// Get the filter arity specifications for filter argument validation.
    ///
    /// Built from extraction results. Returns empty specs when no extraction
    /// data is available.
    fn filter_arity_specs(&self) -> FilterAritySpecs;
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
