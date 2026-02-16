//! Concrete Salsa database for the Django Language Server.
//!
//! This crate owns the [`DjangoDatabase`] struct — the single concrete
//! implementation of all Salsa database traits (`SourceDb`, `WorkspaceDb`,
//! `TemplateDb`, `SemanticDb`, `ProjectDb`).  Both the LSP server and CLI
//! commands consume this crate.

mod db;
mod inspector;
mod queries;
mod scanning;
mod settings;

pub use db::DjangoDatabase;
pub use settings::SettingsUpdate;
