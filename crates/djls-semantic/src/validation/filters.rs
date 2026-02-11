use djls_templates::Filter;
use salsa::Accumulator;

use crate::db::Db;
use crate::specs::filters::FilterAritySpecs;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_filter_arity_rule(
    db: &dyn Db,
    filter: &Filter,
    arity_specs: &FilterAritySpecs,
    template_libraries: &djls_project::TemplateLibraries,
) {
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

    if let Some(arity) = arity_specs.get(&filter.name) {
        let has_arg = filter.arg.is_some();

        if arity.expects_arg && !arity.arg_optional && !has_arg {
            // S115: required argument missing
            ValidationErrorAccumulator(ValidationError::FilterMissingArgument {
                filter: filter.name.clone(),
                span: filter.span,
            })
            .accumulate(db);
        } else if !arity.expects_arg && has_arg {
            // S116: unexpected argument provided
            ValidationErrorAccumulator(ValidationError::FilterUnexpectedArgument {
                filter: filter.name.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
    }
}
