use djls_project::TagRule;
use djls_source::Span;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;

use crate::errors::ValidationError;
use crate::tags::evaluate_tag_rules;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_tag_arguments_rule(
    errors: &mut Vec<ValidationError>,
    name: &str,
    bits: &[TagBit],
    span: Span,
    rules: &TagRule,
) {
    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
    let bits = bits.iter().map(TagBit::as_str).collect::<Vec<_>>();
    for error in evaluate_tag_rules(name, &bits, rules, full_span) {
        errors.push(error);
    }
}
