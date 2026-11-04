use djls_project::FilterArity;
use djls_templates::Filter;

use crate::errors::ValidationError;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_filter_arity_rule(
    errors: &mut Vec<ValidationError>,
    filter: &Filter,
    arity: FilterArity,
) {
    match arity {
        FilterArity::NoArgument if filter.arg.is_some() => {
            // S116: unexpected argument provided
            errors.push(ValidationError::FilterUnexpectedArgument {
                filter: filter.name.to_string(),
                span: filter.span,
            });
        }
        FilterArity::RequiredArgument if filter.arg.is_none() => {
            // S115: required argument missing
            errors.push(ValidationError::FilterMissingArgument {
                filter: filter.name.to_string(),
                span: filter.span,
            });
        }
        FilterArity::NoArgument | FilterArity::RequiredArgument | FilterArity::OptionalArgument => {
        }
    }
}
