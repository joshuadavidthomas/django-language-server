use std::fmt::Display;
use std::sync::Arc;
use std::sync::OnceLock;

pub use messages::*;
use tower_lsp_server::jsonrpc::Error;
use tower_lsp_server::Client;

static CLIENT: OnceLock<Arc<Client>> = OnceLock::new();

pub fn init_client(client: Client) {
    let client_arc = Arc::new(client);
    CLIENT
        .set(client_arc)
        .expect("client should only be initialized once");
}

pub fn get_client() -> Option<Arc<Client>> {
    CLIENT.get().cloned()
}

/// Generates a fire-and-forget notification function that spawns an async task.
///
/// This macro creates a wrapper function that:
/// 1. Gets the global client instance
/// 2. Spawns a new Tokio task that calls the client method asynchronously
/// 3. Does not wait for completion or handle errors
///
/// This...
/// ```rust,ignore
/// notify!(log_message, message_type: MessageType, message: impl Display + Send + 'static);
/// ```
///
/// ...expands to:
/// ```rust,ignore
/// pub fn log_message(message_type: MessageType, message: impl Display + Send + 'static) {
///     if let Some(client) = get_client() {
///         tokio::spawn(async move {
///             client.log_message(message_type, message).await;
///         });
///     }
/// }
/// ```
macro_rules! notify {
    ($name:ident, $($param:ident: $type:ty),*) => {
        pub fn $name($($param: $type),*) {
            if let Some(client) = get_client() {
                tokio::spawn(async move {
                    client.$name($($param),*).await;
                });
            }
        }
    };
}

/// Generates a fire-and-forget notification function that spawns an async task and discards any errors.
///
/// Similar to `notify!`, but explicitly discards any errors returned by the client method.
/// This is useful for methods that might return a Result but where you don't care about the outcome.
///
/// This...
/// ```rust,ignore
/// notify_discard!(code_lens_refresh,);
/// ```
///
/// ...expands to:
/// ```rust,ignore
/// pub fn code_lens_refresh() {
///     if let Some(client) = get_client() {
///         tokio::spawn(async move {
///             let _ = client.code_lens_refresh().await;
///         });
///     }
/// }
/// ```
macro_rules! notify_discard {
    ($name:ident, $($param:ident: $type:ty),*) => {
        pub fn $name($($param: $type),*) {
            if let Some(client) = get_client() {
                tokio::spawn(async move {
                    let _ = client.$name($($param),*).await;
                });
            }
        }
    };
}

/// Generates an async request function that awaits a response from the client.
///
/// Unlike the notification macros, this creates a function that:
/// 1. Is marked as `async` and must be awaited
/// 2. Returns a `Result<T, Error>` with the response type
/// 3. Fails with an internal error if the client is not available
///
/// The semi-colon (`;`) separates the parameters from the return type.
///
/// This...
/// ```rust,ignore
/// request!(show_document, params: ShowDocumentParams ; bool);
/// ```
///
/// ...expands to:
/// ```rust,ignore
/// pub async fn show_document(params: ShowDocumentParams) -> Result<bool, Error> {
///     if let Some(client) = get_client() {
///         client.show_document(params).await
///     } else {
///         Err(Error::internal_error())
///     }
/// }
/// ```
macro_rules! request {
    ($name:ident, $($param:ident: $type:ty),* ; $result:ty) => {
        pub async fn $name($($param: $type),*) -> Result<$result, Error> {
            if let Some(client) = get_client() {
                client.$name($($param),*).await
            } else {
                Err(Error::internal_error())
            }
        }
    };
}

#[allow(dead_code)]
pub mod messages {
    use tower_lsp_server::lsp_types::MessageActionItem;
    use tower_lsp_server::lsp_types::MessageType;
    use tower_lsp_server::lsp_types::ShowDocumentParams;

    use super::get_client;
    use super::Display;
    use super::Error;

    notify!(log_message, message_type: MessageType, message: impl Display + Send + 'static);
    notify!(show_message, message_type: MessageType, message: impl Display + Send + 'static);
    request!(show_message_request, message_type: MessageType, message: impl Display + Send + 'static, actions: Option<Vec<MessageActionItem>> ; Option<MessageActionItem>);
    request!(show_document, params: ShowDocumentParams ; bool);
}

#[allow(dead_code)]
pub mod diagnostics {
    use tower_lsp_server::lsp_types::Diagnostic;
    use tower_lsp_server::lsp_types::Uri;

    use super::get_client;

    notify!(publish_diagnostics, uri: Uri, diagnostics: Vec<Diagnostic>, version: Option<i32>);
    notify_discard!(workspace_diagnostic_refresh,);
}

#[allow(dead_code)]
pub mod workspace {
    use tower_lsp_server::lsp_types::ApplyWorkspaceEditResponse;
    use tower_lsp_server::lsp_types::ConfigurationItem;
    use tower_lsp_server::lsp_types::LSPAny;
    use tower_lsp_server::lsp_types::WorkspaceEdit;
    use tower_lsp_server::lsp_types::WorkspaceFolder;

    use super::get_client;
    use super::Error;

    request!(apply_edit, edit: WorkspaceEdit ; ApplyWorkspaceEditResponse);
    request!(configuration, items: Vec<ConfigurationItem> ; Vec<LSPAny>);
    request!(workspace_folders, ; Option<Vec<WorkspaceFolder>>);
}

#[allow(dead_code)]
pub mod editor {
    use super::get_client;

    notify_discard!(code_lens_refresh,);
    notify_discard!(semantic_tokens_refresh,);
    notify_discard!(inline_value_refresh,);
    notify_discard!(inlay_hint_refresh,);
}

#[allow(dead_code)]
pub mod capabilities {
    use tower_lsp_server::lsp_types::Registration;
    use tower_lsp_server::lsp_types::Unregistration;

    use super::get_client;

    notify_discard!(register_capability, registrations: Vec<Registration>);
    notify_discard!(unregister_capability, unregisterations: Vec<Unregistration>);
}

#[allow(dead_code)]
pub mod monitoring {
    use serde::Serialize;
    use tower_lsp_server::lsp_types::ProgressToken;
    use tower_lsp_server::Progress;

    use super::get_client;

    pub fn telemetry_event<S: Serialize + Send + 'static>(data: S) {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                client.telemetry_event(data).await;
            });
        }
    }

    pub fn progress<T: Into<String> + Send>(token: ProgressToken, title: T) -> Option<Progress> {
        get_client().map(|client| client.progress(token, title))
    }
}

#[allow(dead_code)]
pub mod protocol {
    use tower_lsp_server::lsp_types::notification::Notification;
    use tower_lsp_server::lsp_types::request::Request;

    use super::get_client;
    use super::Error;

    pub fn send_notification<N>(params: N::Params)
    where
        N: Notification,
        N::Params: Send + 'static,
    {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                client.send_notification::<N>(params).await;
            });
        }
    }

    pub async fn send_request<R>(params: R::Params) -> Result<R::Result, Error>
    where
        R: Request,
        R::Params: Send + 'static,
        R::Result: Send + 'static,
    {
        if let Some(client) = get_client() {
            client.send_request::<R>(params).await
        } else {
            Err(Error::internal_error())
        }
    }
}
