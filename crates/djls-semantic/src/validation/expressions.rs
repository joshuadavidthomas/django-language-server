use salsa::Accumulator;

use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::expressions::ConsumedArguments;
use crate::expressions::parse_filter_expression;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_consumed_expressions_rule(
    db: &dyn Db,
    name: &str,
    consumed: &ConsumedArguments<'_>,
) {
    for operand in &consumed.expressions {
        if let Err(message) = parse_filter_expression(&operand.text) {
            ValidationErrorAccumulator(ValidationError::ExpressionSyntaxError {
                tag: name.to_string(),
                message,
                span: operand.span,
            })
            .accumulate(db);
        }
    }
}
