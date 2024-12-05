use anyhow::Result;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use djls_python::PythonEnvironment;

#[derive(Debug)]
struct Backend {
    client: Client,
    python: PythonEnvironment,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> LspResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                ..Default::default()
            },
            offset_encoding: None,
            server_info: Some(ServerInfo {
                name: String::from("Django Language Server"),
                version: Some(String::from("0.1.0")),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "server initialized!")
            .await;

        self.client
            .log_message(MessageType::INFO, format!("\n{}", self.python))
            .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let python = PythonEnvironment::initialize()?;

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend { client, python }).finish();

    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}
