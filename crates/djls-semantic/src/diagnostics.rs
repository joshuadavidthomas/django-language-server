use djls_source::File;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;

use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;
use crate::validate_template_file;

/// Syntax and semantic diagnostics collected for one Template.
pub struct TemplateDiagnostics {
    /// Syntax errors in parser accumulator order.
    pub template_errors: Vec<TemplateError>,
    /// Semantic errors in validation accumulator order.
    pub validation_errors: Vec<ValidationError>,
}

impl TemplateDiagnostics {
    /// Return whether syntax or validation produced any diagnostics.
    #[must_use]
    pub fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
    }
}

/// Run Template validation and collect its syntax and semantic diagnostics.
#[must_use]
pub fn collect_template_diagnostics(db: &dyn Db, file: File) -> TemplateDiagnostics {
    validate_template_file(db, file);

    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file)
            .iter()
            .map(|accumulator| accumulator.0.clone())
            .collect();
    let validation_errors =
        validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file)
            .iter()
            .map(|accumulator| accumulator.0.clone())
            .collect();

    TemplateDiagnostics {
        template_errors,
        validation_errors,
    }
}
