use djls_conf::DiagnosticsConfig;
use djls_project::Db as ProjectDb;
use djls_project::ScopedTemplateLibraries;
use djls_project::TemplateLibraryCatalog;
use djls_project::scoped_template_libraries;
use djls_source::File;

use crate::errors::ValidationError;
use crate::filters::FilterAritySpecs;
use crate::tags::TagSpecs;

#[salsa::db]
pub trait Db: ProjectDb {
    /// Tag specs for structural analysis when there is no Project to extract from.
    ///
    /// Production supplies the Django fallback table; test databases may supply
    /// their own. Project-backed paths must derive meaning through per-library
    /// products.
    fn projectless_tag_specs(&self) -> &TagSpecs;

    /// Get the diagnostics configuration.
    fn diagnostics_config(&self) -> DiagnosticsConfig;

    /// Explicit fixture seam for Filter validation without a Project.
    fn projectless_filter_arity_specs(&self) -> &FilterAritySpecs;
}

pub fn scoped_template_libraries_for_file(
    db: &dyn Db,
    file: File,
) -> ScopedTemplateLibraries<'_, '_> {
    db.project().map_or_else(
        || ScopedTemplateLibraries::from_project_inventory(TemplateLibraryCatalog::empty_ref()),
        |project| scoped_template_libraries(db, project, file),
    )
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
