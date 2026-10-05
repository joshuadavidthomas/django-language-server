use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::jsonrpc;
use tower_lsp_server::ls_types;

/// A minimal server whose only purpose is handing tests a real `Client`, so
/// they can observe exactly what DJLS writes to the transport.
pub(crate) struct TransportBackend(pub(crate) Client);

impl LanguageServer for TransportBackend {
    async fn initialize(
        &self,
        _: ls_types::InitializeParams,
    ) -> jsonrpc::Result<ls_types::InitializeResult> {
        Ok(ls_types::InitializeResult::default())
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }
}
