use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tower_lsp_server::Client;
use tower_lsp_server::ls_types;
use tower_lsp_server::ls_types::notification::Progress as ProgressNotification;

use crate::client::ClientInfo;

const CREATE_PROGRESS_TIMEOUT: Duration = Duration::from_secs(2);

static NEXT_PROGRESS_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct ProgressReporter {
    client: Client,
    info: ClientInfo,
}

pub(crate) struct ProgressItem {
    title: String,
    state: Option<ProgressState>,
}

enum ProgressState {
    Lsp {
        client: Client,
        token: ls_types::ProgressToken,
    },
    Log,
}

impl ProgressReporter {
    pub(crate) fn new(client: Client, info: ClientInfo) -> Self {
        Self { client, info }
    }

    pub(crate) async fn begin(&self, title: &str) -> ProgressItem {
        let title = title.to_string();

        if !self.info.supports_work_done_progress() {
            tracing::info!("{title}");
            return ProgressItem::log(title);
        }

        let token = ls_types::ProgressToken::String(format!(
            "djls-load-{}",
            // Uniqueness is the only invariant; no cross-thread data is
            // synchronized through this counter.
            NEXT_PROGRESS_TOKEN.fetch_add(1, Ordering::Relaxed)
        ));

        let (created_tx, created_rx) = tokio::sync::oneshot::channel();
        let create_client = self.client.clone();
        let create_token = token.clone();
        tokio::spawn(async move {
            let result = create_client.create_work_done_progress(create_token).await;
            let _ = created_tx.send(result);
        });

        match tokio::time::timeout(CREATE_PROGRESS_TIMEOUT, created_rx).await {
            Ok(Ok(Ok(()))) => {
                send_begin(&self.client, token.clone(), title.clone()).await;
                ProgressItem {
                    title,
                    state: Some(ProgressState::Lsp {
                        client: self.client.clone(),
                        token,
                    }),
                }
            }
            Ok(Ok(Err(error))) => {
                tracing::debug!(?error, title, "Falling back to log-only progress");
                tracing::info!("{title}");
                ProgressItem::log(title)
            }
            Ok(Err(_)) => {
                tracing::debug!(title, "Progress creation task was cancelled");
                tracing::info!("{title}");
                ProgressItem::log(title)
            }
            Err(_) => {
                tracing::debug!(
                    title,
                    timeout_ms = CREATE_PROGRESS_TIMEOUT.as_millis(),
                    "Timed out creating work-done progress; falling back to log-only progress"
                );
                tracing::info!("{title}");
                ProgressItem::log(title)
            }
        }
    }
}

impl ProgressItem {
    fn log(title: String) -> Self {
        Self {
            title,
            state: Some(ProgressState::Log),
        }
    }

    pub(crate) async fn report(&self, message: &str) {
        match self.state.as_ref() {
            Some(ProgressState::Lsp { client, token }) => {
                send_report(client, token.clone(), message.to_string()).await;
            }
            Some(ProgressState::Log) | None => {
                tracing::info!("{}: {message}", self.title);
            }
        }
    }

    pub(crate) async fn finish(mut self, message: &str) {
        let state = self.state.take();
        match state {
            Some(ProgressState::Lsp { client, token }) => {
                send_end(&client, token, Some(message.to_string())).await;
            }
            Some(ProgressState::Log) | None => {
                tracing::info!("{}: {message}", self.title);
            }
        }
    }
}

impl Drop for ProgressItem {
    fn drop(&mut self) {
        let Some(ProgressState::Lsp { client, token }) = self.state.take() else {
            return;
        };

        // Only LSP items that sent Begin owe the client an End. Drop without
        // finish() means the refresh future itself was dropped (cancellation or
        // unwind), not a normal supersede.
        tokio::spawn(async move {
            send_end(&client, token, Some("cancelled".to_string())).await;
        });
    }
}

async fn send_begin(client: &Client, token: ls_types::ProgressToken, title: String) {
    client
        .send_notification::<ProgressNotification>(ls_types::ProgressParams {
            token,
            value: ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::Begin(
                ls_types::WorkDoneProgressBegin {
                    title,
                    cancellable: Some(false),
                    message: None,
                    percentage: None,
                },
            )),
        })
        .await;
}

async fn send_report(client: &Client, token: ls_types::ProgressToken, message: String) {
    client
        .send_notification::<ProgressNotification>(ls_types::ProgressParams {
            token,
            value: ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::Report(
                ls_types::WorkDoneProgressReport {
                    cancellable: None,
                    message: Some(message),
                    percentage: None,
                },
            )),
        })
        .await;
}

async fn send_end(client: &Client, token: ls_types::ProgressToken, message: Option<String>) {
    client
        .send_notification::<ProgressNotification>(ls_types::ProgressParams {
            token,
            value: ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::End(
                ls_types::WorkDoneProgressEnd { message },
            )),
        })
        .await;
}
