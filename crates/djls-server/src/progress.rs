use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use tower_lsp_server::Client;
use tower_lsp_server::ls_types;
use tower_lsp_server::ls_types::notification::Progress as ProgressNotification;

use crate::client::ClientInfo;

static NEXT_PROGRESS_TOKEN: AtomicU64 = AtomicU64::new(1);

pub(crate) struct LoadProgress {
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

impl LoadProgress {
    pub(crate) async fn begin(client: Client, info: &ClientInfo, title: &str) -> Self {
        let title = title.to_string();

        if !info.supports_work_done_progress() {
            tracing::info!("{title}");
            return Self {
                title,
                state: Some(ProgressState::Log),
            };
        }

        let token = ls_types::ProgressToken::String(format!(
            "djls-load-{}",
            // Uniqueness is the only invariant; no cross-thread data is
            // synchronized through this counter.
            NEXT_PROGRESS_TOKEN.fetch_add(1, Ordering::Relaxed)
        ));

        // This awaits the client's response, gating the (sequential) refresh
        // queue on it. tower-lsp-server requests have no timeout, so a client
        // that advertises the capability but never answers would stall the
        // refresh — accepted because clients answer create requests
        // immediately, and a hung client breaks far more than progress.
        match client.create_work_done_progress(token.clone()).await {
            Ok(()) => {
                send_begin(&client, token.clone(), title.clone()).await;
                Self {
                    title,
                    state: Some(ProgressState::Lsp { client, token }),
                }
            }
            Err(error) => {
                tracing::debug!(?error, "Falling back to log-only project load progress");
                tracing::info!("{title}");
                Self {
                    title,
                    state: Some(ProgressState::Log),
                }
            }
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

impl Drop for LoadProgress {
    fn drop(&mut self) {
        let Some(ProgressState::Lsp { client, token }) = self.state.take() else {
            return;
        };

        // Drop without finish() means the refresh future itself was dropped
        // (cancellation or unwind), not a normal supersede.
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
