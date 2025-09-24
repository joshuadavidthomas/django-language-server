use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::anyhow;
use anyhow::Result;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// Type alias for a type-erased, pinned, heap-allocated, Send-able future
/// that resolves to `Result<()>`.
///
/// This allows storing different concrete `Future` types (resulting from
/// various `async` blocks) in a uniform way, suitable for sending over a channel
/// or storing in collections, as long as they meet the `Send` bound and
/// produce the expected `Result<()>`.
///
/// - `Pin`: Ensures the `Future`'s memory location is stable, which is often
///   required for self-referential `async` blocks.
/// - `Box`: Allocates the `Future` on the heap.
/// - `dyn Future`: Type erasure - hides the specific concrete `Future` type.
/// - `+ Send`: Ensures the `Future` can be safely sent across threads and required
///   by the tower-lsp-server LSP server trait bounds, even in our single-threaded
///   runtime.
type TaskFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// Type alias for a type-erased, heap-allocated, Send-able closure that,
/// when called, returns a `TaskFuture`.
///
/// This represents a unit of work that can be sent to the queue's worker task.
/// The closure captures any necessary data and contains the logic to start
/// the asynchronous operation, returning it as a `TaskFuture`.
///
/// - `Box`: Allocates the closure on the heap.
/// - `dyn FnOnce()`: Type erasure - hides the specific closure type. It takes no
///   arguments.
/// - `-> TaskFuture`: Specifies that calling the closure produces the type-erased future.
/// - `+ Send + 'static`: Ensures the closure itself can be safely sent across
///   threads and has a static lifetime (doesn't borrow short-lived data). Required
///   for compatibility with our async runtime and LSP traits.
type TaskClosure = Box<dyn FnOnce() -> TaskFuture + Send + 'static>;

/// A simple asynchronous task queue for sequential execution.
///
/// Tasks are submitted as closures that return futures. These closures are sent
/// to a dedicated worker task which executes them one at a time in the order
/// they were received. This ensures sequential processing of background tasks.
///
/// The queue runs within our single-threaded runtime but maintains compatibility
/// with the Send+Sync requirements of the LSP. This provides the benefits of
/// simpler execution while maintaining the required trait bounds.
///
/// Shutdown is handled gracefully when the last `Queue` instance is dropped.
#[derive(Clone)]
pub struct Queue {
    inner: Arc<QueueInner>,
}

/// Internal state of the queue, managed by an Arc for shared ownership.
struct QueueInner {
    /// The sender half of the MPSC channel used to send tasks (as closures)
    /// to the worker task.
    sender: mpsc::Sender<TaskClosure>,
    /// The sender half of a oneshot channel used to signal the worker task
    /// to shut down when the `QueueInner` is dropped.
    shutdown_sender: Option<oneshot::Sender<()>>,
}

impl Queue {
    /// Creates a new `Queue` and spawns its background worker task.
    ///
    /// The worker task runs indefinitely, waiting for tasks on the MPSC channel
    /// or a shutdown signal. Received tasks (closures) are executed sequentially.
    /// If a task's future resolves to an `Err`, the error is printed to stderr.
    pub fn new() -> Self {
        // Create the channel for sending task closures. Bounded to 32 pending tasks.
        let (sender, mut receiver) = mpsc::channel::<TaskClosure>(32);
        // Create the channel for signaling shutdown.
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        // Spawn the worker task.
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Bias selection towards shutdown signal if available
                    // (though default select! behavior is unspecified randomization)
                    // biased; // Uncomment if strict shutdown priority is needed

                    // Wait for the shutdown signal.
                    _ = &mut shutdown_rx => {
                        // Shutdown signal received, break the loop.
                        // Consider draining the receiver here if pending tasks
                        // should be completed before shutdown.
                        break;
                    }
                    // Wait for a task closure from the channel.
                    maybe_task_closure = receiver.recv() => {
                        if let Some(task_closure) = maybe_task_closure {
                            // Received a task closure. Execute it to get the future.
                            let task_future: TaskFuture = task_closure();
                            // Await the future's completion.
                            if let Err(e) = task_future.await {
                                // Log the error if the task failed.
                                // TODO: Integrate with a proper logging framework.
                                eprintln!("Task failed: {e}");
                            }
                        } else {
                            // Channel closed, implies all senders (Queue instances)
                            // are dropped. Break the loop.
                            break;
                        }
                    }
                }
            }
            eprintln!("Queue worker task shutting down");
        });

        Self {
            inner: Arc::new(QueueInner {
                sender,
                shutdown_sender: Some(shutdown_tx),
            }),
        }
    }

    /// Submits an asynchronous task to the queue.
    ///
    /// This method accepts a `Future` directly and sends it to the background worker
    /// task for sequential execution. The future should resolve to `Result<()>`.
    ///
    /// The `await` on this method only waits for the task to be *sent* to the
    /// queue's channel, not for the task to be *executed*. If the queue's
    /// channel is full, this method will wait until space becomes available.
    ///
    /// # Usage
    ///
    /// The `future` must be a `Future` which resolves to `Result<()>`. Typically,
    /// this is provided using an `async move` block:
    ///
    /// ```rust,ignore
    /// let data_to_capture = 42;
    /// queue.submit(async move {
    ///     // ... perform async work using data_to_capture ...
    ///     println!("Processing data: {}", data_to_capture);
    ///     tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    ///     Ok(()) // Indicate success
    /// }).await?;
    /// ```
    ///
    /// # Type Parameters
    ///
    /// - `F`: The type of the `Future`. Must resolve to `Result<()>` and be `Send + 'static`.
    pub async fn submit<F>(&self, future: F) -> Result<()>
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        // Create the inner closure that matches `TaskClosure`'s signature.
        // This closure wraps the future in a way that TaskClosure expects.
        let boxed_task_closure: TaskClosure = Box::new(move || Box::pin(future));

        // Send the boxed, type-erased closure to the worker task.
        // This will wait if the channel buffer is full.
        self.inner
            .sender
            .send(boxed_task_closure)
            .await
            .map_err(|e| {
                // Error likely means the receiver (worker task) has panicked or shut down.
                anyhow!("Failed to submit task: queue receiver closed ({e})")
            })
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

/// Handles cleanup when the last `Queue` reference is dropped.
impl Drop for QueueInner {
    fn drop(&mut self) {
        // Take the shutdown sender (if it hasn't already been taken or failed).
        if let Some(sender) = self.shutdown_sender.take() {
            // Send the shutdown signal.
            // `.ok()` ignores the result, as the receiver might have already
            // terminated if the channel closed naturally or panicked.
            sender.send(()).ok();
            eprintln!("Sent shutdown signal to queue worker");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use anyhow::anyhow;
    use tokio::time::sleep;

    use super::*;

    #[tokio::test]
    async fn test_submit_and_process() {
        let queue = Queue::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // Submit a few tasks
        for i in 0..5 {
            let counter_clone = Arc::clone(&counter);
            queue
                .submit(async move {
                    sleep(Duration::from_millis(5)).await;
                    println!("Processing task {i}");
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .unwrap();
        }

        // Submit a task that will fail
        queue
            .submit(async {
                println!("Submitting failing task");
                Err(anyhow!("Task failed intentionally"))
            })
            .await
            .unwrap();

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 5);

        // Submit another task
        let counter_clone = Arc::clone(&counter);
        queue
            .submit(async move {
                println!("Processing task after error");
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();

        sleep(Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn test_channel_backpressure_submit() {
        let queue = Queue::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for i in 0..32 {
            let queue_clone = queue.clone();
            let counter_clone = Arc::clone(&counter);
            tasks.push(tokio::spawn(async move {
                queue_clone
                    .submit(async move {
                        counter_clone.fetch_add(1, Ordering::Relaxed);
                        sleep(Duration::from_millis(2)).await;
                        Ok(())
                    })
                    .await
                    .expect("Submit should succeed");
                println!("Submitted task {i}");
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        println!("Finished submitting initial 32 tasks");

        let counter_clone = Arc::clone(&counter);
        let submit_task = queue.submit(async move {
            println!("Processing the 33rd task");
            counter_clone.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });

        #[cfg(windows)]
        let timeout_ms = 1000;
        #[cfg(not(windows))]
        let timeout_ms = 500;

        match tokio::time::timeout(Duration::from_millis(timeout_ms), submit_task).await {
            Ok(Ok(())) => {
                println!("Successfully submitted 33rd task");
            }
            Ok(Err(e)) => panic!("Submit failed unexpectedly: {e}"),
            Err(timeout_err) => panic!(
                "Submit timed out: {timeout_err}, likely blocked due to backpressure not resolving"
            ),
        }

        #[cfg(windows)]
        sleep(Duration::from_millis(1000)).await;
        #[cfg(not(windows))]
        sleep(Duration::from_millis(200)).await;

        assert_eq!(counter.load(Ordering::Relaxed), 33);
    }

    #[tokio::test]
    async fn test_shutdown() {
        let queue = Queue::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone1 = Arc::clone(&counter);
        queue
            .submit(async move {
                sleep(Duration::from_millis(50)).await;
                counter_clone1.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();

        let counter_clone2 = Arc::clone(&counter);
        queue
            .submit(async move {
                sleep(Duration::from_millis(50)).await;
                counter_clone2.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();

        drop(queue);
        sleep(Duration::from_millis(200)).await;

        let final_count = counter.load(Ordering::SeqCst);
        println!("Final count after shutdown: {final_count}");
        assert!(final_count <= 2);
    }

    #[tokio::test]
    async fn test_queue_cloning() {
        let queue1 = Queue::new();
        let queue2 = queue1.clone();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone1 = Arc::clone(&counter);
        let task1 = queue1.submit(async move {
            counter_clone1.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        let counter_clone2 = Arc::clone(&counter);
        let task2 = queue2.submit(async move {
            counter_clone2.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        tokio::try_join!(task1, task2).unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_error_task_does_not_stop_queue() {
        let queue = Queue::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone1 = Arc::clone(&counter);
        queue
            .submit(async move {
                counter_clone1.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();

        queue
            .submit(async { Err(anyhow!("Intentional failure")) })
            .await
            .unwrap();

        let counter_clone2 = Arc::clone(&counter);
        queue
            .submit(async move {
                counter_clone2.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();

        sleep(Duration::from_millis(100)).await;

        let counter_clone3 = Arc::clone(&counter);
        queue
            .submit(async move {
                counter_clone3.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();

        sleep(Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
}
