use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;

use super::ipc::InspectorProcess;
use super::DjlsRequest;
use super::DjlsResponse;
use crate::python::PythonEnvironment;

/// Global singleton pool for convenience
static GLOBAL_POOL: std::sync::OnceLock<InspectorPool> = std::sync::OnceLock::new();

/// Get or create the global inspector pool
pub fn global_pool() -> &'static InspectorPool {
    GLOBAL_POOL.get_or_init(InspectorPool::new)
}
/// Default idle timeout in seconds
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Manages a pool of inspector processes with automatic cleanup
#[derive(Clone)]
pub struct InspectorPool {
    inner: Arc<Mutex<InspectorPoolInner>>,
}

impl std::fmt::Debug for InspectorPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectorPool")
            .field("has_active_process", &self.has_active_process())
            .finish()
    }
}

struct InspectorPoolInner {
    process: Option<InspectorProcessHandle>,
    idle_timeout: Duration,
}

struct InspectorProcessHandle {
    process: InspectorProcess,
    last_used: Instant,
    python_env: PythonEnvironment,
    project_path: std::path::PathBuf,
}

impl InspectorPool {
    /// Create a new inspector pool with default idle timeout (60 seconds)
    #[must_use]
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_IDLE_TIMEOUT)
    }

    /// Create a new inspector pool with custom idle timeout
    #[must_use]
    pub fn with_timeout(idle_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InspectorPoolInner {
                process: None,
                idle_timeout,
            })),
        }
    }

    /// Execute a query, reusing existing process if available and not idle
    ///
    /// # Panics
    ///
    /// Panics if the inspector pool mutex is poisoned (another thread panicked while holding the lock)
    pub fn query(
        &self,
        python_env: &PythonEnvironment,
        project_path: &Path,
        request: &DjlsRequest,
    ) -> Result<DjlsResponse> {
        let mut inner = self.inner.lock().expect("Inspector pool mutex poisoned");
        let idle_timeout = inner.idle_timeout;

        // Check if we need to drop the existing process
        let need_new_process = if let Some(handle) = &mut inner.process {
            // Check various conditions
            let idle_too_long = handle.last_used.elapsed() > idle_timeout;
            let not_running = !handle.process.is_running();
            let different_env =
                handle.python_env != *python_env || handle.project_path != project_path;

            idle_too_long || not_running || different_env
        } else {
            true
        };

        if need_new_process {
            inner.process = None;
        }

        // Get or create process
        if inner.process.is_none() {
            // Create new process
            let process = InspectorProcess::new(python_env, project_path)?;
            inner.process = Some(InspectorProcessHandle {
                process,
                last_used: Instant::now(),
                python_env: python_env.clone(),
                project_path: project_path.to_path_buf(),
            });
        }

        // Now we can safely get a mutable reference
        let handle = inner
            .process
            .as_mut()
            .expect("Process should exist after creation");

        // Execute query
        let response = handle.process.query(request)?;
        handle.last_used = Instant::now();

        Ok(response)
    }

    /// Manually close the inspector process
    ///
    /// # Panics
    ///
    /// Panics if the inspector pool mutex is poisoned
    pub fn close(&self) {
        let mut inner = self.inner.lock().expect("Inspector pool mutex poisoned");
        inner.process = None;
    }

    /// Check if there's an active process
    ///
    /// # Panics
    ///
    /// Panics if the inspector pool mutex is poisoned
    #[must_use]
    pub fn has_active_process(&self) -> bool {
        let mut inner = self.inner.lock().expect("Inspector pool mutex poisoned");
        if let Some(handle) = &mut inner.process {
            handle.process.is_running() && handle.last_used.elapsed() <= inner.idle_timeout
        } else {
            false
        }
    }

    /// Get the configured idle timeout
    ///
    /// # Panics
    ///
    /// Panics if the inspector pool mutex is poisoned
    #[must_use]
    pub fn idle_timeout(&self) -> Duration {
        let inner = self.inner.lock().expect("Inspector pool mutex poisoned");
        inner.idle_timeout
    }

    /// Start a background cleanup task that periodically checks for idle processes
    ///
    /// # Panics
    ///
    /// The spawned thread will panic if the inspector pool mutex is poisoned
    pub fn start_cleanup_task(self: Arc<Self>) {
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30)); // Check every 30 seconds

                let mut inner = self.inner.lock().expect("Inspector pool mutex poisoned");
                if let Some(handle) = &inner.process {
                    if handle.last_used.elapsed() > inner.idle_timeout {
                        // Process is idle, drop it
                        inner.process = None;
                    }
                }
            }
        });
    }
}

impl Default for InspectorPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let pool = InspectorPool::new();
        assert_eq!(pool.idle_timeout(), DEFAULT_IDLE_TIMEOUT);
        assert!(!pool.has_active_process());
    }

    #[test]
    fn test_pool_with_custom_timeout() {
        let timeout = Duration::from_secs(120);
        let pool = InspectorPool::with_timeout(timeout);
        assert_eq!(pool.idle_timeout(), timeout);
    }

    #[test]
    fn test_pool_close() {
        let pool = InspectorPool::new();
        pool.close();
        assert!(!pool.has_active_process());
    }
}
