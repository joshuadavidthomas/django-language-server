use crate::context::FunctionContext;
use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::BlockTagSpec;
use crate::ExtractionError;

#[allow(clippy::unnecessary_wraps)]
pub fn extract_block_spec(
    _parsed: &ParsedModule,
    _reg: &RegistrationInfo,
    _ctx: &FunctionContext,
) -> Result<Option<BlockTagSpec>, ExtractionError> {
    Ok(None)
}
