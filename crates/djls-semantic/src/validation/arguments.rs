use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use salsa::Accumulator;

use crate::db::Db;
use crate::tag_rules::evaluate_tag_rules;
use crate::ValidationErrorAccumulator;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_tag_arguments_rule(
    db: &dyn Db,
    name: &str,
    bits: &[String],
    span: Span,
    rules: &djls_python::TagRule,
) {
    let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
    for error in evaluate_tag_rules(name, bits, rules, marker_span) {
        ValidationErrorAccumulator(error).accumulate(db);
    }
}
