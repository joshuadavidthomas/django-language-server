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

fn get_client() -> Option<Arc<Client>> {
    CLIENT.get().cloned()
}

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

pub mod diagnostics {
    use tower_lsp_server::lsp_types::Diagnostic;
    use tower_lsp_server::lsp_types::Uri;

    use super::get_client;

    notify!(publish_diagnostics, uri: Uri, diagnostics: Vec<Diagnostic>, version: Option<i32>);
    notify_discard!(workspace_diagnostic_refresh,);
}

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

pub mod editor {
    use super::get_client;

    notify_discard!(code_lens_refresh,);
    notify_discard!(semantic_tokens_refresh,);
    notify_discard!(inline_value_refresh,);
    notify_discard!(inlay_hint_refresh,);
}

pub mod capabilities {
    use tower_lsp_server::lsp_types::Registration;
    use tower_lsp_server::lsp_types::Unregistration;

    use super::get_client;

    notify_discard!(register_capability, registrations: Vec<Registration>);
    notify_discard!(unregister_capability, unregisterations: Vec<Unregistration>);
}

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
