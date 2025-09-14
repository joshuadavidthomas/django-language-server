//! IDE features for Django Language Server
//!
//! This crate contains all editor features and IDE functionality,
//! providing a clean interface between semantic analysis and LSP transport.

pub mod completions;
pub mod diagnostics;
pub mod snippets;
pub mod converters;

// Re-export public API
pub use completions::handle_completion;
pub use diagnostics::{collect_diagnostics, IdeDiagnostic, DiagnosticSeverity};
pub use snippets::{generate_partial_snippet, generate_snippet_for_tag, generate_snippet_for_tag_with_end, generate_snippet_from_args};