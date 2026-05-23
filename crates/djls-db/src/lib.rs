//! Concrete Salsa database for the Django Language Server.
//!
//! This crate owns the [`DjangoDatabase`] struct — the single concrete
//! implementation of all Salsa database traits (`SourceDb`, `SemanticDb`,
//! `djls_project::Db`). Both the LSP server and CLI commands consume this crate.
//!
mod db;
mod settings;

pub use db::DjangoDatabase;
pub use settings::SettingsUpdate;
