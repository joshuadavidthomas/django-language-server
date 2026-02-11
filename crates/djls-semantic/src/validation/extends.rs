use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use salsa::Accumulator;

use crate::db::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_extends_rule(
    db: &dyn Db,
    span: Span,
    seen_extends: bool,
    contains_nontext: bool,
) {
    let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    if seen_extends {
        ValidationErrorAccumulator(ValidationError::MultipleExtends { span: marker_span }).accumulate(db);
    } else if contains_nontext {
        ValidationErrorAccumulator(ValidationError::ExtendsMustBeFirst { span: marker_span }).accumulate(db);
    }
}
