use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, instrument, warn, Instrument};

pub trait Task: Send + 'static {
    type Output: Send + 'static;
    fn run(&self) -> Result<Self::Output>;
}

struct WorkerInner {
    sender: mpsc::Sender<TaskMessage>,
    shutdown_sender: Option<oneshot::Sender<()>>,
}

#[derive(Clone)]
pub struct Worker {
    inner: Arc<WorkerInner>,
}

enum TaskMessage {
    Execute(Box<dyn TaskTrait>),
    WithResult(
        Box<dyn TaskTrait>,
        oneshot::Sender<Result<Box<dyn std::any::Any + Send>>>,
    ),
}

trait TaskTrait: Send {
    fn run_boxed(self: Box<Self>) -> Result<Box<dyn std::any::Any + Send>>;
}

#[async_trait::async_trait]
impl<T: Task> TaskTrait for T {
    #[instrument(skip(self))]
    fn run_boxed(self: Box<Self>) -> Result<Box<dyn std::any::Any + Send>> {
        debug!("Running boxed task");
        self.run()
            .map(|output| Box::new(output) as Box<dyn std::any::Any + Send>)
            .map_err(|e| {
                error!(?e, "Task execution failed");
                e
            })
    }
}

impl Worker {
    #[instrument]
    pub fn new() -> Self {
        let (sender, mut receiver) = mpsc::channel(32);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        debug!("Creating new worker with channel capacity 32");

        tokio::spawn(
            async move {
                info!("Worker task started");
                loop {
                    tokio::select! {
                        Some(msg) = receiver.recv() => {
                            match msg {
                                TaskMessage::Execute(task) => {
                                    debug!("Executing task without result");
                                    if let Err(e) = task.run_boxed() {
                                        error!(?e, "Task execution failed");
                                    }
                                }
                                TaskMessage::WithResult(task, sender) => {
                                    debug!("Executing task with result");
                                    let result = task.run_boxed();
                                    if let Err(e) = &result {
                                        error!(?e, "Task execution failed");
                                    }
                                    if let Err(_) = sender.send(result) {
                                        warn!("Failed to send task result - receiver dropped");
                                    }
                                }
                            }
                        }
                        _ = &mut shutdown_rx => {
                            info!("Worker received shutdown signal");
                            break;
                        }
                    }
                }
                info!("Worker task stopped");
            }
            .in_current_span(),
        );

        Self {
            inner: Arc::new(WorkerInner {
                sender,
                shutdown_sender: Some(shutdown_tx),
            }),
        }
    }

    #[instrument(skip(self, task))]
    pub fn execute<T>(&self, task: T) -> Result<()>
    where
        T: Task + 'static,
    {
        debug!("Attempting to execute task synchronously");
        self.inner
            .sender
            .try_send(TaskMessage::Execute(Box::new(task)))
            .map_err(|e| {
                error!(?e, "Failed to execute task");
                anyhow::anyhow!("Failed to execute task: {}", e)
            })
    }

    #[instrument(skip(self, task))]
    pub async fn submit<T>(&self, task: T) -> Result<()>
    where
        T: Task + 'static,
    {
        debug!("Submitting task asynchronously");
        self.inner
            .sender
            .send(TaskMessage::Execute(Box::new(task)))
            .await
            .map_err(|e| {
                error!(?e, "Failed to submit task");
                anyhow::anyhow!("Failed to submit task: {}", e)
            })
    }

    #[instrument(skip(self, task))]
    pub async fn wait_for<T>(&self, task: T) -> Result<T::Output>
    where
        T: Task + 'static,
    {
        debug!("Submitting task with result");
        let (tx, rx) = oneshot::channel();

        self.inner
            .sender
            .send(TaskMessage::WithResult(Box::new(task), tx))
            .await
            .map_err(|e| {
                error!(?e, "Failed to send task");
                anyhow::anyhow!("Failed to send task: {}", e)
            })?;

        debug!("Waiting for task result");
        let result = rx.await.map_err(|e| {
            error!(?e, "Failed to receive task result");
            anyhow::anyhow!("Failed to receive result: {}", e)
        })??;

        debug!("Downcasting task result");
        result.downcast().map(|b| *b).map_err(|_| {
            error!("Failed to downcast task result");
            anyhow::anyhow!("Failed to downcast result")
        })
    }
}

impl Drop for WorkerInner {
    fn drop(&mut self) {
        debug!("WorkerInner being dropped");
        if let Some(sender) = self.shutdown_sender.take() {
            if let Err(_) = sender.send(()) {
                warn!("Failed to send shutdown signal - receiver already dropped");
            }
        }
    }
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
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
            Ok(self.0 * 2)
        }
    }

    // Basic functionality tests
    #[tokio::test]
    async fn test_wait_for() {
        let worker = Worker::new();
        let result = worker.wait_for(TestTask(21)).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_submit() {
        let worker = Worker::new();
        for i in 0..32 {
            assert!(worker.execute(TestTask(i)).is_ok());
        }
        assert!(worker.execute(TestTask(33)).is_err());
        assert!(worker.submit(TestTask(33)).await.is_ok());
        sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_execute() {
        let worker = Worker::new();
        assert!(worker.execute(TestTask(21)).is_ok());
        sleep(Duration::from_millis(50)).await;
    }

    // Test channel backpressure
    #[tokio::test]
    async fn test_channel_backpressure() {
        let worker = Worker::new();

        // Fill the channel (channel size is 32)
        for i in 0..32 {
            assert!(worker.execute(TestTask(i)).is_ok());
        }

        // Next execute should fail
        assert!(worker.execute(TestTask(33)).is_err());

        // But wait_for should eventually succeed
        let result = worker.wait_for(TestTask(33)).await.unwrap();
        assert_eq!(result, 66);
    }

    // Test concurrent tasks
    #[tokio::test]
    async fn test_concurrent_tasks() {
        let worker = Worker::new();
        let mut handles = Vec::new();

        // Spawn multiple concurrent tasks
        for i in 0..10 {
            let worker = worker.clone();
            let handle = tokio::spawn(async move {
                let result = worker.wait_for(TestTask(i)).await.unwrap();
                assert_eq!(result, i * 2);
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }
    }

    // Test shutdown behavior
    #[tokio::test]
    async fn test_shutdown() {
        {
            let worker = Worker::new();
            worker.execute(TestTask(1)).unwrap();
            worker.wait_for(TestTask(2)).await.unwrap();
            // Worker will be dropped here, triggering shutdown
        }
        sleep(Duration::from_millis(50)).await;
    }

    // Test error handling
    struct ErrorTask;

    impl Task for ErrorTask {
        type Output = (); // Unit type for error test

        fn run(&self) -> Result<Self::Output> {
            Err(anyhow!("Task failed"))
        }
    }

    #[tokio::test]
    async fn test_error_handling() {
        let worker = Worker::new();

        // Test error propagation
        assert!(worker.wait_for(ErrorTask).await.is_err());

        // Test that worker continues to function after error
        let result = worker.wait_for(TestTask(21)).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_worker_cloning() {
        let worker = Worker::new();
        let worker2 = worker.clone();

        let (result1, result2) = tokio::join!(
            worker.wait_for(TestTask(21)),
            worker2.wait_for(TestTask(42))
        );

        assert_eq!(result1.unwrap(), 42);
        assert_eq!(result2.unwrap(), 84);
    }

    #[tokio::test]
    async fn test_multiple_workers() {
        let worker = Worker::new();
        let mut handles = Vec::new();

        for i in 0..10 {
            let worker = worker.clone();
            let handle = tokio::spawn(async move {
                let result = worker.wait_for(TestTask(i)).await.unwrap();
                assert_eq!(result, i * 2);
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }
    }
}
