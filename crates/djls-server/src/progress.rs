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
    state: Option<ProgressState>,
}

enum ProgressState {
    Lsp {
        client: Client,
        token: ls_types::ProgressToken,
    },
    Log {
        title: String,
    },
}

impl ProgressReporter {
    pub(crate) fn new(client: Client, info: ClientInfo) -> Self {
        Self { client, info }
    }

    pub(crate) async fn begin(&self, title: &str) -> ProgressItem {
        let title = title.to_string();

        if !self.info.supports_work_done_progress() {
            tracing::info!("{title}");
            return ProgressItem {
                state: Some(ProgressState::Log { title }),
            };
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
            drop(created_tx.send(result));
        });

        match tokio::time::timeout(CREATE_PROGRESS_TIMEOUT, created_rx).await {
            Ok(Ok(Ok(()))) => {
                send_begin(&self.client, token.clone(), title).await;
                ProgressItem {
                    state: Some(ProgressState::Lsp {
                        client: self.client.clone(),
                        token,
                    }),
                }
            }
            Ok(Ok(Err(error))) => {
                tracing::debug!(?error, title, "Work-done progress unavailable");
                tracing::info!("{title}");
                ProgressItem {
                    state: Some(ProgressState::Log { title }),
                }
            }
            Ok(Err(_)) => {
                tracing::debug!(title, "Progress creation task was cancelled");
                tracing::info!("{title}");
                ProgressItem {
                    state: Some(ProgressState::Log { title }),
                }
            }
            Err(_) => {
                tracing::debug!(
                    title,
                    timeout_ms = CREATE_PROGRESS_TIMEOUT.as_millis(),
                    "Timed out creating work-done progress"
                );
                tracing::info!("{title}");
                ProgressItem {
                    state: Some(ProgressState::Log { title }),
                }
            }
        }
    }
}

impl ProgressItem {
    pub(crate) async fn report(&self, message: &str) {
        self.send_report(message.to_string(), None).await;
    }

    pub(crate) async fn report_fraction(&self, done: usize, total: usize, message: &str) {
        let message = if total == 0 {
            format!("{done} {message}")
        } else {
            format!("{done}/{total} {message}")
        };
        let percentage = done
            .saturating_mul(100)
            .checked_div(total)
            .map(|value| value.min(100))
            .and_then(|value| u32::try_from(value).ok());

        self.send_report(message, percentage).await;
    }

    async fn send_report(&self, message: String, percentage: Option<u32>) {
        match self.state.as_ref() {
            Some(ProgressState::Lsp { client, token }) => {
                send_report(client, token.clone(), message, percentage).await;
            }
            Some(ProgressState::Log { title }) => {
                tracing::info!("{title}: {message}");
            }
            None => {}
        }
    }

    pub(crate) async fn finish(mut self, message: &str) {
        match self.state.take() {
            Some(ProgressState::Lsp { client, token }) => {
                send_end(&client, token, Some(message.to_string())).await;
            }
            Some(ProgressState::Log { title }) => {
                tracing::info!("{title}: {message}");
            }
            None => {}
        }
    }
}

impl Drop for ProgressItem {
    fn drop(&mut self) {
        let Some(ProgressState::Lsp { client, token }) = self.state.take() else {
            return;
        };

        // Only LSP items that sent Begin owe the client an End. Drop without
        // finish() means the progress owner future itself was dropped
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

async fn send_report(
    client: &Client,
    token: ls_types::ProgressToken,
    message: String,
    percentage: Option<u32>,
) {
    client
        .send_notification::<ProgressNotification>(ls_types::ProgressParams {
            token,
            value: ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::Report(
                ls_types::WorkDoneProgressReport {
                    cancellable: None,
                    message: Some(message),
                    percentage,
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

#[cfg(test)]
mod tests {
    use futures_util::SinkExt;
    use futures_util::StreamExt;
    use tower_lsp_server::LanguageServer;
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc;
    use tower_service::Service;

    use super::*;
    use crate::client::ClientOptions;

    struct TransportBackend(Client);

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

    #[tokio::test]
    async fn unavailable_progress_falls_back_to_info_tracing() {
        let log = tempfile::NamedTempFile::new().expect("log file");
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_writer(std::sync::Mutex::new(log.reopen().expect("log writer")))
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);

        for mode in ["unsupported", "rejected", "timeout", "cancelled"] {
            let (mut service, socket) = LspService::new(TransportBackend);
            service
                .call(
                    jsonrpc::Request::build("initialize")
                        .params(serde_json::json!({"capabilities": {}}))
                        .id(1_i64)
                        .finish(),
                )
                .await
                .expect("initialize");
            let capabilities = ls_types::ClientCapabilities {
                window: Some(ls_types::WindowClientCapabilities {
                    work_done_progress: Some(mode != "unsupported"),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let reporter = ProgressReporter::new(
                service.inner().0.clone(),
                ClientInfo::new(&capabilities, None, ClientOptions::default()),
            );
            let (mut requests, mut responses) = socket.split();
            let begin = reporter.begin("test progress");
            let respond = async {
                if mode == "unsupported" {
                    return;
                }
                let request = requests.next().await.expect("progress create request");
                assert_eq!(request.method(), "window/workDoneProgress/create");
                if mode == "rejected" || mode == "cancelled" {
                    let error = if mode == "cancelled" {
                        jsonrpc::Error::request_cancelled()
                    } else {
                        jsonrpc::Error::method_not_found()
                    };
                    responses
                        .send(jsonrpc::Response::from_error(
                            request.id().expect("request id").clone(),
                            error,
                        ))
                        .await
                        .expect("progress rejection");
                }
            };
            let (item, ()) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(begin, respond)
            })
            .await
            .expect("progress creation must terminate");
            assert!(
                matches!(item.state, Some(ProgressState::Log { .. })),
                "{mode}"
            );
            item.report("report").await;
            item.report_fraction(2, 7, "files").await;
            item.finish("finished").await;
            assert!(futures_util::poll!(requests.next()).is_pending());
        }

        let output = std::fs::read_to_string(log.path()).expect("read log");
        for expected in [
            "test progress",
            "test progress: report",
            "test progress: 2/7 files",
            "test progress: finished",
        ] {
            assert!(
                output.contains(expected),
                "Missing {expected:?} in {output}"
            );
        }
    }
}
