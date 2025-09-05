//! Logging infrastructure for forwarding tracing events to LSP client messages.
//!
//! This module provides the `LspLayer` implementation for forwarding tracing
//! events to the LSP client through the tracing infrastructure.
//!
//! ## `LspLayer`
//!
//! The `LspLayer` is a tracing `Layer` that intercepts tracing events and
//! forwards appropriate ones to the LSP client. It filters events by level:
//! - ERROR, WARN, INFO, DEBUG → forwarded to LSP client
//! - TRACE → kept server-side only (for performance)
//!
//! The `LspLayer` automatically handles forwarding appropriate log levels
//! to the LSP client while preserving structured logging data for file output.

use std::sync::Arc;

use tower_lsp_server::lsp_types;
use tracing::field::Visit;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;

/// A tracing Layer that forwards events to the LSP client.
///
/// This layer intercepts tracing events and converts them to LSP log messages
/// that are sent to the client. It filters events by level to avoid overwhelming
/// the client with verbose trace logs.
pub struct LspLayer {
    send_message: Arc<dyn Fn(lsp_types::MessageType, String) + Send + Sync>,
}

impl LspLayer {
    pub fn new<F>(send_message: F) -> Self
    where
        F: Fn(lsp_types::MessageType, String) + Send + Sync + 'static,
    {
        Self {
            send_message: Arc::new(send_message),
        }
    }
}

struct MessageVisitor {
    message: Option<String>,
}

impl MessageVisitor {
    fn new() -> Self {
        Self { message: None }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }
}

impl<S> Layer<S> for LspLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();

        let message_type = match *metadata.level() {
            Level::ERROR => lsp_types::MessageType::ERROR,
            Level::WARN => lsp_types::MessageType::WARNING,
            Level::INFO => lsp_types::MessageType::INFO,
            Level::DEBUG => lsp_types::MessageType::LOG,
            Level::TRACE => {
                // Skip TRACE level - too verbose for LSP client
                // TODO: Add MessageType::Debug in LSP 3.18.0
                return;
            }
        };

        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        if let Some(message) = visitor.message {
            (self.send_message)(message_type, message);
        }
    }
}

/// Initialize the dual-layer tracing subscriber.
///
/// Sets up:
/// - File layer: writes to /tmp/djls.log with daily rotation
/// - LSP layer: forwards INFO+ messages to the client
/// - `EnvFilter`: respects `RUST_LOG` env var, defaults to "info"
///
/// Returns a `WorkerGuard` that must be kept alive for the file logging to work.
pub fn init_tracing<F>(send_message: F) -> WorkerGuard
where
    F: Fn(lsp_types::MessageType, String) + Send + Sync + 'static,
{
    let file_appender = tracing_appender::rolling::daily("/tmp", "djls.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .with_filter(env_filter);

    let lsp_layer =
        LspLayer::new(send_message).with_filter(tracing_subscriber::filter::LevelFilter::INFO);

    Registry::default().with(file_layer).with(lsp_layer).init();

    guard
}
