use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;
use tempfile::NamedTempFile;

use crate::db::Db as ProjectDb;
use crate::python::Interpreter;

pub trait InspectorRequest: Serialize {
    /// The query name sent to Python (e.g., "templatetags", "`python_env`")
    const NAME: &'static str;
    /// The response type to deserialize into
    type Response: DeserializeOwned;
}

#[derive(Debug, Deserialize)]
pub struct InspectorResponse<T = serde_json::Value> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

pub fn query<Q: InspectorRequest>(db: &dyn ProjectDb, request: &Q) -> Option<Q::Response> {
    let project = db.project()?;
    let interpreter = project.interpreter(db);
    let project_path = project.root(db);
    let django_settings_module = project.django_settings_module(db);

    tracing::debug!(
        "Inspector query '{}': interpreter={:?}, project_path={}, django_settings_module={:?}",
        Q::NAME,
        interpreter,
        project_path,
        django_settings_module
    );

    let inspector = db.inspector();
    match inspector.query::<Q, Q::Response>(
        interpreter,
        project_path,
        django_settings_module.as_deref(),
        request,
    ) {
        Ok(response) if response.ok => {
            tracing::debug!("Inspector query '{}' succeeded with data", Q::NAME);
            response.data
        }
        Ok(response) => {
            tracing::warn!(
                "Inspector query '{}' returned ok=false, error={:?}",
                Q::NAME,
                response.error
            );
            None
        }
        Err(e) => {
            tracing::error!("Inspector query '{}' failed: {}", Q::NAME, e);
            None
        }
    }
}

const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Manages inspector process with automatic cleanup
#[derive(Clone)]
pub struct Inspector {
    inner: Arc<Mutex<InspectorInner>>,
}

impl Inspector {
    #[must_use]
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_IDLE_TIMEOUT)
    }

    #[must_use]
    pub fn with_timeout(idle_timeout: Duration) -> Self {
        let inspector = Self {
            inner: Arc::new(Mutex::new(InspectorInner {
                process: None,
                idle_timeout,
            })),
        };

        // Auto-start cleanup task using a clone
        let cleanup_inspector = inspector.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30));
            let inner = &mut cleanup_inspector.inner();
            if let Some(process) = &inner.process {
                if process.is_idle(inner.idle_timeout) {
                    inner.shutdown_process();
                }
            }
        });

        inspector
    }

    /// Get a lock on the inner state
    ///
    /// # Panics
    ///
    /// Panics if the inspector mutex is poisoned (another thread panicked while holding the lock)
    fn inner(&self) -> std::sync::MutexGuard<'_, InspectorInner> {
        self.inner.lock().expect("Inspector mutex poisoned")
    }

    /// Execute a typed query, reusing existing process if available
    pub fn query<Q: InspectorRequest, R: DeserializeOwned>(
        &self,
        interpreter: &Interpreter,
        project_path: &Utf8Path,
        django_settings_module: Option<&str>,
        request: &Q,
    ) -> Result<InspectorResponse<R>> {
        self.inner()
            .query::<Q, R>(interpreter, project_path, django_settings_module, request)
    }

    /// Manually close the inspector process
    pub fn close(&self) {
        self.inner().shutdown_process();
    }
}

impl Default for Inspector {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Inspector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut inner = self.inner();
        let idle_timeout = inner.idle_timeout;
        let has_process = inner
            .process
            .as_mut()
            .is_some_and(|p| p.is_running() && !p.is_idle(idle_timeout));
        f.debug_struct("Inspector")
            .field("has_active_process", &has_process)
            .finish()
    }
}

struct InspectorInner {
    process: Option<InspectorProcess>,
    idle_timeout: Duration,
}

impl InspectorInner {
    /// Execute a typed query, ensuring a valid process exists
    fn query<Q: InspectorRequest, R: DeserializeOwned>(
        &mut self,
        interpreter: &Interpreter,
        project_path: &Utf8Path,
        django_settings_module: Option<&str>,
        request: &Q,
    ) -> Result<InspectorResponse<R>> {
        self.ensure_process(interpreter, project_path, django_settings_module)?;

        let process = self.process_mut();
        let response = process.query::<Q, R>(request)?;
        process.last_used = Instant::now();

        Ok(response)
    }

    /// Get a mutable reference to the process state, panicking if it doesn't exist
    fn process_mut(&mut self) -> &mut InspectorProcess {
        self.process
            .as_mut()
            .expect("Process should exist after creation")
    }

    /// Ensure a process exists for the given environment
    fn ensure_process(
        &mut self,
        interpreter: &Interpreter,
        project_path: &Utf8Path,
        django_settings_module: Option<&str>,
    ) -> Result<()> {
        let needs_new_process = match &mut self.process {
            None => {
                tracing::debug!("No existing inspector process, spawning new one");
                true
            }
            Some(state) => {
                let not_running = !state.is_running();
                let interpreter_changed = state.interpreter != *interpreter;
                let path_changed = state.project_path != project_path;
                let settings_changed =
                    state.django_settings_module.as_deref() != django_settings_module;

                if not_running || interpreter_changed || path_changed || settings_changed {
                    tracing::debug!(
                        "Inspector process needs restart: not_running={}, interpreter_changed={}, path_changed={}, settings_changed={}",
                        not_running, interpreter_changed, path_changed, settings_changed
                    );
                    true
                } else {
                    false
                }
            }
        };

        if needs_new_process {
            self.shutdown_process();
            tracing::info!(
                "Spawning new inspector process with django_settings_module={:?}",
                django_settings_module
            );
            self.process = Some(InspectorProcess::spawn(
                interpreter.to_owned(),
                project_path.to_path_buf(),
                django_settings_module.map(String::from),
            )?);
        }
        Ok(())
    }

    /// Shutdown the current process if it exists
    fn shutdown_process(&mut self) {
        if let Some(process) = self.process.take() {
            process.shutdown_gracefully();
        }
    }
}

impl Drop for InspectorInner {
    fn drop(&mut self) {
        self.shutdown_process();
    }
}

const INSPECTOR_PYZ: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/dist/djls_inspector.pyz"
));

struct InspectorFile(NamedTempFile);

impl InspectorFile {
    pub fn create() -> Result<Self> {
        let mut zipapp_file = tempfile::Builder::new()
            .prefix("djls_inspector_")
            .suffix(".pyz")
            .tempfile()
            .context("Failed to create temp file for inspector")?;

        zipapp_file
            .write_all(INSPECTOR_PYZ)
            .context("Failed to write inspector zipapp to temp file")?;
        zipapp_file
            .flush()
            .context("Failed to flush inspector zipapp")?;

        Ok(Self(zipapp_file))
    }

    pub fn path(&self) -> &Utf8Path {
        Utf8Path::from_path(self.0.path()).expect("Temp file path should always be valid UTF-8")
    }
}

struct InspectorProcess {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    last_used: Instant,
    interpreter: Interpreter,
    project_path: Utf8PathBuf,
    django_settings_module: Option<String>,
    // keep a handle on the tempfile so it doesn't get cleaned up
    _zipapp_file_handle: InspectorFile,
}

impl InspectorProcess {
    /// Spawn a new inspector process
    pub fn spawn(
        interpreter: Interpreter,
        project_path: Utf8PathBuf,
        django_settings_module: Option<String>,
    ) -> Result<Self> {
        let zipapp_file = InspectorFile::create()?;

        let python_path = interpreter
            .python_path(&project_path)
            .context("Failed to resolve Python interpreter")?;

        let mut cmd = Command::new(&python_path);
        cmd.arg(zipapp_file.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()) // Capture stderr instead of inheriting
            .current_dir(&project_path);

        if let Ok(pythonpath) = std::env::var("PYTHONPATH") {
            let mut paths = vec![project_path.to_string()];
            paths.push(pythonpath);
            cmd.env("PYTHONPATH", paths.join(":"));
        } else {
            cmd.env("PYTHONPATH", project_path.clone());
        }

        // Set Django settings module if we have one
        if let Some(ref module) = django_settings_module {
            tracing::debug!("Setting DJANGO_SETTINGS_MODULE={}", module);
            cmd.env("DJANGO_SETTINGS_MODULE", module);
        } else {
            tracing::warn!("No DJANGO_SETTINGS_MODULE provided to inspector process");
        }

        let mut child = cmd.spawn().context("Failed to spawn inspector process")?;

        let stdin = child.stdin.take().context("Failed to get stdin handle")?;
        let stdout = BufReader::new(child.stdout.take().context("Failed to get stdout handle")?);
        let stderr = BufReader::new(child.stderr.take().context("Failed to get stderr handle")?);

        // Spawn a thread to capture stderr for debugging
        std::thread::spawn(move || {
            let mut stderr = stderr;
            let mut line = String::new();
            while stderr.read_line(&mut line).is_ok() && !line.is_empty() {
                tracing::error!("Inspector stderr: {}", line.trim());
                line.clear();
            }
        });

        tracing::debug!(
            "Inspector process started successfully with zipapp at {:?}",
            zipapp_file.path()
        );

        Ok(Self {
            child,
            stdin,
            stdout,
            last_used: Instant::now(),
            interpreter,
            project_path,
            django_settings_module,
            _zipapp_file_handle: zipapp_file,
        })
    }

    /// Send a typed request and receive a typed response
    pub fn query<Q: InspectorRequest, R: DeserializeOwned>(
        &mut self,
        request: &Q,
    ) -> Result<InspectorResponse<R>> {
        // Build the wire format request
        let wire_request = serde_json::json!({
            "query": Q::NAME,
            "args": request,
        });

        let request_json = serde_json::to_string(&wire_request)?;

        writeln!(self.stdin, "{request_json}")?;
        self.stdin.flush()?;

        let mut response_line = String::new();
        self.stdout
            .read_line(&mut response_line)
            .context("Failed to read response from inspector")?;

        let response: InspectorResponse<R> = match serde_json::from_str(&response_line) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    "Failed to parse inspector response: {}. Raw response: '{}'",
                    e,
                    response_line
                );
                return Err(anyhow::anyhow!("Failed to parse inspector response"));
            }
        };

        Ok(response)
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Check if the process has been idle for longer than the timeout
    pub fn is_idle(&self, timeout: Duration) -> bool {
        self.last_used.elapsed() > timeout
    }

    /// Attempt graceful shutdown of the process
    pub fn shutdown_gracefully(mut self) {
        // Give the process a moment to exit cleanly (100ms total)
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(10));
            if !self.is_running() {
                // Process exited cleanly
                let _ = self.child.wait();
                return;
            }
        }

        // If still running, terminate it
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for InspectorProcess {
    fn drop(&mut self) {
        // Fallback kill if not already shut down gracefully
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inspector_creation() {
        // Test that we can create an inspector
        let _inspector = Inspector::new();
        // Cleanup thread starts automatically
    }

    #[test]
    fn test_inspector_with_custom_timeout() {
        // Test creation with custom timeout
        let _inspector = Inspector::with_timeout(Duration::from_secs(120));
        // Cleanup thread starts automatically
    }

    #[test]
    fn test_inspector_close() {
        let inspector = Inspector::new();
        inspector.close();
        // Process should be closed
    }

    #[test]
    fn test_inspector_cleanup_task_auto_starts() {
        // Test that the cleanup task starts automatically
        let _inspector = Inspector::with_timeout(Duration::from_millis(100));

        // Give it a moment to ensure the thread starts
        std::thread::sleep(Duration::from_millis(10));

        // Can't easily test the actual cleanup behavior in a unit test,
        // but the thread should be running in the background
    }
}
