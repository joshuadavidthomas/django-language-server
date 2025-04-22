use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub trait Task: Send + 'static {
    type Output: Send + 'static;
    fn run(&self) -> Result<Self::Output>;
}

trait TaskTrait: Send {
    fn run_boxed(self: Box<Self>);
}

impl<T: Task> TaskTrait for T {
    fn run_boxed(self: Box<Self>) {
        match self.run() {
            Ok(_) => { /* Task succeeded, do nothing */ }
            Err(e) => {
                // Log the error if the task failed.
                // Consider adding a proper logging mechanism later.
                eprintln!("Task failed: {}", e);
            }
        }
    }
}

#[derive(Clone)]
pub struct Queue {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    sender: mpsc::Sender<Box<dyn TaskTrait>>,
    shutdown_sender: Option<oneshot::Sender<()>>,
}

impl Queue {
    pub fn new() -> Self {
        let (sender, mut receiver) = mpsc::channel::<Box<dyn TaskTrait>>(32); // Channel for tasks
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(task) = receiver.recv() => {
                        task.run_boxed();
                    }
                    _ = &mut shutdown_rx => {
                        // Drain the channel before shutting down? Optional.
                        // For now, just break.
                        break;
                    }
                    else => break,
                }
            }
        });

        Self {
            inner: Arc::new(QueueInner {
                sender,
                shutdown_sender: Some(shutdown_tx),
            }),
        }
    }

    /// Submits a task to the queue asynchronously, waiting if the channel is full.
    /// The task is executed in the background, and its result is ignored.
    pub async fn submit<T>(&self, task: T) -> Result<()>
    where
        T: Task + 'static,
    {
        self.inner
            .sender
            .send(Box::new(task))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit task: {}", e))
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for QueueInner {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            sender.send(()).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::time::Duration;
    use tokio::time::sleep;

    struct TestTask(i32);
    impl Task for TestTask {
        type Output = i32;
        fn run(&self) -> Result<Self::Output> {
            std::thread::sleep(Duration::from_millis(10));
            Ok(self.0 * 2)
        }
    }

    struct ErrorTask;
    impl Task for ErrorTask {
        type Output = ();
        fn run(&self) -> Result<Self::Output> {
            Err(anyhow!("Task failed intentionally"))
        }
    }

    #[tokio::test]
    async fn test_submit_and_process() {
        let queue = Queue::new();
        // Submit a few tasks
        for i in 0..5 {
            queue.submit(TestTask(i)).await.unwrap();
        }
        // Submit a task that will fail
        queue.submit(ErrorTask).await.unwrap();

        // Allow some time for tasks to be processed by the background worker.
        // In a real scenario, you might not wait like this, but for testing,
        // we need to ensure the background task has a chance to run.
        sleep(Duration::from_millis(100)).await;

        // We can't directly assert results here, but we can check the queue still works.
        queue.submit(TestTask(10)).await.unwrap();
        sleep(Duration::from_millis(50)).await; // Allow time for the last task
    }

    #[tokio::test]
    async fn test_channel_backpressure_submit() {
        let queue = Queue::new();

        // Fill the channel (channel size is 32) using submit
        let mut tasks = Vec::new();
        for i in 0..32 {
            let queue_clone = queue.clone();
            // Spawn tasks to submit concurrently, as submit waits
            tasks.push(tokio::spawn(async move {
                queue_clone
                    .submit(TestTask(i))
                    .await
                    .expect("Submit should succeed");
            }));
        }
        // Wait for all initial submissions to likely be sent (though not necessarily processed)
        for task in tasks {
            task.await.unwrap();
        }

        // Try submitting one more task. This should wait until a slot is free.
        // We'll use a timeout to ensure it doesn't block forever if something is wrong.
        let submit_task = queue.submit(TestTask(33));
        match tokio::time::timeout(Duration::from_millis(200), submit_task).await {
            Ok(Ok(_)) => { /* Successfully submitted after waiting */ }
            Ok(Err(e)) => panic!("Submit failed unexpectedly: {}", e),
            Err(_) => panic!("Submit timed out, likely blocked due to backpressure not resolving"),
        }

        // Allow time for processing
        sleep(Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_shutdown() {
        let queue = Queue::new();
        queue.submit(TestTask(1)).await.unwrap();
        queue.submit(TestTask(2)).await.unwrap();
        // Queue is dropped here, triggering shutdown
        drop(queue);

        // Allow time for shutdown signal to be processed and potentially
        // for the background task to finish ongoing work (though not guaranteed here).
        sleep(Duration::from_millis(100)).await;
        // No direct assertion, just checking it doesn't panic/hang.
    }

    #[tokio::test]
    async fn test_queue_cloning() {
        let queue1 = Queue::new();
        let queue2 = queue1.clone();

        // Submit tasks via both clones
        let task1 = queue1.submit(TestTask(10));
        let task2 = queue2.submit(TestTask(20));

        // Wait for submissions to complete
        tokio::try_join!(task1, task2).unwrap();

        // Allow time for processing
        sleep(Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_error_task_does_not_stop_queue() {
        let queue = Queue::new();

        queue.submit(TestTask(1)).await.unwrap();
        queue.submit(ErrorTask).await.unwrap(); // Submit the failing task
        queue.submit(TestTask(2)).await.unwrap();

        // Allow time for tasks to process
        sleep(Duration::from_millis(100)).await;

        // Submit another task to ensure the queue is still running after the error
        queue.submit(TestTask(3)).await.unwrap();
        sleep(Duration::from_millis(50)).await;
        // If we reach here without panic, the queue continued after the error.
        // We expect an error message "Task failed: Task failed intentionally"
        // to be printed to stderr during the test run.
    }
}
