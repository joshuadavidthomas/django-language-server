//! Template-specific database trait and Salsa integration.
//!
//! This module implements the incremental computation infrastructure for Django templates
//! using Salsa. It extends the workspace database with template-specific functionality
//! including parsing, validation, and error accumulation.
//!
//! ## Architecture
//!
//! The module uses Salsa's incremental computation framework to:
//! - Cache parsed ASTs and only reparse when files change
//! - Accumulate errors during parsing and validation
//! - Provide efficient workspace-wide error collection
//!
//! ## Key Components
//!
//! - [`Db`]: Database trait extending the workspace database
//! - [`parse_template`]: Main entry point for template parsing
//! - [`TemplateErrorAccumulator`]: Accumulator for collecting template errors
//!
//! ## Incremental Computation
//!
//! When a template file changes:
//! 1. Salsa invalidates the cached AST for that file
//! 2. Next access to `parse_template` triggers reparse
//! 3. Errors are accumulated during parse/validation
//! 4. Other files remain cached unless they also changed
//!
//! ## Example
//!
//! ```ignore
//! // Parse a template and get its AST
//! let nodelist = parse_template(db, file);
//!
//! // Retrieve accumulated errors
//! let errors = parse_template::accumulated::<TemplateErrorAccumulator>(db, file);
//!
//! // Get errors for all workspace files
//! for file in workspace.files() {
//!     let _ = parse_template(db, file); // Trigger parsing
//!     let errors = parse_template::accumulated::<TemplateErrorAccumulator>(db, file);
//!     // Process errors...
//! }
//! ```

use djls_source::Db as SourceDb;

use crate::error::TemplateError;

/// Accumulator for template errors
#[salsa::accumulator]
pub struct TemplateErrorAccumulator(pub TemplateError);

/// Template-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: SourceDb {}
