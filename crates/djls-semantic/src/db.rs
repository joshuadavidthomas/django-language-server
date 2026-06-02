use djls_conf::DiagnosticsConfig;

use crate::errors::ValidationError;
use crate::project::TemplateLibraries;
use crate::python::ModelGraph;
use crate::specs::filters::FilterAritySpecs;
use crate::specs::tags::TagSpecs;

#[salsa::db]
pub trait Db: djls_project::Db {
    /// Get the computed Django tag specifications for semantic analysis.
    fn tag_specs(&self) -> &TagSpecs;

    /// Get the diagnostics configuration.
    fn diagnostics_config(&self) -> DiagnosticsConfig;

    /// Get template libraries for the current project.
    ///
    /// This includes:
    /// - discovered libraries from scanning `sys.path`
    /// - installed libraries/symbols from project introspection (when available)
    fn template_libraries(&self) -> &TemplateLibraries;

    /// Get the filter arity specifications for filter argument validation.
    ///
    /// Built from extraction results. Returns empty specs when no extraction
    /// data is available.
    fn filter_arity_specs(&self) -> &FilterAritySpecs;

    /// Get the merged model graph for the current project.
    ///
    /// Combines models from both workspace `models.py` files and installed
    /// packages (site-packages). Returns an empty graph when no project is
    /// configured.
    fn model_graph(&self) -> &ModelGraph;
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
