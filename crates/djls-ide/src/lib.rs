mod completions;
mod context;
mod diagnostics;
mod ext;
mod folding;
mod formatting;
mod hover;
mod navigation;
mod snippets;
mod symbols;

pub use completions::handle_completion;
pub use diagnostics::collect_diagnostics;
pub use folding::collect_folding_ranges;
pub use formatting::format_document;
pub use hover::hover;
pub use navigation::find_references;
pub use navigation::goto_definition;
pub use symbols::document_symbols;

pub(crate) const SOURCE_NAME: &str = "djls";
