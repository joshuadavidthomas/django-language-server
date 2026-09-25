//! A `window/showMessage` popup when a project reload fails.
//!
//! One popup per failure streak, not per failed reload: while the project
//! stays broken, every save can trigger another failed reload, and repeating
//! the popup each time would be noise. The log still records every failure.

use std::sync::Arc;

use tokio::spawn as spawn_task;
use tokio::sync::Mutex;
use tokio::sync::watch;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types::MessageType;
use tracing::Instrument;
use tracing::debug;
use tracing::instrument::WithSubscriber;

use crate::session::IntrinsicReadinessState;
use crate::session::Session;

const MESSAGE: &str = "Django project reload failed. Check your Django language server \
                       configuration; see the server log for the failing phase.";

#[derive(Clone, Copy, Default)]
struct FailureState {
    /// The latest failed generation, or `None` after a successful reload.
    generation: Option<u64>,
    /// Incremented on each transition from healthy to failed.
    streak: u64,
}

#[derive(Clone)]
pub(super) struct ReloadFailureNotice {
    state: watch::Sender<FailureState>,
}

impl ReloadFailureNotice {
    pub(super) fn new(session: &Arc<Mutex<Session>>, client: Client) -> Self {
        let (state, mut changes) = watch::channel(FailureState::default());
        let session = Arc::downgrade(session);
        spawn_task(
            async move {
                // A watch channel keeps only the latest state, so a stalled popup
                // bounds pending work to one value and a newer failure always
                // replaces an older one instead of being dropped.
                let mut shown_streak = 0;
                while changes.changed().await.is_ok() {
                    let FailureState { generation, streak } = *changes.borrow_and_update();
                    let Some(generation) = generation else {
                        continue;
                    };
                    if streak == shown_streak {
                        debug!(
                            generation,
                            outcome = "skipped",
                            "Reload failure already shown"
                        );
                        continue;
                    }
                    let Some(session) = session.upgrade() else {
                        return;
                    };
                    let current = session.lock().await.readiness_state()
                        == IntrinsicReadinessState::Failed(generation);
                    drop(session);
                    if !current {
                        // Newer work superseded this failure; if it fails too, the
                        // same streak is still unshown and will be shown then.
                        debug!(generation, outcome = "stale", "Reload failure superseded");
                        continue;
                    }
                    shown_streak = streak;
                    // Tower accepting the send is not proof of display.
                    client
                        .show_message(MessageType::ERROR, MESSAGE)
                        .instrument(tracing::debug_span!(
                            parent: None,
                            "project.failure_notice",
                            generation
                        ))
                        .await;
                    debug!(generation, outcome = "success", "Reload failure shown");
                }
            }
            .with_current_subscriber(),
        );
        Self { state }
    }

    pub(super) fn failed(&self, generation: u64) {
        self.state.send_modify(|state| {
            if state.generation.is_none() {
                state.streak += 1;
            }
            state.generation = Some(generation);
        });
    }

    pub(super) fn recovered(&self) {
        self.state.send_if_modified(|state| {
            state.generation = None;
            false
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::StreamExt;
    use tower_lsp_server::LanguageServer;
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc;
    use tower_lsp_server::ls_types;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::logging::capture::Capture;
    use crate::logging::capture::callsite_guard;

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

    async fn next_outcome(capture: &Capture, seen: &mut usize) -> (u64, String) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(event) = capture.0.lock().expect("capture lock").get(*seen) {
                    *seen += 1;
                    if let Some(outcome) = event["fields"]["outcome"].as_str() {
                        return (
                            event["fields"]["generation"].as_u64().expect("generation"),
                            outcome.to_string(),
                        );
                    }
                    continue;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("notice worker outcome")
    }

    async fn fail_next_generation(session: &Arc<Mutex<Session>>) -> u64 {
        let mut session = session.lock().await;
        session.mark_project_changed();
        let generation = session.desired_generation();
        assert!(session.fail_intrinsic_readiness(generation));
        generation
    }

    #[tokio::test]
    async fn one_notice_per_failure_streak_and_superseded_failures_wait() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        let _subscriber =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(capture.clone()));
        let (service, mut socket) = LspService::new(TransportBackend);
        let session = Arc::new(Mutex::new(Session::default()));
        let notice = ReloadFailureNotice::new(&session, service.inner().0.clone());
        let mut seen = 0;

        // A failure already superseded by newer work is not shown yet...
        let stale = fail_next_generation(&session).await;
        session.lock().await.mark_project_changed();
        notice.failed(stale);
        assert_eq!(
            next_outcome(&capture, &mut seen).await,
            (stale, "stale".into())
        );

        // ...but the newer work failing in the same streak is.
        let first = fail_next_generation(&session).await;
        notice.failed(first);
        assert_eq!(
            next_outcome(&capture, &mut seen).await,
            (first, "success".into())
        );
        let message = socket.next().await.expect("show message");
        assert_eq!(message.method(), "window/showMessage");

        // Further failures before a recovery repeat nothing.
        let repeat = fail_next_generation(&session).await;
        notice.failed(repeat);
        assert_eq!(
            next_outcome(&capture, &mut seen).await,
            (repeat, "skipped".into())
        );

        notice.recovered();
        let next_streak = fail_next_generation(&session).await;
        notice.failed(next_streak);
        assert_eq!(
            next_outcome(&capture, &mut seen).await,
            (next_streak, "success".into())
        );
        let message = socket.next().await.expect("show message");
        assert_eq!(message.method(), "window/showMessage");
    }

    #[tokio::test]
    async fn newest_failure_replaces_pending_one() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        let _subscriber =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(capture.clone()));
        let (service, _socket) = LspService::new(TransportBackend);
        let session = Arc::new(Mutex::new(Session::default()));
        let notice = ReloadFailureNotice::new(&session, service.inner().0.clone());
        // The current-thread runtime doesn't run the worker until this test
        // yields, as if it were stalled sending an earlier popup.
        notice.failed(5);
        notice.failed(6);
        notice.failed(7);
        let latest = fail_next_generation(&session).await;
        notice.failed(latest);

        let mut seen = 0;
        assert_eq!(
            next_outcome(&capture, &mut seen).await,
            (latest, "success".into())
        );
        assert!(
            capture
                .0
                .lock()
                .expect("capture lock")
                .iter()
                .all(|event| event["fields"]["generation"] == latest)
        );
    }
}
