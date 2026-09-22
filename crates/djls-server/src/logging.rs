//! Tracing subscriber setup.
//!
//! One tracing event stream feeds two destinations: the log file and the
//! editor's output panel through `window/logMessage` ([`LspLayer`]). Each
//! destination has its own filter, and neither can block the code emitting
//! the event.

mod file_writer;

use std::fmt;
use std::fmt::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;
use tracing::Level;
use tracing::debug;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::filter;
use tracing_subscriber::fmt as tracing_fmt;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const LSP_QUEUE_LIMIT: usize = 256;
const LSP_RECORD_LIMIT: usize = 4 * 1024;

// Do not use `djls=info`: target directives match prefixes, including tooling
// crates that are not part of the language-server runtime.
const DEFAULT_FILTER: &str = "warn,djls_server=info,djls_conf=info,djls_db=info,djls_ide=info,djls_project=info,djls_source=info,djls_semantic=info,djls_templates=info,djls_format=info";
// Keep in sync with the runtime crates in `DEFAULT_FILTER`. Dependencies stay
// out of the editor: Salsa logs every query at INFO, and `tower_lsp_server`
// logs its own send failures, which would feed back into this layer.
const LSP_TARGETS: &[&str] = &[
    "djls_server",
    "djls_conf",
    "djls_db",
    "djls_ide",
    "djls_project",
    "djls_source",
    "djls_semantic",
    "djls_templates",
    "djls_format",
];

fn log_filter(value: Option<&str>) -> EnvFilter {
    // `EnvFilter` accepts an empty string and then enables nothing at all.
    value
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_FILTER))
}

type LspSender = Arc<RwLock<Option<mpsc::Sender<LspLogRecord>>>>;

/// A tracing layer that forwards events to the LSP client.
///
/// Events are queued rather than sent inline: `on_event` runs synchronously on
/// whichever thread emitted the event, and sending is async. A full queue drops
/// the event instead of growing or blocking the caller.
struct LspLayer {
    sender: LspSender,
}

struct LspLogRecord {
    message_type: ls_types::MessageType,
    message: String,
}

impl<S> Layer<S> for LspLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let Some(message_type) = lsp_message_type(*event.metadata().level()) else {
            return;
        };

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let Some(message) = visitor.finish() else {
            return;
        };

        if let Ok(sender) = self.sender.read()
            && let Some(sender) = sender.as_ref()
        {
            drop(sender.try_send(LspLogRecord {
                message_type,
                message,
            }));
        }
    }
}

fn lsp_message_type(level: Level) -> Option<ls_types::MessageType> {
    match level {
        Level::ERROR => Some(ls_types::MessageType::ERROR),
        Level::WARN => Some(ls_types::MessageType::WARNING),
        Level::INFO => Some(ls_types::MessageType::INFO),
        Level::DEBUG | Level::TRACE => None,
    }
}

fn is_lsp_target(target: &str) -> bool {
    target != "djls_server::logging"
        && !target.starts_with("djls_server::logging::")
        && LSP_TARGETS.iter().any(|prefix| {
            target == *prefix
                || target
                    .strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.starts_with("::"))
        })
}

/// Starts and stops the worker that drains [`LspLayer`]'s queue.
///
/// The layer is installed before the LSP service exists, so the client is
/// attached later with [`LspLogControl::start`]. Until then, and after
/// [`LspLogControl::stop`], events are dropped.
#[derive(Clone)]
pub(crate) struct LspLogControl {
    sender: LspSender,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl LspLogControl {
    fn new(sender: LspSender) -> Self {
        Self {
            sender,
            task: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn disconnected() -> Self {
        Self::new(Arc::new(RwLock::new(None)))
    }

    pub(crate) fn start(&self, client: Client) {
        let Ok(mut task) = self.task.lock() else {
            return;
        };
        if task.is_some() {
            return;
        }
        let Ok(mut sender) = self.sender.write() else {
            return;
        };
        let (queue, mut receiver) = mpsc::channel::<LspLogRecord>(LSP_QUEUE_LIMIT);
        *sender = Some(queue);
        *task = Some(tokio::spawn(async move {
            while let Some(record) = receiver.recv().await {
                client
                    .log_message(record.message_type, record.message)
                    .await;
            }
        }));
    }

    /// Stops accepting events and ends the worker without waiting on the
    /// transport, which may already be closed or saturated during shutdown.
    pub(crate) async fn stop(&self) {
        if let Ok(mut sender) = self.sender.write() {
            sender.take();
        }
        let task = self.task.lock().ok().and_then(|mut task| task.take());
        if let Some(task) = task {
            task.abort();
            drop(task.await);
        }
    }
}

/// Wraps `work` to run inside `span` on a Tokio blocking worker, and records
/// how long it computed (excluding time queued for a worker).
///
/// Blocking workers don't inherit the caller's span, so it is captured here
/// and entered only on the worker; no guard ever crosses an await.
pub(crate) fn blocking_in_span<T>(
    span: tracing::Span,
    work: impl FnOnce() -> T,
) -> impl FnOnce() -> T {
    with_caller_dispatch(move || {
        span.in_scope(|| {
            let _timer = ComputeTimer(Instant::now());
            work()
        })
    })
}

// Emit compute timing even when blocking work unwinds.
struct ComputeTimer(Instant);

impl Drop for ComputeTimer {
    fn drop(&mut self) {
        debug!(
            event = "compute_completed",
            compute_ms = self.0.elapsed().as_secs_f64() * 1000.0
        );
    }
}

// Production installs one global subscriber, which every thread already sees.
// Tests install scoped subscribers, which Tokio's blocking pool does not
// inherit, so only test builds carry the caller's dispatcher over.
#[cfg(not(test))]
fn with_caller_dispatch<T>(work: impl FnOnce() -> T) -> impl FnOnce() -> T {
    work
}

#[cfg(test)]
fn with_caller_dispatch<T>(work: impl FnOnce() -> T) -> impl FnOnce() -> T {
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    move || tracing::dispatcher::with_default(&dispatch, work)
}

/// Captures events, with their fields and enclosing spans, as JSON for
/// instrumentation tests across modules.
#[cfg(test)]
pub(crate) mod capture {
    use std::sync::Arc;
    use std::sync::Mutex;

    use tracing::field::Field;
    use tracing::field::Visit;
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::registry::LookupSpan;
    use tracing_subscriber::registry::Scope;

    /// Keep the returned dispatcher alive for the whole test.
    ///
    /// tracing-core 0.1.36's single-dispatch fast path can cache a callsite as
    /// disabled when a subscriber-less thread registers it first. A second
    /// registered (never installed) dispatcher keeps interest dynamic.
    /// <https://github.com/tokio-rs/tracing/issues/2874>
    #[must_use]
    pub(crate) fn callsite_guard() -> tracing::Dispatch {
        tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new())
    }

    #[derive(Clone, Default)]
    pub(crate) struct Capture(pub(crate) Arc<Mutex<Vec<serde_json::Value>>>);

    #[derive(Default)]
    struct Fields(serde_json::Map<String, serde_json::Value>);

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().into(), format!("{value:?}").into());
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().into(), value.into());
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.0.insert(field.name().into(), value.into());
        }

        fn record_f64(&mut self, field: &Field, value: f64) {
            self.0.insert(field.name().into(), value.into());
        }

        fn record_i64(&mut self, field: &Field, value: i64) {
            self.0.insert(field.name().into(), value.into());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().into(), value.into());
        }
    }

    impl<S> tracing_subscriber::Layer<S> for Capture
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        fn enabled(&self, metadata: &tracing::Metadata<'_>, _ctx: Context<'_, S>) -> bool {
            // Instrumentation lives at DEBUG and above; skip unrelated TRACE logging.
            *metadata.level() <= tracing::Level::DEBUG
        }

        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            id: &tracing::span::Id,
            ctx: Context<'_, S>,
        ) {
            let mut fields = Fields::default();
            attrs.record(&mut fields);
            ctx.span(id)
                .expect("new span exists")
                .extensions_mut()
                .insert(fields);
        }

        fn on_record(
            &self,
            id: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            ctx: Context<'_, S>,
        ) {
            let span = ctx.span(id).expect("recorded span exists");
            let mut extensions = span.extensions_mut();
            if let Some(fields) = extensions.get_mut::<Fields>() {
                values.record(fields);
            }
        }

        fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
            let mut fields = Fields::default();
            event.record(&mut fields);
            let spans: Vec<_> = ctx
                .event_scope(event)
                .into_iter()
                .flat_map(Scope::from_root)
                .map(|span| {
                    serde_json::json!({
                        "name": span.name(),
                        "fields": span.extensions().get::<Fields>().expect("span fields exist").0,
                    })
                })
                .collect();
            self.0
                .lock()
                .expect("capture lock should not be poisoned")
                .push(serde_json::json!({
                    "level": event.metadata().level().as_str(),
                    "fields": fields.0,
                    "spans": spans,
                }));
        }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: BoundedText,
    fields: BoundedText,
}

impl MessageVisitor {
    fn push_field_name(&mut self, field: &Field) {
        self.fields.push_raw(" ");
        self.fields.push_raw(field.name());
        self.fields.push_raw("=");
    }

    fn finish(self) -> Option<String> {
        let mut output = BoundedText::default();
        if self.message.text.is_empty() {
            let fields = self.fields.text.trim_start();
            if fields.is_empty() {
                return None;
            }
            output.push_raw(fields);
        } else {
            output.push_raw(&self.message.text);
            output.push_raw(&self.fields.text);
        }
        Some(output.text)
    }
}

// Every other `record_*` method defaults to `record_debug`.
impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message.push_debug(value);
        } else {
            self.push_field_name(field);
            self.fields.push_debug(value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_escaped(value);
        } else {
            self.push_field_name(field);
            self.fields.push_escaped(value);
        }
    }
}

struct BoundedText {
    text: String,
    truncated: bool,
}

impl Default for BoundedText {
    fn default() -> Self {
        Self {
            text: String::with_capacity(LSP_RECORD_LIMIT),
            truncated: false,
        }
    }
}

impl BoundedText {
    fn push_raw(&mut self, value: &str) {
        if self.truncated {
            return;
        }
        let remaining = LSP_RECORD_LIMIT.saturating_sub(self.text.len());
        if value.len() <= remaining {
            self.text.push_str(value);
            return;
        }
        let mut end = remaining.saturating_sub('…'.len_utf8()).min(value.len());
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        if self.text.len() < LSP_RECORD_LIMIT {
            self.text.push('…');
        }
        self.truncated = true;
    }

    fn push_debug(&mut self, value: &dyn fmt::Debug) {
        if write!(self, "{value:?}").is_err() {
            self.truncated = true;
        }
    }

    fn push_escaped(&mut self, value: &str) {
        for character in value.chars() {
            if character.is_control() {
                for escaped in character.escape_default() {
                    let mut buffer = [0; 4];
                    self.push_raw(escaped.encode_utf8(&mut buffer));
                }
            } else {
                let mut buffer = [0; 4];
                self.push_raw(character.encode_utf8(&mut buffer));
            }
        }
    }
}

impl Write for BoundedText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.push_raw(value);
        Ok(())
    }
}

/// Holds logging resources.
///
/// Keep it alive until after the service and runtime are dropped so the file
/// worker flushes last.
pub(crate) struct LoggingGuard {
    _file_guard: file_writer::WorkerGuard,
    lsp: LspLogControl,
}

impl LoggingGuard {
    pub(crate) fn lsp(&self) -> LspLogControl {
        self.lsp.clone()
    }
}

/// Initialize the dual-layer tracing subscriber.
///
/// - File layer: `RUST_LOG`, or [`DEFAULT_FILTER`] when unset, empty, or invalid.
/// - LSP layer: INFO+ from [`LSP_TARGETS`] only, regardless of `RUST_LOG`.
pub(crate) fn init_tracing() -> LoggingGuard {
    // Never print invalid environment contents; they can contain sensitive data.
    let env_filter = log_filter(std::env::var("RUST_LOG").ok().as_deref());

    let counters = Arc::new(file_writer::Counters::default());
    let writer = file_writer::FileWriter::new(
        djls_conf::log_dir().map_err(std::io::Error::other),
        Arc::clone(&counters),
    );
    let (output, file_guard) = file_writer::RecordWriter::start(writer, counters);

    let log_layer = tracing_fmt::layer()
        .with_writer(output)
        .with_ansi(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .with_filter(env_filter);

    let sender = LspSender::default();
    let lsp_layer = LspLayer {
        sender: Arc::clone(&sender),
    }
    .with_filter(filter::filter_fn(|metadata| {
        lsp_message_type(*metadata.level()).is_some() && is_lsp_target(metadata.target())
    }));

    Registry::default().with(log_layer).with(lsp_layer).init();

    LoggingGuard {
        _file_guard: file_guard,
        lsp: LspLogControl::new(sender),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::StreamExt;
    use tower_lsp_server::LanguageServer;
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

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

    fn lsp_capture() -> (impl tracing::Subscriber, mpsc::Receiver<LspLogRecord>) {
        let (queue, receiver) = mpsc::channel(LSP_QUEUE_LIMIT);
        let layer = LspLayer {
            sender: Arc::new(RwLock::new(Some(queue))),
        }
        .with_filter(filter::filter_fn(|metadata| {
            lsp_message_type(*metadata.level()).is_some() && is_lsp_target(metadata.target())
        }));
        (tracing_subscriber::registry().with(layer), receiver)
    }

    #[test]
    fn missing_empty_or_invalid_filter_uses_safe_defaults() {
        for value in [
            None,
            Some(""),
            Some("  "),
            Some("info,djls_server=not-a-level"),
        ] {
            let subscriber = tracing_subscriber::registry().with(log_filter(value));
            tracing::subscriber::with_default(subscriber, || {
                assert!(tracing::enabled!(target: "djls_server::server", tracing::Level::INFO));
                assert!(tracing::enabled!(target: "djls_project::discovery", tracing::Level::INFO));
                assert!(!tracing::enabled!(target: "djls_server", tracing::Level::DEBUG));
                assert!(!tracing::enabled!(target: "salsa::runtime", tracing::Level::INFO));
                assert!(!tracing::enabled!(target: "tower_lsp_server", tracing::Level::INFO));
                assert!(!tracing::enabled!(target: "unrelated", tracing::Level::INFO));
                assert!(!tracing::enabled!(target: "djls_testing", tracing::Level::INFO));
                assert!(tracing::enabled!(target: "salsa::runtime", tracing::Level::WARN));
                assert!(tracing::enabled!(target: "tower_lsp_server", tracing::Level::ERROR));
            });
        }
    }

    #[test]
    fn valid_filter_replaces_file_defaults() {
        let subscriber = tracing_subscriber::registry()
            .with(log_filter(Some("error,salsa=info,djls_server=debug")));
        tracing::subscriber::with_default(subscriber, || {
            assert!(tracing::enabled!(target: "salsa::runtime", tracing::Level::INFO));
            assert!(tracing::enabled!(target: "djls_server", tracing::Level::DEBUG));
            assert!(!tracing::enabled!(target: "djls_project", tracing::Level::INFO));
            assert!(!tracing::enabled!(target: "unrelated", tracing::Level::WARN));
        });
    }

    #[test]
    fn lsp_targets_match_whole_crate_names() {
        assert!(is_lsp_target("djls_server"));
        assert!(is_lsp_target("djls_project::discovery"));
        assert!(!is_lsp_target("djls_server_extra"));
        assert!(!is_lsp_target("djls_server::logging"));
        assert!(!is_lsp_target("djls_testing"));
        assert!(!is_lsp_target("salsa::runtime"));
        assert!(!is_lsp_target("tower_lsp_server::service"));
    }

    #[tokio::test]
    async fn lsp_layer_forwards_owned_info_with_all_fields() {
        // Keep callsite interest dynamic with tracing-core 0.1.36.
        let _callsite_guard = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new());
        let (subscriber, mut receiver) = lsp_capture();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                target: "djls_server::server",
                outcome = "ready",
                count = 3,
                duration_ms = 12_u128,
                reason = ?"stale",
                "Server ready"
            );
            tracing::debug!(target: "djls_server::server", "debug detail");
            tracing::error!(target: "tower_lsp_server::transport", "transport failed");
            tracing::warn!(target: "dependency", "dependency warning");
        });

        let record = receiver.recv().await.expect("owned INFO record");
        assert_eq!(record.message_type, ls_types::MessageType::INFO);
        assert_eq!(
            record.message,
            "Server ready outcome=ready count=3 duration_ms=12 reason=\"stale\""
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn lsp_layer_is_bounded_and_rendering_is_capped() {
        let _callsite_guard = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new());
        let (subscriber, mut receiver) = lsp_capture();
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..(LSP_QUEUE_LIMIT + 10) {
                tracing::info!(target: "djls_server::server", "{}", "x".repeat(LSP_RECORD_LIMIT * 2));
            }
        });

        let first = receiver.recv().await.expect("first bounded record");
        assert_eq!(first.message.len(), LSP_RECORD_LIMIT);
        assert!(first.message.ends_with('…'));
        assert_eq!(receiver.len(), LSP_QUEUE_LIMIT - 1);
    }

    #[tokio::test]
    async fn lsp_worker_sends_owned_event_and_stops() {
        let _callsite_guard = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new());
        let (service, mut socket) = LspService::new(TransportBackend);
        let control = LspLogControl::disconnected();
        control.start(service.inner().0.clone());
        let subscriber = tracing_subscriber::registry().with(LspLayer {
            sender: Arc::clone(&control.sender),
        });

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(target: "djls_server::server", "Visible warning");
        });

        let notification = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .expect("notification timeout")
            .expect("notification");
        assert_eq!(notification.method(), "window/logMessage");
        control.stop().await;
        assert!(control.sender.read().expect("sender").is_none());
    }

    #[tokio::test]
    async fn saturated_transport_does_not_block_shutdown() {
        let _callsite_guard = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new());
        let (service, _socket) = LspService::new(TransportBackend);
        let control = LspLogControl::disconnected();
        control.start(service.inner().0.clone());
        let subscriber = tracing_subscriber::registry().with(LspLayer {
            sender: Arc::clone(&control.sender),
        });

        tracing::subscriber::with_default(subscriber, || {
            for index in 0..(LSP_QUEUE_LIMIT * 4) {
                tracing::info!(target: "djls_server::server", index, "queued");
            }
        });

        tokio::time::timeout(Duration::from_secs(1), control.stop())
            .await
            .expect("logging shutdown");
    }
}
