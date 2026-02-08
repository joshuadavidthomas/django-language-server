mod specs;

pub use specs::EndTag;
#[cfg(test)]
pub(crate) use specs::IntermediateTag;
pub use specs::TagSpec;
pub use specs::TagSpecs;
#[cfg(test)]
pub(crate) use specs::test_tag_specs;
