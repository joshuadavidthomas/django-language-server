mod rules;
mod specs;

pub(crate) use rules::evaluate_tag_rules;
pub use specs::EndTag;
#[cfg(test)]
pub(crate) use specs::IntermediateTag;
pub use specs::TagArgument;
pub use specs::TagArgumentKind;
pub use specs::TagSpec;
pub use specs::TagSpecs;
pub use specs::builtin_tag_specs;

use crate::resolution::TemplateReferenceKind;

/// Durable Django template meaning for a tag.
///
/// This describes what the tag does in the template domain. Feature-specific
/// projections, such as document symbols, map these roles into their own shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagRole {
    TemplateReference(TemplateReferenceKind),
    TemplateLibraryLoader,
    TemplateBlock,
    ControlTag,
    TemplateTag,
    StaticAssetReference,
    RouteReference,
}
