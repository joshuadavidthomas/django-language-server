mod builtins;
mod specs;

pub(crate) use builtins::django_builtin_specs;
pub use specs::EndTag;
pub(crate) use specs::IntermediateTag;
pub use specs::TagArg;
pub use specs::TagSpec;
pub use specs::TagSpecs;
