pub mod builtins;
pub mod db;
pub mod snippets;
pub mod specs;
pub mod validation;

pub use builtins::django_builtin_specs;
pub use db::SemanticDb;
pub use snippets::generate_partial_snippet;
pub use snippets::generate_snippet_for_tag;
pub use snippets::generate_snippet_for_tag_with_end;
pub use snippets::generate_snippet_from_args;
pub use specs::ArgType;
pub use specs::EndTag;
pub use specs::IntermediateTag;
pub use specs::SimpleArgType;
pub use specs::TagArg;
pub use specs::TagSpec;
pub use specs::TagSpecs;
pub use validation::TagValidator;

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
