use crate::errors::ValidationError;
use crate::expressions::ConsumedArguments;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_consumed_expressions_rule(
    errors: &mut Vec<ValidationError>,
    name: &str,
    consumed: &ConsumedArguments<'_>,
) {
    for operand in &consumed.expressions {
        if let Err(message) = &operand.parsed {
            errors.push(ValidationError::ExpressionSyntaxError {
                tag: name.to_string(),
                message: message.clone(),
                span: operand.span,
            });
        }
    }
}
