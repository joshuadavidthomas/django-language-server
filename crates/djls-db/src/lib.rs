//! Concrete Salsa database for the Django Language Server.
//!
//! This crate owns the [`DjangoDatabase`] struct — the single concrete
//! implementation of all Salsa database traits (`SourceDb`, `WorkspaceDb`,
//! `TemplateDb`, `SemanticDb`, `ProjectDb`).  Both the LSP server and CLI
//! commands consume this crate.
//!
//! The [`check_file`] function provides the shared per-file check
//! orchestration (parse → validate → collect errors) used by both
//! the CLI and the LSP server.

mod check;
mod db;
mod inspector;
mod queries;
mod scanning;
mod settings;

pub use check::check_file;
pub use check::render_template_error;
pub use check::render_validation_error;
pub use check::CheckResult;
pub use db::DjangoDatabase;
pub use inspector::load_inspector_cache;
pub use settings::SettingsUpdate;
