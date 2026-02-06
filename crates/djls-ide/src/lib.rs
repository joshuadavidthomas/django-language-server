mod completions;
mod context;
mod diagnostics;
mod ext;
mod navigation;
mod snippets;

pub use completions::handle_completion;
pub use diagnostics::collect_diagnostics;
pub use navigation::find_references;
pub use navigation::goto_definition;
pub use snippets::generate_snippet_for_tag_with_end;

pub const SOURCE_NAME: &str = "djls";
