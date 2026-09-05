use djls_project::FilterArity;
use djls_templates::Filter;
use salsa::Accumulator;

use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_filter_arity_rule(db: &dyn Db, filter: &Filter, arity: FilterArity) {
    match arity {
        FilterArity::NoArgument if filter.arg.is_some() => {
            // S116: unexpected argument provided
            ValidationErrorAccumulator(ValidationError::FilterUnexpectedArgument {
                filter: filter.name.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
        FilterArity::RequiredArgument if filter.arg.is_none() => {
            // S115: required argument missing
            ValidationErrorAccumulator(ValidationError::FilterMissingArgument {
                filter: filter.name.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
        FilterArity::NoArgument | FilterArity::RequiredArgument | FilterArity::OptionalArgument => {
        }
    }
}
