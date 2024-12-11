use anyhow::Result;
use tower_lsp::async_trait;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::MessageActionItem;
use tower_lsp::lsp_types::MessageType;
use tower_lsp::lsp_types::Url;
use tower_lsp::Client;

#[async_trait]
pub trait Notifier: Send + Sync {
    fn log_message(&self, typ: MessageType, msg: &str) -> Result<()>;
    fn show_message(&self, typ: MessageType, msg: &str) -> Result<()>;
    async fn show_message_request(
        &self,
        typ: MessageType,
        msg: &str,
        actions: Option<Vec<MessageActionItem>>,
    ) -> Result<Option<MessageActionItem>>;
    fn publish_diagnostics(
        &self,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    ) -> Result<()>;
}

pub struct TowerLspNotifier {
    client: Client,
}

impl TowerLspNotifier {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Notifier for TowerLspNotifier {
    fn log_message(&self, typ: MessageType, msg: &str) -> Result<()> {
        let client = self.client.clone();
        let msg = msg.to_string();
        tokio::spawn(async move {
            client.log_message(typ, msg).await;
        });
        Ok(())
    }

    fn show_message(&self, typ: MessageType, msg: &str) -> Result<()> {
        let client = self.client.clone();
        let msg = msg.to_string();
        tokio::spawn(async move {
            client.show_message(typ, msg).await;
        });
        Ok(())
    }

    async fn show_message_request(
        &self,
        typ: MessageType,
        msg: &str,
        actions: Option<Vec<MessageActionItem>>,
    ) -> Result<Option<MessageActionItem>> {
        let client = self.client.clone();
        let msg = msg.to_string();
        Ok(client.show_message_request(typ, msg, actions).await?)
    }

    fn publish_diagnostics(
        &self,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    ) -> Result<()> {
        let client = self.client.clone();
        tokio::spawn(async move {
            client.publish_diagnostics(uri, diagnostics, version).await;
        });
        Ok(())
    }
}
