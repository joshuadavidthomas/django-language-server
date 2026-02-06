use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;

/// Function context containing detected variable names.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct FunctionContext {
    /// Name of the variable bound to `token.split_contents()`
    pub split_var: Option<String>,
    /// Name of the parser parameter
    pub parser_var: Option<String>,
    /// Name of the token parameter  
    pub token_var: Option<String>,
}

impl FunctionContext {
    /// Build function context from a registration, detecting split-contents variable.
    pub fn from_registration(_parsed: &ParsedModule, _reg: &RegistrationInfo) -> Self {
        // Placeholder implementation - Phase 3 will implement this
        Self::default()
    }
}
