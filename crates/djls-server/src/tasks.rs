use crate::notifier::Notifier;
use anyhow::Result;
use djls_worker::Task;
use std::sync::Arc;
use std::time::Duration;
use tower_lsp::lsp_types::MessageType;

pub struct DebugTask {
    pub message: String,
    pub delay: Duration,
    pub notifier: Arc<Box<dyn Notifier>>,
}

impl DebugTask {
    pub fn new(message: String, delay: Duration, notifier: Arc<Box<dyn Notifier>>) -> Self {
        Self {
            message,
            delay,
            notifier,
        }
    }
}

impl Task for DebugTask {
    type Output = String;

    fn run(&self) -> Result<Self::Output> {
        std::thread::sleep(self.delay);
        let result = format!("Debug task completed: {}", self.message);

        // Log the result
        self.notifier
            .log_message(MessageType::INFO, &result)
            .unwrap_or_default();

        Ok(result)
    }
}
