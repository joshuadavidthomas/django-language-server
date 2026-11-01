//! Template parser Salsa database and diagnostics.
//!
//! The parser needs file source text and the [`LexerMode`] that decides
//! whether tags may span lines. The database supplies the mode so this crate
//! stays independent of how a project derives it.
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
//! // Parse a readable template and get its AST
//! let nodelist = parse_template(db, file).expect("template should be readable");
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

use crate::lexer::LexerMode;
use crate::parser::ParseError;

#[salsa::db]
pub trait Db: djls_source::Db {
    /// How the lexer delimits tags in this database's templates.
    ///
    /// Tracked queries that lex call this, so an implementation backed by
    /// tracked project queries invalidates lexing when those queries change.
    fn lexer_mode(&self) -> LexerMode;
}

/// Accumulator for template parse errors.
#[salsa::accumulator]
pub struct TemplateErrorAccumulator(pub ParseError);
