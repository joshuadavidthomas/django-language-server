use djls_source::File;
use djls_templates::ParseError;
use djls_templates::TemplateErrorAccumulator;
use djls_templates::parse_template;

use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;
use crate::validate_template_file;

/// Syntax and semantic diagnostics collected for one Template, borrowed from the
/// database's accumulators.
pub struct TemplateDiagnostics<'db> {
    /// Syntax errors in parser accumulator order.
    pub template_errors: Vec<&'db ParseError>,
    /// Semantic errors in validation accumulator order.
    pub validation_errors: Vec<&'db ValidationError>,
}

impl TemplateDiagnostics<'_> {
    /// Return whether syntax or validation produced any diagnostics.
    #[must_use]
    pub fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
    }
}

/// Run Template validation and collect its syntax and semantic diagnostics.
#[must_use]
pub fn collect_template_diagnostics(db: &dyn Db, file: File) -> TemplateDiagnostics<'_> {
    validate_template_file(db, file);

    let template_errors = parse_template::accumulated::<TemplateErrorAccumulator>(db, file)
        .into_iter()
        .map(|accumulator| &accumulator.0)
        .collect();
    let validation_errors =
        validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file)
            .into_iter()
            .map(|accumulator| &accumulator.0)
            .collect();

    TemplateDiagnostics {
        template_errors,
        validation_errors,
    }
}
