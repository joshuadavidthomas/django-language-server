mod completions;
mod diagnostics;
mod navigation;
mod snippets;

pub use completions::handle_completion;
pub use diagnostics::collect_diagnostics;
pub use navigation::find_template_references;
pub use navigation::goto_template_definition;
pub use snippets::generate_partial_snippet;
pub use snippets::generate_snippet_for_tag;
pub use snippets::generate_snippet_for_tag_with_end;
pub use snippets::generate_snippet_from_args;
