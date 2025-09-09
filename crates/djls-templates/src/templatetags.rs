mod snippets;
mod specs;

pub use snippets::generate_partial_snippet;
pub use snippets::generate_snippet_for_tag;
pub use snippets::generate_snippet_for_tag_with_end;
pub use snippets::generate_snippet_from_args;
pub use specs::Arg;
pub use specs::ArgType;
pub use specs::SimpleArgType;
pub use specs::TagSpecs;

pub enum TagType {
    Opener,
    Intermediate,
    Closer,
    Standalone,
}

impl TagType {
    #[must_use]
    pub fn for_name(name: &str, tag_specs: &TagSpecs) -> TagType {
        if tag_specs.is_opener(name) {
            TagType::Opener
        } else if tag_specs.is_closer(name) {
            TagType::Closer
        } else if tag_specs.is_intermediate(name) {
            TagType::Intermediate
        } else {
            TagType::Standalone
        }
    }
}
