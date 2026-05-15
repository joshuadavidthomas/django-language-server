use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::TagArgument;
use salsa::Accumulator;

use crate::db::Db;
use crate::tag_rules::evaluate_tag_rules;
use crate::ValidationErrorAccumulator;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_tag_arguments_rule(
    db: &dyn Db,
    name: &str,
    arguments: &[TagArgument],
    span: Span,
    rules: &crate::TagRule,
) {
    let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
    let bits = arguments
        .iter()
        .map(|argument| argument.as_str().to_string())
        .collect::<Vec<_>>();
    for error in evaluate_tag_rules(name, &bits, rules, marker_span) {
        ValidationErrorAccumulator(error).accumulate(db);
    }
}
