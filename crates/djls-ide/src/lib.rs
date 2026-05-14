mod completions;
mod context;
mod diagnostics;
mod ext;
mod folding;
mod hover;
mod navigation;
mod snippets;
mod symbols;

pub use completions::handle_completion;
pub use diagnostics::collect_diagnostics;
pub use folding::collect_folding_ranges;
pub use hover::hover;
pub use navigation::find_references;
pub use navigation::goto_definition;
pub use snippets::generate_partial_snippet;
pub use snippets::generate_snippet_for_tag;
pub use snippets::generate_snippet_for_tag_with_end;
pub use snippets::generate_snippet_from_args;
pub use symbols::document_symbols;

pub const SOURCE_NAME: &str = "djls";
