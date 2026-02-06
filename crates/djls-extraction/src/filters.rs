use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::FilterArity;
use crate::ExtractionError;

#[allow(clippy::unnecessary_wraps)]
pub fn extract_filter_arity(
    _parsed: &ParsedModule,
    _reg: &RegistrationInfo,
) -> Result<FilterArity, ExtractionError> {
    Ok(FilterArity::Unknown)
}
