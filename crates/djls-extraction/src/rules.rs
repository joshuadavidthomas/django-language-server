use crate::context::FunctionContext;
use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::ExtractedRule;
use crate::ExtractionError;

/// Extract validation rules from TemplateSyntaxError guards.
pub fn extract_tag_rules(
    _parsed: &ParsedModule,
    _reg: &RegistrationInfo,
    _ctx: &FunctionContext,
) -> Result<Vec<ExtractedRule>, ExtractionError> {
    // Placeholder implementation - Phase 4 will implement this
    Ok(Vec::new())
}
