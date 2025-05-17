use std::future::Future;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc, OnceLock,
};

use tower_lsp_server::jsonrpc::Error;
use tower_lsp_server::lsp_types::notification::Notification;
use tower_lsp_server::lsp_types::{Diagnostic, MessageType, NumberOrString, Uri};
use tower_lsp_server::Client;

pub static CLIENT: OnceLock<Arc<Client>> = OnceLock::new();

pub fn init_client(client: Client) {
    let client_arc = Arc::new(client);
    CLIENT
        .set(client_arc)
        .expect("client should only be initialized once");
}

/// Run an async operation with the client if available
///
/// This helper function encapsulates the common pattern of checking if the client
/// is available, then spawning a task to run an async operation with it.
fn with_client<F, Fut>(f: F)
where
    F: FnOnce(Arc<Client>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if let Some(client) = CLIENT.get().cloned() {
        tokio::spawn(async move {
            f(client).await;
        });
    }
}

pub fn log_message(message_type: MessageType, message: &str) {
    let message = message.to_string();
    with_client(move |client| async move {
        client.log_message(message_type, &message).await;
    });
}

pub fn show_message(message_type: MessageType, message: &str) {
    let message = message.to_string();
    with_client(move |client| async move {
        client.show_message(message_type, &message).await;
    });
}

pub fn publish_diagnostics(uri: &str, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
    let uri = match uri.parse::<Uri>() {
        Ok(uri) => uri,
        Err(e) => {
            eprintln!("Invalid URI for diagnostics: {uri} - {e}");
            return;
        }
    };

    with_client(move |client| async move {
        client.publish_diagnostics(uri, diagnostics, version).await;
    });
}

pub fn send_notification<N>(params: N::Params)
where
    N: Notification,
    N::Params: Send + 'static,
{
    with_client(move |client| async move {
        client.send_notification::<N>(params).await;
    });
}

/// Start progress reporting
pub fn start_progress(
    token: impl Into<NumberOrString> + Send + 'static,
    title: &str,
    message: Option<String>,
) {
    let token = token.into();
    let title = title.to_string();

    with_client(move |client| async move {
        let progress = client.progress(token, title);

        // Add optional message if provided
        let progress = if let Some(msg) = message {
            progress.with_message(msg)
        } else {
            progress
        };

        // Begin the progress reporting
        let _ = progress.begin().await;
    });
}

/// Report progress
pub fn report_progress(
    token: impl Into<NumberOrString> + Send + 'static,
    title: &str,
    message: Option<String>,
    percentage: Option<u32>,
) {
    let token = token.into();
    let title = title.to_string();

    with_client(move |client| async move {
        // First begin the progress
        let ongoing_progress = client.progress(token, title).begin().await;

        match (message, percentage) {
            (Some(msg), Some(_pct)) => {
                // Both message and percentage - can't easily represent both with unbounded
                ongoing_progress.report(msg).await;
            }
            (Some(msg), None) => {
                // Only message
                ongoing_progress.report(msg).await;
            }
            (None, Some(_pct)) => {
                // Only percentage - not supported in unbounded progress
                // We'd need to use bounded progress with percentage
            }
            (None, None) => {
                // Nothing to report
            }
        }
    });
}

/// End progress reporting
pub fn end_progress(
    token: impl Into<NumberOrString> + Send + 'static,
    title: &str,
    message: Option<String>,
) {
    let token = token.into();
    let title = title.to_string();

    with_client(move |client| async move {
        let ongoing_progress = client.progress(token, title).begin().await;

        if let Some(msg) = message {
            ongoing_progress.finish_with_message(msg).await;
        } else {
            ongoing_progress.finish().await;
        }
    });
}

/// States for progress tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    NotStarted = 0,
    Started = 1,
    Finished = 2,
}

/// A handle for managing progress reporting with lifecycle tracking
#[derive(Clone)]
pub struct ProgressHandle {
    client: Arc<Client>,
    token: NumberOrString,
    title: String,
    state: Arc<AtomicU8>,
}

impl ProgressHandle {
    /// Create a new progress operation and return a handle to it
    pub fn new(token: impl Into<NumberOrString>, title: &str) -> Option<Self> {
        let token = token.into();
        let title = title.to_string();

        CLIENT.get().cloned().map(|client| {
            // Create the handle
            let handle = Self {
                client: client.clone(),
                token: token.clone(),
                title: title.clone(),
                state: Arc::new(AtomicU8::new(ProgressState::NotStarted as u8)),
            };

            // Clone for the closure
            let handle_clone = handle.clone();

            // Start the progress and update state
            tokio::spawn(async move {
                let _ = client.progress(token, title).begin().await;
                handle_clone.update_state(ProgressState::Started);
            });

            handle
        })
    }

    /// Update the progress state
    fn update_state(&self, state: ProgressState) {
        self.state.store(state as u8, Ordering::SeqCst);
    }

    /// Get the current progress state
    pub fn state(&self) -> ProgressState {
        match self.state.load(Ordering::SeqCst) {
            0 => ProgressState::NotStarted,
            1 => ProgressState::Started,
            _ => ProgressState::Finished,
        }
    }

    /// Check if already finished - returns true if already finished
    fn is_finished(&self) -> bool {
        self.state
            .swap(ProgressState::Finished as u8, Ordering::SeqCst)
            == ProgressState::Finished as u8
    }

    /// Report progress with a message
    pub fn report(&self, message: impl Into<String>) {
        // Only report if in Started state
        if self.state() != ProgressState::Started {
            return;
        }

        let token = self.token.clone();
        let title = self.title.clone();
        let message = message.into();
        let client = self.client.clone();

        tokio::spawn(async move {
            let ongoing = client.progress(token, title).begin().await;
            ongoing.report(message).await;
        });
    }

    /// Complete the progress
    pub fn finish(self, message: impl Into<String>) {
        // Only finish if not already finished
        if self.is_finished() {
            return;
        }

        let token = self.token.clone();
        let title = self.title.clone();
        let message = message.into();
        let client = self.client.clone();

        tokio::spawn(async move {
            let ongoing = client.progress(token, title).begin().await;
            ongoing.finish_with_message(message).await;
        });
    }

    /// Complete the progress without a message
    pub fn finish_without_message(self) {
        // Only finish if not already finished
        if self.is_finished() {
            return;
        }

        let client = self.client.clone();
        let token = self.token.clone();
        let title = self.title.clone();

        tokio::spawn(async move {
            let ongoing = client.progress(token, title).begin().await;
            ongoing.finish().await;
        });
    }
}

/// Send a custom request to the client using a specific LSP request type
///
/// Unlike other methods, this one needs to be async since it returns a result.
pub async fn send_request<R>(params: R::Params) -> Result<R::Result, Error>
where
    R: tower_lsp_server::lsp_types::request::Request,
    R::Params: Send,
    R::Result: Send,
{
    if let Some(client) = CLIENT.get() {
        client.send_request::<R>(params).await
    } else {
        Err(Error::internal_error())
    }
}

/// Show an info message in the client's UI
pub fn info(message: &str) {
    show_message(MessageType::INFO, message);
}

/// Show a warning message in the client's UI
pub fn warn(message: &str) {
    show_message(MessageType::WARNING, message);
}

/// Show an error message in the client's UI
pub fn error(message: &str) {
    show_message(MessageType::ERROR, message);
}

/// Log an info message to the client's log
pub fn log_info(message: &str) {
    log_message(MessageType::INFO, message);
}

/// Log a warning message to the client's log
pub fn log_warn(message: &str) {
    log_message(MessageType::WARNING, message);
}

/// Log an error message to the client's log
pub fn log_error(message: &str) {
    log_message(MessageType::ERROR, message);
}

/// Clear all diagnostics for a file by publishing an empty array
pub fn clear_diagnostics(uri: &str) {
    publish_diagnostics(uri, vec![], None);
}
