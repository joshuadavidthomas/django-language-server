use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use tracing::Event;
use tracing::Level;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

/// Events split at the editor-forwarding boundary: INFO and above reach the
/// client by default, so privacy assertions target `default_visible`.
#[derive(Debug, Default)]
pub struct CapturedEvents {
    pub default_visible: String,
    pub debug: String,
}

#[derive(Clone, Default)]
struct CaptureLayer(Arc<Mutex<CapturedEvents>>);

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let level = *event.metadata().level();
        let mut fields = FieldText::default();
        event.record(&mut fields);
        let mut captured = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let sink = if level <= Level::INFO {
            &mut captured.default_visible
        } else {
            &mut captured.debug
        };
        sink.push_str(&level.to_string());
        for field in fields.0 {
            sink.push(' ');
            sink.push_str(&field);
        }
        sink.push('\n');
    }
}

#[derive(Default)]
struct FieldText(Vec<String>);

impl Visit for FieldText {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.push(format!("{}={value:?}", field.name()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push(format!("{}={value}", field.name()));
    }
}

pub fn capture_events<T>(f: impl FnOnce() -> T) -> (T, CapturedEvents) {
    // tracing-core 0.1.36 can cache a callsite as disabled when another
    // subscriber-less test reaches it first. A second registered dispatcher
    // keeps scoped captures dynamic. https://github.com/tokio-rs/tracing/issues/2874
    let _callsite_cache_guard = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new());
    let layer = CaptureLayer::default();
    let captured = Arc::clone(&layer.0);
    let subscriber = tracing_subscriber::Registry::default().with(layer);
    let result = tracing::subscriber::with_default(subscriber, f);
    let captured = std::mem::take(&mut *captured.lock().unwrap_or_else(PoisonError::into_inner));
    (result, captured)
}
