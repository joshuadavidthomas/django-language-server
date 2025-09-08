//! Template-specific database trait and Salsa integration.
//!
//! This module implements the incremental computation infrastructure for Django templates
//! using Salsa. It extends the workspace database with template-specific functionality
//! including parsing, validation, and diagnostic accumulation.
//!
//! ## Architecture
//!
//! The module uses Salsa's incremental computation framework to:
//! - Cache parsed ASTs and only reparse when files change
//! - Accumulate diagnostics during parsing and validation
//! - Provide efficient workspace-wide diagnostic collection
//!
//! ## Key Components
//!
//! - [`Db`]: Database trait extending the workspace database
//! - [`analyze_template`]: Main entry point for template analysis
//! - [`TemplateDiagnostic`]: Accumulator for collecting LSP diagnostics
//!
//! ## Incremental Computation
//!
//! When a template file changes:
//! 1. Salsa invalidates the cached AST for that file
//! 2. Next access to `analyze_template` triggers reparse
//! 3. Diagnostics are accumulated during parse/validation
//! 4. Other files remain cached unless they also changed
//!
//! ## Example
//!
//! ```ignore
//! // Analyze a template and get its AST
//! let ast = analyze_template(db, file);
//!
//! // Retrieve accumulated diagnostics
//! let diagnostics = analyze_template::accumulated::<TemplateDiagnostic>(db, file);
//!
//! // Get diagnostics for all workspace files
//! for file in workspace.files() {
//!     let _ = analyze_template(db, file); // Trigger analysis
//!     let diags = analyze_template::accumulated::<TemplateDiagnostic>(db, file);
//!     // Process diagnostics...
//! }
//! ```

use std::sync::Arc;

use djls_workspace::Db as WorkspaceDb;
use tower_lsp_server::lsp_types;

use crate::templatetags::TagSpecs;

/// Thin wrapper around LSP diagnostic for accumulator
#[salsa::accumulator]
pub struct TemplateDiagnostic(pub lsp_types::Diagnostic);

impl From<TemplateDiagnostic> for lsp_types::Diagnostic {
    fn from(diagnostic: TemplateDiagnostic) -> Self {
        diagnostic.0
    }
}

impl From<&TemplateDiagnostic> for lsp_types::Diagnostic {
    fn from(diagnostic: &TemplateDiagnostic) -> Self {
        diagnostic.0.clone()
    }
}

/// Template-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {
    /// Get the Django tag specifications for template parsing and validation
    fn tag_specs(&self) -> Arc<TagSpecs>;
}
