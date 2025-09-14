//! IDE features for Django Language Server
//!
//! This crate contains all editor features and IDE functionality,
//! providing a clean interface between semantic analysis and LSP transport.

pub mod completions;
pub mod converters;
pub mod diagnostics;
pub mod snippets;

// Re-export public API
pub use completions::handle_completion;
pub use diagnostics::collect_diagnostics;
pub use diagnostics::DiagnosticSeverity;
pub use diagnostics::IdeDiagnostic;
pub use snippets::generate_partial_snippet;
pub use snippets::generate_snippet_for_tag;
pub use snippets::generate_snippet_for_tag_with_end;
pub use snippets::generate_snippet_from_args;
