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
//! - [`parse_template`]: Main entry point for template parsing
//! - [`TemplateDiagnostic`]: Accumulator for collecting LSP diagnostics
//!
//! ## Incremental Computation
//!
//! When a template file changes:
//! 1. Salsa invalidates the cached AST for that file
//! 2. Next access to `parse_template` triggers reparse
//! 3. Diagnostics are accumulated during parse/validation
//! 4. Other files remain cached unless they also changed
//!
//! ## Example
//!
//! ```ignore
//! // Parse a template and get its AST
//! let nodelist = parse_template(db, file);
//!
//! // Retrieve accumulated diagnostics
//! let diagnostics = parse_template::accumulated::<TemplateDiagnostic>(db, file);
//!
//! // Get diagnostics for all workspace files
//! for file in workspace.files() {
//!     let _ = parse_template(db, file); // Trigger parsing
//!     let diags = parse_template::accumulated::<TemplateDiagnostic>(db, file);
//!     // Process diagnostics...
//! }
//! ```

use djls_workspace::Db as WorkspaceDb;

/// Accumulator for collecting syntax diagnostics
#[salsa::accumulator]
pub struct SyntaxDiagnosticAccumulator(pub crate::error::SyntaxDiagnostic);

/// Template-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {}
