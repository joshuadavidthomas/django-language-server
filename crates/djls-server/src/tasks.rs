use anyhow::Result;
use djls_worker::Task;
use std::time::Duration;

pub struct DebugTask {
    pub message: String,
    pub delay: Duration,
}

impl DebugTask {
    pub fn new(message: String, delay: Duration) -> Self {
        Self { message, delay }
    }
}

impl Task for DebugTask {
    type Output = String;

    fn run(&self) -> Result<Self::Output> {
        std::thread::sleep(self.delay);
        let result = format!("Debug task completed: {}", self.message);

        Ok(result)
    }
}
