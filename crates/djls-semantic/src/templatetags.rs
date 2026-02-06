mod builtins;
mod specs;

pub use builtins::django_builtin_specs;
pub use specs::EndTag;
#[cfg(test)]
pub(crate) use specs::IntermediateTag;
pub use specs::TagSpec;
pub use specs::TagSpecs;
