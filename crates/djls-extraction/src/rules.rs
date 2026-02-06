use crate::context::FunctionContext;
use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::ExtractedRule;
use crate::ExtractionError;

#[allow(clippy::unnecessary_wraps)]
pub fn extract_tag_rules(
    _parsed: &ParsedModule,
    _reg: &RegistrationInfo,
    _ctx: &FunctionContext,
) -> Result<Vec<ExtractedRule>, ExtractionError> {
    Ok(Vec::new())
}
