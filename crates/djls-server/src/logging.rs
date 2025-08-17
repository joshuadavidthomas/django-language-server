//! Temporary logging macros for dual-dispatch to both LSP client and tracing.
//!
//! These macros bridge the gap during our migration from `client::log_message`
//! to the tracing infrastructure. They ensure messages are sent to both systems
//! so we maintain LSP client visibility while building out tracing support.
//!
//! Each macro supports two invocation patterns to handle the different APIs:
//!
//! 1. String literal:
//! ```rust,ignore
//! log_info!("Server initialized");
//! log_warn!("Configuration not found");
//! log_error!("Failed to parse document");
//! ```
//!
//! 2. Format string with arguments:
//! ```rust,ignore
//! log_info!("Processing {} documents", count);
//! log_warn!("Timeout after {}ms for {}", ms, path);
//! log_error!("Failed to open {}: {}", file, err);
//! ```
//!
//! The difference in the macro arms exists because of how each system works:
//!
//! - `client::log_message` expects a single string value
//! - `tracing` macros can handle format strings natively for structured logging
//! - For format strings, we format once for the client but pass the original
//!   format string and args to tracing to preserve structured data

#[macro_export]
macro_rules! log_info {
    ($msg:literal) => {
        $crate::client::log_message(tower_lsp_server::lsp_types::MessageType::INFO, $msg);
        tracing::info!($msg);
    };
    ($fmt:literal, $($arg:tt)*) => {
        $crate::client::log_message(tower_lsp_server::lsp_types::MessageType::INFO, format!($fmt, $($arg)*));
        tracing::info!($fmt, $($arg)*);
    };
}

#[macro_export]
macro_rules! log_warn {
    ($msg:literal) => {
        $crate::client::log_message(tower_lsp_server::lsp_types::MessageType::WARNING, $msg);
        tracing::warn!($msg);
    };
    ($fmt:literal, $($arg:tt)*) => {
        $crate::client::log_message(tower_lsp_server::lsp_types::MessageType::WARNING, format!($fmt, $($arg)*));
        tracing::warn!($fmt, $($arg)*);
    };
}

#[macro_export]
macro_rules! log_error {
    ($msg:literal) => {
        $crate::client::log_message(tower_lsp_server::lsp_types::MessageType::ERROR, $msg);
        tracing::error!($msg);
    };
    ($fmt:literal, $($arg:tt)*) => {
        $crate::client::log_message(tower_lsp_server::lsp_types::MessageType::ERROR, format!($fmt, $($arg)*));
        tracing::error!($fmt, $($arg)*);
    };
}
