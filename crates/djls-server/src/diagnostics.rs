//! Coalesced diagnostic delivery. Mutations enqueue under the session lock;
//! computation and transport never hold that lock or share a stale batch snapshot.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::path_to_file;
use salsa::Cancelled;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::task::JoinError;
use tokio::task::spawn_blocking;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;
use tracing::Instrument;
use tracing::debug;
use tracing::debug_span;
use tracing::error;

use crate::document::TextDocument;
use crate::ext::UriExt;
use crate::logging::blocking_in_span;
use crate::session::IntrinsicReadinessState;
use crate::session::Session;
use crate::session::SessionSnapshot;

#[derive(Default)]
pub(crate) struct DiagnosticQueue {
    next_ticket: u64,
    publications: BTreeMap<Utf8PathBuf, PublicationState>,
}

enum PublicationState {
    Queued { version: i32, ticket: u64 },
    Active { ticket: u64 },
}

#[derive(Clone)]
struct DiagnosticDocument {
    path: Utf8PathBuf,
    version: i32,
    ticket: u64,
}

impl DiagnosticQueue {
    pub(crate) fn schedule(&mut self, document: &TextDocument) {
        self.next_ticket += 1;
        self.publications.insert(
            document.path().to_path_buf(),
            PublicationState::Queued {
                version: document.version(),
                ticket: self.next_ticket,
            },
        );
    }

    pub(crate) fn close(&mut self, path: &Utf8Path) {
        self.publications.remove(path);
    }

    pub(crate) fn has_outstanding(&self, path: &Utf8Path) -> bool {
        self.publications.contains_key(path)
    }

    fn take_next(&mut self) -> Option<DiagnosticDocument> {
        self.publications.iter_mut().find_map(|(path, state)| {
            let PublicationState::Queued { version, ticket } = state else {
                return None;
            };
            let document = DiagnosticDocument {
                path: path.clone(),
                version: *version,
                ticket: *ticket,
            };
            *state = PublicationState::Active { ticket: *ticket };
            Some(document)
        })
    }

    fn finish(&mut self, document: &DiagnosticDocument) {
        if self.is_current(document) {
            self.publications.remove(&document.path);
        }
    }

    fn is_current(&self, document: &DiagnosticDocument) -> bool {
        matches!(
            self.publications.get(&document.path),
            Some(PublicationState::Active { ticket }) if *ticket == document.ticket
        )
    }

    fn retry(&mut self, document: DiagnosticDocument) {
        if self.is_current(&document) {
            self.publications.insert(
                document.path,
                PublicationState::Queued {
                    version: document.version,
                    ticket: document.ticket,
                },
            );
        }
    }
}

#[derive(Clone)]
pub(crate) struct DiagnosticPublisher {
    wake: Arc<Notify>,
    refresh: Arc<Notify>,
}

impl DiagnosticPublisher {
    pub(crate) fn new(session: Arc<Mutex<Session>>, client: Client) -> Self {
        let wake = Arc::new(Notify::new());
        let refresh = Arc::new(Notify::new());
        tokio::spawn(publish_pending(session, client.clone(), Arc::clone(&wake)));
        tokio::spawn(refresh_diagnostics(client, Arc::clone(&refresh)));
        Self { wake, refresh }
    }

    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    pub(crate) async fn project_ready(&self, session: &Arc<Mutex<Session>>, generation: u64) {
        let mut session = session.lock().await;
        if session.readiness_state() != IntrinsicReadinessState::Ready(generation) {
            return;
        }
        if session.client_info().supports_pull_diagnostics()
            && session
                .client_info()
                .supports_workspace_diagnostic_refresh()
        {
            self.refresh.notify_one();
        } else {
            session.queue_all_diagnostics();
            self.wake();
        }
    }
}

async fn refresh_diagnostics(client: Client, wake: Arc<Notify>) {
    loop {
        wake.notified().await;
        // One active refresh and one pending wake, regardless of edit rate.
        // Dropping this future does not retire the client's pending RPC. Keep it
        // alive until the response arrives; this worker owns no database snapshot.
        async {
            let started = Instant::now();
            debug!(outcome = "started", "Diagnostic refresh");
            let outcome = match client.workspace_diagnostic_refresh().await {
                Ok(()) => "accepted",
                Err(_) => "rejected",
            };
            debug!(
                outcome,
                transport_ms = started.elapsed().as_secs_f64() * 1000.0,
                "Diagnostic refresh"
            );
        }
        .instrument(debug_span!("diagnostics.refresh"))
        .await;
    }
}

async fn publish_pending(session: Arc<Mutex<Session>>, client: Client, wake: Arc<Notify>) {
    loop {
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (work, mut readiness) = {
            let mut session = session.lock().await;
            let state = session.readiness_state();
            let work = if matches!(
                state,
                IntrinsicReadinessState::Ready(_) | IntrinsicReadinessState::ReadyWithoutProject
            ) {
                session
                    .diagnostics
                    .take_next()
                    .map(|document| (document, state, session.snapshot()))
            } else {
                None
            };
            (work, session.readiness_receiver())
        };
        let Some((document, state, snapshot)) = work else {
            tokio::select! {
                () = &mut notified => {}
                _ = readiness.changed() => {}
            }
            continue;
        };

        let generation = match state {
            IntrinsicReadinessState::Ready(generation) => Some(generation),
            IntrinsicReadinessState::ReadyWithoutProject
            | IntrinsicReadinessState::Unready(_)
            | IntrinsicReadinessState::Failed(_) => None,
        };
        let span = debug_span!(
            "diagnostics.publication",
            ticket = document.ticket,
            version = document.version,
            generation
        );
        publish_document(&session, &client, document, state, snapshot)
            .instrument(span)
            .await;
    }
}

async fn publish_document(
    session: &Mutex<Session>,
    client: &Client,
    document: DiagnosticDocument,
    state: IntrinsicReadinessState,
    snapshot: SessionSnapshot,
) {
    debug!(outcome = "started", "Diagnostic publication");
    let path = document.path.clone();
    let result = spawn_blocking(blocking_in_span(tracing::Span::current(), move || {
        compute_diagnostics(&snapshot, &path)
    }))
    .await;
    let task_failed = result.is_err();
    let diagnostics = match classify_diagnostics_task_join(result) {
        Ok(Some(diagnostics)) => diagnostics,
        Ok(None) => {
            let mut session = session.lock().await;
            // A failed task is reported even if an edit superseded it meanwhile.
            let outcome = if task_failed {
                "blocking_task_failed"
            } else if !session.diagnostics.is_current(&document) {
                "superseded"
            } else {
                "no_payload"
            };
            session.diagnostics.finish(&document);
            debug!(outcome, "Diagnostic publication");
            return;
        }
        Err(_) => {
            let mut session = session.lock().await;
            let outcome = if session.diagnostics.is_current(&document) {
                "retried"
            } else {
                "superseded"
            };
            session.diagnostics.retry(document);
            debug!(
                outcome,
                reason = "computation_cancelled",
                "Diagnostic publication"
            );
            return;
        }
    };
    {
        let mut session = session.lock().await;
        if !session.diagnostics.is_current(&document) {
            debug!(outcome = "superseded", "Diagnostic publication");
            return;
        }
        if session.readiness_state() != state {
            session.diagnostics.retry(document);
            debug!(
                outcome = "retried",
                reason = "readiness_changed",
                "Diagnostic publication"
            );
            return;
        }
    }
    let Some(uri) = ls_types::Uri::from_path(&document.path) else {
        session.lock().await.diagnostics.finish(&document);
        debug!(
            outcome = "no_payload",
            reason = "invalid_uri",
            "Diagnostic publication"
        );
        return;
    };
    // This single publisher preserves commit order without holding the session
    // mutex during client backpressure. The payload and version share a snapshot.
    let started = Instant::now();
    debug!(
        phase = "transport",
        outcome = "started",
        "Diagnostic publication"
    );
    client
        .publish_diagnostics(uri, diagnostics, Some(document.version))
        .await;
    // Completion is a transport observation, not an editor acknowledgement.
    debug!(
        phase = "transport",
        outcome = "success",
        transport_ms = started.elapsed().as_secs_f64() * 1000.0,
        "Diagnostic publication"
    );
    // A fallback obligation ends after delivery, unless an intervening edit
    // superseded this ticket and requires a current-version publication.
    session.lock().await.diagnostics.finish(&document);
}

type DiagnosticsJobResult = Result<Option<Vec<ls_types::Diagnostic>>, Cancelled>;

fn compute_diagnostics(snapshot: &SessionSnapshot, path: &Utf8Path) -> DiagnosticsJobResult {
    let result = Cancelled::catch(AssertUnwindSafe(|| {
        let file = path_to_file(snapshot.db(), path).ok()?;
        djls_ide::collect_diagnostics(
            snapshot.db(),
            file,
            snapshot.client_info().position_encoding(),
        )
    }));
    let outcome = match &result {
        Ok(Some(_)) => "success",
        Ok(None) => "no_payload",
        Err(_) => "cancelled",
    };
    debug!(phase = "computation", outcome, "Diagnostic computation");
    result
}

fn classify_diagnostics_task_join(
    joined: Result<DiagnosticsJobResult, JoinError>,
) -> DiagnosticsJobResult {
    match joined {
        Ok(result) => result,
        Err(error) => {
            // Panic payloads can contain source text or paths.
            error!(
                panicked = error.is_panic(),
                "Diagnostic computation task failed"
            );
            debug!(?error, "Task failure detail");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::future::poll_fn;
    use std::pin::pin;
    use std::task::Poll;
    use std::time::Duration;

    use djls_source::FileKind;
    use futures_util::SinkExt;
    use futures_util::StreamExt;
    use tower_lsp_server::LanguageServer;
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc;
    use tower_service::Service;
    use tracing::instrument::WithSubscriber;
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

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn pull_fallback_edit_during_blocked_publication_reaches_latest_version() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut service, mut socket) = LspService::new(TransportBackend);
            poll_fn(|cx| service.poll_ready(cx))
                .await
                .expect("ready service");
            service
                .call(
                    jsonrpc::Request::build("initialize")
                        .params(serde_json::json!({"capabilities": {}}))
                        .id(1_i64)
                        .finish(),
                )
                .await
                .expect("initialized service");
            let client = service.inner().0.clone();
            let mut session = Session::new(&ls_types::InitializeParams {
                capabilities: ls_types::ClientCapabilities {
                    text_document: Some(ls_types::TextDocumentClientCapabilities {
                        diagnostic: Some(ls_types::DiagnosticClientCapabilities::default()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            });
            let uri: ls_types::Uri = "file:///tmp/backpressure.html".parse().expect("file URI");
            let path = Utf8Path::new("/tmp/backpressure.html");
            let _mutation = session.open_document(&ls_types::TextDocumentItem {
                uri: uri.clone(),
                language_id: "htmldjango".into(),
                version: 1,
                text: "{{ value".into(),
            });
            let generation = session.desired_generation();
            let prime = djls_ide::prime_template_library_products(session.db()).expect("project");
            assert!(session.publish_intrinsic_readiness(generation, &prime));
            session.queue_all_diagnostics();
            let session = Arc::new(Mutex::new(session));
            let wake = Arc::new(Notify::new());
            let mut publisher = pin!(publish_pending(
                Arc::clone(&session),
                client.clone(),
                Arc::clone(&wake)
            ));

            let old = poll_fn(|cx| {
                // Saturate the actual bounded client channel before each publisher poll.
                // A pending filler has already enqueued its message; no capacity constant
                // or sleeps are needed, including while computation runs on its worker.
                while pin!(client.log_message(ls_types::MessageType::LOG, "filler"))
                    .poll(cx)
                    .is_ready()
                {}
                assert!(publisher.as_mut().poll(cx).is_pending());
                while let Poll::Ready(Some(message)) = socket.poll_next_unpin(cx) {
                    if message.method() == "textDocument/publishDiagnostics" {
                        return Poll::Ready(message);
                    }
                }
                Poll::Pending
            })
            .await;
            let old: ls_types::PublishDiagnosticsParams = serde_json::from_value(
                serde_json::to_value(old).expect("notification")["params"].clone(),
            )
            .expect("diagnostic params");
            assert_eq!(old.version, Some(1));
            assert_eq!(old.diagnostics.len(), 1);
            {
                let events = capture.0.lock().expect("capture lock");
                assert!(events.iter().any(|event| {
                    event["fields"]["phase"] == "computation"
                        && event["fields"]["outcome"] == "success"
                        && event["spans"][0]["fields"]["version"] == 1
                }));
                assert!(
                    !events
                        .iter()
                        .any(|event| event["fields"]["phase"] == "transport"
                            && event["fields"]["outcome"] == "success"),
                    "an enqueued message is not a completed send"
                );
            }
            {
                let mut session = session.lock().await;
                assert!(
                    matches!(
                        session.diagnostics.publications.get(path),
                        Some(PublicationState::Active { .. })
                    ),
                    "publication must still be blocked, not already retired"
                );
                let _mutation = session.update_document(
                    &ls_types::VersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version: 2,
                    },
                    vec![ls_types::TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: "<p>fixed</p>".into(),
                    }],
                );
            }

            // Draining transport releases the old send. Its completion must leave
            // the new ticket alive for the same real publisher loop to compute.
            let latest = poll_fn(|cx| {
                assert!(publisher.as_mut().poll(cx).is_pending());
                while let Poll::Ready(Some(message)) = socket.poll_next_unpin(cx) {
                    if message.method() == "textDocument/publishDiagnostics" {
                        return Poll::Ready(message);
                    }
                }
                Poll::Pending
            })
            .await;
            let latest: ls_types::PublishDiagnosticsParams = serde_json::from_value(
                serde_json::to_value(latest).expect("notification")["params"].clone(),
            )
            .expect("diagnostic params");
            assert_eq!(latest.uri, uri);
            assert_eq!(latest.version, Some(2));
            assert!(latest.diagnostics.is_empty());
            poll_fn(|cx| {
                assert!(publisher.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            {
                let mut session = session.lock().await;
                assert!(!session.diagnostics.has_outstanding(path));
                let _mutation = session.update_document(
                    &ls_types::VersionedTextDocumentIdentifier { uri, version: 3 },
                    vec![ls_types::TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: "{{ broken again".into(),
                    }],
                );
                assert!(!session.diagnostics.has_outstanding(path));
            }
            wake.notify_one();
            poll_fn(|cx| {
                assert!(publisher.as_mut().poll(cx).is_pending());
                assert!(
                    socket.poll_next_unpin(cx).is_pending(),
                    "ordinary pull edit must not push"
                );
                Poll::Ready(())
            })
            .await;
        })
        .with_subscriber(subscriber)
        .await
        .expect("publisher must make progress despite backpressure");
        let events = capture.0.lock().expect("capture lock");
        let sends: Vec<_> = events
            .iter()
            .filter(|event| {
                event["fields"]["phase"] == "transport" && event["fields"]["outcome"] == "success"
            })
            .collect();
        assert_eq!(sends.len(), 2);
        assert_eq!(sends[0]["spans"][0]["fields"]["version"], 1);
        assert_eq!(sends[1]["spans"][0]["fields"]["version"], 2);
        assert!(
            sends[0]["spans"][0]["fields"]["ticket"].as_u64()
                < sends[1]["spans"][0]["fields"]["ticket"].as_u64()
        );
        for send in sends {
            assert_eq!(send["spans"][0]["name"], "diagnostics.publication");
            assert!(send["spans"][0]["fields"]["generation"].is_u64());
            assert!(send["fields"]["transport_ms"].is_f64());
            let computation = events
                .iter()
                .find(|event| {
                    event["fields"]["phase"] == "computation"
                        && event["spans"][0] == send["spans"][0]
                })
                .expect("blocking computation inherits ticket context");
            assert_eq!(computation["spans"].as_array().map(Vec::len), Some(1));
            assert!(events.iter().any(|event| {
                event["fields"]["compute_ms"].is_f64() && event["spans"][0] == send["spans"][0]
            }));
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| event["fields"].get("phase").is_none()
                    && event["fields"]["outcome"] == "started")
                .count(),
            2,
            "idle wakes and queue polls do not start operations"
        );
        let serialized = serde_json::to_string(&*events).expect("captured events");
        assert!(!serialized.contains("backpressure.html"));
        assert!(!serialized.contains("file:///"));
        assert!(!serialized.contains("{{ value"));
    }

    #[tokio::test]
    async fn publication_terminal_outcomes() {
        let _callsite_guard = callsite_guard();
        for (name, expected) in [
            ("page.txt", "no_payload"),
            ("page.html", "retried"),
            ("closed.html", "superseded"),
        ] {
            let capture = Capture::default();
            let subscriber = tracing_subscriber::registry().with(capture.clone());
            let (service, _socket) = LspService::new(TransportBackend);
            let mut session = Session::default();
            let _mutation = session.open_document(&ls_types::TextDocumentItem {
                uri: format!("file:///tmp/{name}").parse().expect("URI"),
                language_id: "htmldjango".into(),
                version: 9,
                text: "{{ value".into(),
            });
            let document = session.diagnostics.take_next().expect("ticket");
            if expected == "superseded" {
                session.diagnostics.close(&document.path);
            }
            let snapshot = session.snapshot();
            let session = Mutex::new(session);
            publish_document(
                &session,
                &service.inner().0,
                document.clone(),
                IntrinsicReadinessState::Ready(u64::MAX),
                snapshot,
            )
            .with_subscriber(subscriber)
            .await;
            let mut session = session.lock().await;
            if expected == "retried" {
                assert_eq!(
                    session.diagnostics.take_next().expect("retry").ticket,
                    document.ticket
                );
            } else {
                assert!(!session.diagnostics.has_outstanding(&document.path));
            }
            let events = capture.0.lock().expect("capture lock");
            assert_eq!(
                events.last().expect("terminal event")["fields"]["outcome"],
                expected,
                "{name}"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| event["fields"]["phase"] == "transport"),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn publication_is_quiet_under_default_filter() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        let subscriber = tracing_subscriber::registry()
            .with(capture.clone())
            .with(tracing_subscriber::EnvFilter::new("warn,djls_server=info"));
        let (service, _socket) = LspService::new(TransportBackend);
        let mut session = Session::default();
        let _mutation = session.open_document(&ls_types::TextDocumentItem {
            uri: "file:///tmp/page.html".parse().expect("URI"),
            language_id: "htmldjango".into(),
            version: 9,
            text: "{{ value".into(),
        });
        let document = session.diagnostics.take_next().expect("ticket");
        let snapshot = session.snapshot();
        let session = Mutex::new(session);
        publish_document(
            &session,
            &service.inner().0,
            document,
            IntrinsicReadinessState::Ready(u64::MAX),
            snapshot,
        )
        .with_subscriber(subscriber)
        .await;
        let events = capture.0.lock().expect("capture lock");
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn diagnostic_eligibility_uses_the_source_path_not_the_client_language() {
        for (name, language, expected) in [
            ("page.html", "html", Some(1)),
            ("page.html", "python", Some(1)),
            ("page.txt", "htmldjango", None),
        ] {
            let mut session = Session::default();
            let uri = format!("file:///tmp/{name}").parse().expect("file URI");
            let _mutation = session.open_document(&ls_types::TextDocumentItem {
                uri,
                language_id: language.into(),
                version: 1,
                text: "{{ value".into(),
            });
            for reload in [false, true] {
                if reload {
                    session.queue_all_diagnostics();
                }
                let document = session.diagnostics.take_next().expect("queued path");
                let file = path_to_file(session.db(), &document.path).expect("buffered file");
                let diagnostics = djls_ide::collect_diagnostics(
                    session.db(),
                    file,
                    djls_source::PositionEncoding::Utf16,
                );
                assert_eq!(diagnostics.map(|items| items.len()), expected);
                session.diagnostics.finish(&document);
            }
        }
        let mut session = Session::default();
        let _mutation = session.update_document(
            &ls_types::VersionedTextDocumentIdentifier {
                uri: "file:///tmp/without-open.html".parse().expect("file URI"),
                version: 7,
            },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "{{ value".into(),
            }],
        );
        let document = session
            .diagnostics
            .take_next()
            .expect("full replacement queued");
        assert_eq!(document.version, 7);
        let file = path_to_file(session.db(), &document.path).expect("buffered file");
        assert_eq!(djls_ide::collect_diagnostics(
            session.db(), file, djls_source::PositionEncoding::Utf16,
        ).expect("template diagnostics").len(), 1);
    }

    #[test]
    fn pull_fallback_republish_survives_edits_until_delivery() {
        let mut session = Session::new(&ls_types::InitializeParams {
            capabilities: ls_types::ClientCapabilities {
                text_document: Some(ls_types::TextDocumentClientCapabilities {
                    diagnostic: Some(ls_types::DiagnosticClientCapabilities::default()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        });
        let uri = "file:///tmp/fallback.html".parse().expect("file URI");
        let _mutation = session.open_document(&ls_types::TextDocumentItem {
            uri,
            language_id: "html".into(),
            version: 1,
            text: "{{ value".into(),
        });
        assert!(
            !session
                .diagnostics
                .has_outstanding(Utf8Path::new("/tmp/fallback.html"))
        );
        session.queue_all_diagnostics();
        let queued = session.diagnostics.take_next().expect("queued fallback");
        assert_eq!(queued.version, 1);
        session.diagnostics.retry(queued);
        let mut delivered = None;
        for version in [2, 3] {
            // Exercise replacement both before and after work becomes active.
            let in_flight =
                (version == 3).then(|| session.diagnostics.take_next().expect("queued fallback"));
            let _mutation = session.update_document(
                &ls_types::VersionedTextDocumentIdentifier {
                    uri: "file:///tmp/fallback.html".parse().expect("file URI"),
                    version,
                },
                vec![ls_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "<p>ok</p>".into(),
                }],
            );
            if let Some(in_flight) = in_flight {
                session.diagnostics.finish(&in_flight);
                assert!(session.diagnostics.has_outstanding(&in_flight.path));
            }
            let current = session.diagnostics.take_next().expect("fallback retained");
            assert_eq!(current.version, version);
            if version == 2 {
                session.diagnostics.retry(current);
            } else {
                delivered = Some(current);
            }
        }
        let delivered = delivered.expect("delivery");
        let file = path_to_file(session.db(), &delivered.path).expect("buffered file");
        assert!(djls_ide::collect_diagnostics(
            session.db(), file, djls_source::PositionEncoding::Utf16,
        ).expect("template diagnostics").is_empty());
        session.diagnostics.finish(&delivered);
        assert!(!session.diagnostics.has_outstanding(&delivered.path));
        let _mutation = session.update_document(
            &ls_types::VersionedTextDocumentIdentifier {
                uri: "file:///tmp/fallback.html".parse().expect("file URI"),
                version: 4,
            },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "{{ value".into(),
            }],
        );
        assert!(
            !session.diagnostics.has_outstanding(&delivered.path),
            "delivery must not enable permanent push diagnostics"
        );
    }

    #[tokio::test]
    async fn diagnostics_snapshot_task_panic_produces_no_publish_payload() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let joined = spawn_blocking(|| {
            panic!("synthetic diagnostics panic");
            #[allow(unreachable_code)]
            Ok::<Option<Vec<ls_types::Diagnostic>>, Cancelled>(None)
        })
        .await;
        tracing::subscriber::with_default(subscriber, || {
            let span = debug_span!("diagnostics.publication", ticket = 7_u64, version = 3);
            let _entered = span.enter();
            assert!(
                classify_diagnostics_task_join(joined)
                    .expect("not Salsa cancellation")
                    .is_none()
            );
        });
        let events = capture.0.lock().expect("capture lock");
        let failure = events
            .iter()
            .find(|event| event["level"] == "ERROR")
            .expect("task failure event");
        assert_eq!(failure["fields"]["panicked"], true);
        assert_eq!(failure["spans"][0]["name"], "diagnostics.publication");
        assert_eq!(failure["spans"][0]["fields"]["ticket"], 7);
        let default_visible: Vec<_> = events
            .iter()
            .filter(|event| event["level"] != "DEBUG")
            .collect();
        assert!(
            !serde_json::to_string(&default_visible)
                .expect("events")
                .contains("synthetic diagnostics panic")
        );
    }

    #[tokio::test]
    async fn refresh_attempts_report_responses_and_coalesce_pending_wakes() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut service, mut socket) = LspService::new(TransportBackend);
            service
                .call(
                    jsonrpc::Request::build("initialize")
                        .params(serde_json::json!({"capabilities": {}}))
                        .id(1_i64)
                        .finish(),
                )
                .await
                .expect("initialize");
            let wake = Arc::new(Notify::new());
            let mut worker = pin!(refresh_diagnostics(
                service.inner().0.clone(),
                Arc::clone(&wake)
            ));
            wake.notify_one();
            for accepted in [true, false] {
                let request = poll_fn(|cx| {
                    assert!(worker.as_mut().poll(cx).is_pending());
                    socket.poll_next_unpin(cx)
                })
                .await
                .expect("refresh request");
                assert_eq!(request.method(), "workspace/diagnostic/refresh");
                if accepted {
                    for _ in 0..40 {
                        wake.notify_one();
                    }
                }
                poll_fn(|cx| {
                    assert!(worker.as_mut().poll(cx).is_pending());
                    assert!(
                        socket.poll_next_unpin(cx).is_pending(),
                        "only one active RPC"
                    );
                    Poll::Ready(())
                })
                .await;
                let id = request.id().expect("request ID").clone();
                let response = if accepted {
                    jsonrpc::Response::from_ok(id, serde_json::Value::Null)
                } else {
                    jsonrpc::Response::from_error(id, jsonrpc::Error::invalid_request())
                };
                socket.send(response).await.expect("client response");
            }
            poll_fn(|cx| {
                assert!(worker.as_mut().poll(cx).is_pending());
                assert!(
                    socket.poll_next_unpin(cx).is_pending(),
                    "pending wakes coalesced"
                );
                Poll::Ready(())
            })
            .await;
        })
        .with_subscriber(subscriber)
        .await
        .expect("refresh progress");
        let events = capture.0.lock().expect("capture lock");
        let outcomes: Vec<_> = events
            .iter()
            .map(|event| event["fields"]["outcome"].as_str().expect("outcome"))
            .collect();
        assert_eq!(outcomes, ["started", "accepted", "started", "rejected"]);
        for event in events.iter() {
            assert_eq!(event["spans"][0]["name"], "diagnostics.refresh");
            if event["fields"]["outcome"] != "started" {
                assert!(event["fields"]["transport_ms"].is_f64());
            }
        }
    }

    #[tokio::test]
    async fn ready_diagnostics_use_the_delivery_supported_by_the_client() {
        for (pull, refresh) in [(false, false), (true, false), (true, true)] {
            let mut session = Session::new(&ls_types::InitializeParams {
                capabilities: ls_types::ClientCapabilities {
                    workspace: Some(ls_types::WorkspaceClientCapabilities {
                        diagnostics: Some(ls_types::DiagnosticWorkspaceClientCapabilities {
                            refresh_support: Some(refresh),
                        }),
                        ..Default::default()
                    }),
                    text_document: Some(ls_types::TextDocumentClientCapabilities {
                        diagnostic: pull
                            .then_some(ls_types::DiagnosticClientCapabilities::default()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            });
            let document = ls_types::TextDocumentItem {
                uri: "file:///tmp/djls-diagnostic-delivery.html"
                    .parse()
                    .expect("file URI"),
                language_id: "htmldjango".to_string(),
                version: 1,
                text: "{{ value".to_string(),
            };
            let _mutation = session.open_document(&document);
            let path = Utf8Path::new("/tmp/djls-diagnostic-delivery.html");
            session.diagnostics.close(path);
            let generation = session.desired_generation();
            let prime = djls_ide::prime_template_library_products(session.db()).expect("project");
            assert!(session.publish_intrinsic_readiness(generation, &prime));
            let publisher = DiagnosticPublisher {
                wake: Arc::new(Notify::new()),
                refresh: Arc::new(Notify::new()),
            };
            let session = Arc::new(Mutex::new(session));
            publisher.project_ready(&session, generation).await;
            assert_eq!(
                session.lock().await.diagnostics.has_outstanding(path),
                !(pull && refresh)
            );
            let notified =
                tokio::time::timeout(Duration::from_millis(1), publisher.refresh.notified()).await;
            assert_eq!(notified.is_ok(), pull && refresh);
        }
    }

    #[test]
    fn latest_pending_version_wins_and_close_reopen_rejects_old_work() {
        let path = Utf8PathBuf::from("/project/page.html");
        let mut queue = DiagnosticQueue::default();
        for version in 0..40 {
            queue.schedule(&TextDocument::new(
                path.clone(),
                format!("{version}"),
                version,
                FileKind::Template,
            ));
        }
        let old = queue.take_next().expect("pending document");
        assert_eq!(old.version, 39);
        queue.close(&path);
        queue.schedule(&TextDocument::new(
            path,
            "reopened".to_string(),
            39,
            FileKind::Template,
        ));
        assert!(
            !queue.is_current(&old),
            "LSP version reuse must not revive old work"
        );
        queue.retry(old);
        let current = queue.take_next().expect("reopened document");
        assert!(queue.is_current(&current));
        queue.retry(current);
        assert_eq!(
            queue.take_next().expect("retried document").version,
            39,
            "current cancellations remain pending"
        );
    }

    #[test]
    fn stale_retry_and_finish_do_not_retire_replacement() {
        let path = Utf8PathBuf::from("/project/page.html");
        let mut queue = DiagnosticQueue::default();
        queue.schedule(&TextDocument::new(
            path.clone(),
            "old".to_string(),
            1,
            FileKind::Template,
        ));
        let old = queue.take_next().expect("initial document");

        queue.schedule(&TextDocument::new(
            path.clone(),
            "new".to_string(),
            2,
            FileKind::Template,
        ));
        queue.retry(old.clone());
        queue.finish(&old);

        assert!(queue.has_outstanding(&path));
        let replacement = queue.take_next().expect("replacement document");
        assert_eq!(replacement.version, 2);
        queue.retry(old.clone());
        queue.finish(&old);
        assert!(queue.is_current(&replacement));
        assert!(queue.take_next().is_none());
        queue.finish(&replacement);
        assert!(!queue.has_outstanding(&path));
    }

    #[test]
    fn take_next_skips_active_path_when_ordering_queued_work() {
        let mut queue = DiagnosticQueue::default();
        for path in ["/project/a.html", "/project/b.html", "/project/c.html"] {
            queue.schedule(&TextDocument::new(
                Utf8PathBuf::from(path),
                path.to_string(),
                1,
                FileKind::Template,
            ));
        }

        let first = queue.take_next().expect("first document");
        assert_eq!(first.path, Utf8Path::new("/project/a.html"));
        let second = queue.take_next().expect("second document");
        assert_eq!(second.path, Utf8Path::new("/project/b.html"));
        let third = queue.take_next().expect("third document");
        assert_eq!(third.path, Utf8Path::new("/project/c.html"));
        assert!(queue.take_next().is_none());
    }
}
