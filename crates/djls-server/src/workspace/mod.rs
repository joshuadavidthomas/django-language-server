// Module declarations
mod document;
mod store;
mod utils;

// Public re-exports for document types
pub use document::ClosingBrace;
pub use document::LanguageId;
pub use document::LineIndex;
pub use document::TemplateTagContext;
pub use document::TextDocument;
// Public re-exports for store
pub use store::Store;
// Public re-exports for workspace utilities
pub use utils::get_project_path;
