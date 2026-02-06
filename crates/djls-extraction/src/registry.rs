use crate::parser::ParsedModule;
use crate::types::DecoratorKind;
use crate::ExtractionError;

/// Information about a found registration decorator.
#[derive(Debug, Clone)]
pub struct RegistrationInfo {
    pub name: String,
    pub decorator_kind: DecoratorKind,
    // Additional fields will be added in Phase 2
}

/// Registry of found tag and filter registrations.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub tags: Vec<RegistrationInfo>,
    pub filters: Vec<RegistrationInfo>,
}

/// Find all `@register.tag`, `@register.filter`, and related decorators in the AST.
pub fn find_registrations(_parsed: &ParsedModule) -> Result<Registry, ExtractionError> {
    // Placeholder implementation - Phase 2 will implement this
    Ok(Registry::default())
}
