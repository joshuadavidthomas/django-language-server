use djls_conf::DiagnosticsConfig;
use djls_project::Db as ProjectDb;
use djls_project::ModelGraph;
use djls_project::TemplateEnvironment;
use djls_project::template_environment;
use djls_source::File;

use crate::errors::ValidationError;
use crate::filters::FilterAritySpecs;
use crate::tags::TagSpecs;

#[salsa::db]
pub trait Db: ProjectDb {
    /// Get the Django tag specifications for semantic analysis.
    fn tag_specs(&self) -> &TagSpecs;

    /// Get the diagnostics configuration.
    fn diagnostics_config(&self) -> DiagnosticsConfig;

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

pub fn template_environment_for_file(db: &dyn Db, file: File) -> TemplateEnvironment<'_> {
    db.project().map_or_else(
        || TemplateEnvironment::from_project_inventory(djls_project::TemplateLibraries::empty_ref()),
        |project| template_environment(db, project, file),
    )
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
