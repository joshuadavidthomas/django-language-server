use crate::parser::ParsedModule;
use crate::types::DecoratorKind;
use crate::ExtractionError;

/// Information about a discovered registration decorator.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RegistrationInfo {
    /// The registered name (tag/filter name in templates)
    pub name: String,
    /// Kind of decorator
    pub decorator_kind: DecoratorKind,
    /// Index of the function definition in the module body
    pub func_index: usize,
}

/// All registrations found in a module.
#[derive(Debug, Clone, Default)]
pub struct FoundRegistrations {
    pub tags: Vec<RegistrationInfo>,
    pub filters: Vec<RegistrationInfo>,
}

#[allow(clippy::unnecessary_wraps)]
pub fn find_registrations(
    _parsed: &ParsedModule,
) -> Result<FoundRegistrations, ExtractionError> {
    Ok(FoundRegistrations::default())
}
