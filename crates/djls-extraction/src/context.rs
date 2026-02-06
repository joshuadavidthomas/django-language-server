use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;

/// Context information extracted from a tag registration function.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct FunctionContext {
    /// Variable name bound to `token.split_contents()` result (e.g., "bits", "args", "parts")
    pub split: Option<String>,
    /// Variable name for the parser parameter
    pub parser: Option<String>,
    /// Variable name for the token parameter
    pub token: Option<String>,
}

impl FunctionContext {
    pub fn from_registration(
        _parsed: &ParsedModule,
        _reg: &RegistrationInfo,
    ) -> Self {
        Self::default()
    }
}
