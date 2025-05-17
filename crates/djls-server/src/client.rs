use std::fmt::Display;
use std::sync::Arc;
use std::sync::OnceLock;

use tower_lsp_server::jsonrpc::Error;
use tower_lsp_server::Client;

static CLIENT: OnceLock<Arc<Client>> = OnceLock::new();

pub fn init_client(client: Client) {
    let client_arc = Arc::new(client);
    CLIENT
        .set(client_arc)
        .expect("client should only be initialized once");
}

fn get_client() -> Option<Arc<Client>> {
    CLIENT.get().cloned()
}

macro_rules! notify {
      ($method:ident, $($arg:expr),*) => {
          if let Some(client) = get_client() {
              tokio::spawn(async move {
                  client.$method($($arg),*).await;
              });
          }
      };
  }

macro_rules! notify_discard {
      ($method:ident, $($arg:expr),*) => {
          if let Some(client) = get_client() {
              tokio::spawn(async move {
                  let _ = client.$method($($arg),*).await;
              });
          }
      };
  }

macro_rules! request {
      ($method:ident, $($arg:expr),*) => {
          if let Some(client) = get_client() {
              client.$method($($arg),*).await
          } else {
              Err(Error::internal_error())
          }
      };
  }

pub mod messages {
    use tower_lsp_server::lsp_types::MessageActionItem;
    use tower_lsp_server::lsp_types::MessageType;
    use tower_lsp_server::lsp_types::ShowDocumentParams;

    use super::*;

    pub fn log_message(message_type: MessageType, message: impl Display + Send + 'static) {
        notify!(log_message, message_type, message);
    }

    pub fn show_message(message_type: MessageType, message: impl Display + Send + 'static) {
        notify!(show_message, message_type, message);
    }

    pub async fn show_message_request(
        message_type: MessageType,
        message: impl Display + Send + 'static,
        actions: Option<Vec<MessageActionItem>>,
    ) -> Result<Option<MessageActionItem>, Error> {
        request!(show_message_request, message_type, message, actions)
    }

    pub async fn show_document(params: ShowDocumentParams) -> Result<bool, Error> {
        request!(show_document, params)
    }
}

pub mod diagnostics {
    use tower_lsp_server::lsp_types::Diagnostic;
    use tower_lsp_server::lsp_types::Uri;

    use super::*;

    pub fn publish_diagnostics(uri: Uri, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                client.publish_diagnostics(uri, diagnostics, version).await;
            });
        }
    }

    pub fn workspace_diagnostic_refresh() {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                let _ = client.workspace_diagnostic_refresh().await;
            });
        }
    }
}

pub mod workspace {
    use tower_lsp_server::lsp_types::ApplyWorkspaceEditResponse;
    use tower_lsp_server::lsp_types::ConfigurationItem;
    use tower_lsp_server::lsp_types::LSPAny;
    use tower_lsp_server::lsp_types::WorkspaceEdit;
    use tower_lsp_server::lsp_types::WorkspaceFolder;

    use super::*;

    pub async fn apply_edit(edit: WorkspaceEdit) -> Result<ApplyWorkspaceEditResponse, Error> {
        if let Some(client) = get_client() {
            client.apply_edit(edit).await
        } else {
            Err(Error::internal_error())
        }
    }

    pub async fn configuration(items: Vec<ConfigurationItem>) -> Result<Vec<LSPAny>, Error> {
        if let Some(client) = get_client() {
            client.configuration(items).await
        } else {
            Err(Error::internal_error())
        }
    }

    pub async fn workspace_folders() -> Result<Option<Vec<WorkspaceFolder>>, Error> {
        if let Some(client) = get_client() {
            client.workspace_folders().await
        } else {
            Err(Error::internal_error())
        }
    }
}

pub mod editor {
    use super::*;

    pub fn code_lens_refresh() {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                let _ = client.code_lens_refresh().await;
            });
        }
    }

    pub fn semantic_tokens_refresh() {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                let _ = client.semantic_tokens_refresh().await;
            });
        }
    }

    pub fn inline_value_refresh() {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                let _ = client.inline_value_refresh().await;
            });
        }
    }

    pub fn inlay_hint_refresh() {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                let _ = client.inlay_hint_refresh().await;
            });
        }
    }
}

pub mod capabilities {
    use tower_lsp_server::lsp_types::Registration;
    use tower_lsp_server::lsp_types::Unregistration;

    use super::*;

    pub fn register_capability(registrations: Vec<Registration>) {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                let _ = client.register_capability(registrations).await;
            });
        }
    }

    pub fn unregister_capability(unregisterations: Vec<Unregistration>) {
        if let Some(client) = get_client() {
            tokio::spawn(async move {
                let _ = client.unregister_capability(unregisterations).await;
            });
        }
    }
}

pub mod monitoring {
    use serde::Serialize;
    use tower_lsp_server::lsp_types::ProgressToken;
    use tower_lsp_server::Progress;

    use super::*;

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

pub mod protocol {
    use tower_lsp_server::lsp_types::notification::Notification;
    use tower_lsp_server::lsp_types::request::Request;

    use super::*;

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
