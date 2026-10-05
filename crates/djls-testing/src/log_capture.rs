// Test captures should fail loudly when their registry or event shape is invalid.
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::Mutex;

use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::registry::Scope;

/// Keep the returned dispatcher alive for the whole test.
///
/// tracing-core 0.1.36's single-dispatch fast path can cache a callsite as
/// disabled when a subscriber-less thread registers it first. A second
/// registered (never installed) dispatcher keeps interest dynamic.
/// <https://github.com/tokio-rs/tracing/issues/2874>
#[must_use]
pub fn callsite_guard() -> tracing::Dispatch {
    tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new())
}

#[derive(Clone, Default)]
pub struct Capture(pub Arc<Mutex<Vec<serde_json::Value>>>);

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
/// Events split at the editor-forwarding boundary: INFO and above reach the
/// client by default, so privacy assertions target `default_visible`.
#[derive(Debug, Default)]
pub struct CapturedEvents {
    pub default_visible: String,
    pub debug: String,
}

/// Run a synchronous operation and collect its tracing events.
///
/// # Panics
///
/// Panics if the capture lock is poisoned or a captured event has an invalid shape.
pub fn capture_events<T>(f: impl FnOnce() -> T) -> (T, CapturedEvents) {
    let _guard = callsite_guard();
    let capture = Capture::default();
    let subscriber = tracing_subscriber::Registry::default().with(capture.clone());
    let result = tracing::subscriber::with_default(subscriber, f);
    let mut captured = CapturedEvents::default();
    for event in capture.0.lock().expect("capture lock").iter() {
        let level = event["level"].as_str().expect("level");
        let sink = if matches!(level, "INFO" | "WARN" | "ERROR") {
            &mut captured.default_visible
        } else {
            &mut captured.debug
        };
        sink.push_str(level);
        let fields = event["fields"].as_object().expect("fields");
        // Keep the message first so textual privacy assertions stay readable.
        for (name, value) in fields
            .iter()
            .filter(|(name, _)| *name == "message")
            .chain(fields.iter().filter(|(name, _)| *name != "message"))
        {
            sink.push(' ');
            sink.push_str(name);
            sink.push('=');
            sink.push_str(
                &value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned),
            );
        }
        sink.push('\n');
    }
    (result, captured)
}
