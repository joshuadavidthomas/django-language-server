pub mod queries;

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
use crate::python::python_environment;
use crate::python::PythonEnvironment;
use queries::Query;

#[derive(Serialize)]
pub struct DjlsRequest {
    #[serde(flatten)]
    pub query: Query,
}

#[derive(Debug, Deserialize)]
pub struct DjlsResponse<T = serde_json::Value> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

/// Run an inspector query and return the JSON result as a string.
///
/// This tracked function executes inspector queries through the shared inspector
/// and caches the results based on project state and query kind.
pub fn inspector_run(db: &dyn ProjectDb, query: Query) -> Option<String> {
    let project = db.project()?;
    let python_env = python_environment(db, project)?;
    let project_path = project.root(db);

    match db
        .inspector()
        .query(&python_env, project_path, &DjlsRequest { query })
    {
        Ok(response) => {
            if response.ok {
                if let Some(data) = response.data {
                    // Convert to JSON string
                    serde_json::to_string(&data).ok()
                } else {
                    None
                }
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// Run an inspector query and return the typed result directly.
///
/// This generic function executes inspector queries and deserializes
/// the response data into the specified type.
#[allow(dead_code)]
pub fn inspector_run_typed<T>(db: &dyn ProjectDb, query: Query) -> Option<T>
where
    T: DeserializeOwned,
{
    let json_str = inspector_run(db, query)?;
    serde_json::from_str(&json_str).ok()
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

    /// Execute a query, reusing existing process if available
    pub fn query(
        &self,
        python_env: &PythonEnvironment,
        project_path: &Utf8Path,
        request: &DjlsRequest,
    ) -> Result<DjlsResponse> {
        self.inner().query(python_env, project_path, request)
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
    /// Execute a query, ensuring a valid process exists
    fn query(
        &mut self,
        python_env: &PythonEnvironment,
        project_path: &Utf8Path,
        request: &DjlsRequest,
    ) -> Result<DjlsResponse> {
        self.ensure_process(python_env, project_path)?;

        let process = self.process_mut();
        let response = process.query(request)?;
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
        python_env: &PythonEnvironment,
        project_path: &Utf8Path,
    ) -> Result<()> {
        let needs_new_process = match &mut self.process {
            None => true,
            Some(state) => {
                !state.is_running()
                    || state.python_env != *python_env
                    || state.project_path != project_path
            }
        };

        if needs_new_process {
            self.shutdown_process();
            self.process = Some(InspectorProcess::spawn(python_env, project_path)?);
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

struct InspectorProcess {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    _zipapp_file: InspectorFile,
    last_used: Instant,
    python_env: PythonEnvironment,
    project_path: Utf8PathBuf,
}

impl InspectorProcess {
    /// Spawn a new inspector process
    pub fn spawn(python_env: &PythonEnvironment, project_path: &Utf8Path) -> Result<Self> {
        let zipapp_file = InspectorFile::create()?;

        let mut cmd = Command::new(&python_env.python_path);
        cmd.arg(zipapp_file.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .current_dir(project_path);

        if let Ok(pythonpath) = std::env::var("PYTHONPATH") {
            let mut paths = vec![project_path.to_string()];
            paths.push(pythonpath);
            cmd.env("PYTHONPATH", paths.join(":"));
        } else {
            cmd.env("PYTHONPATH", project_path);
        }

        if let Ok(settings) = std::env::var("DJANGO_SETTINGS_MODULE") {
            cmd.env("DJANGO_SETTINGS_MODULE", settings);
        } else {
            // Try to detect settings module
            if project_path.join("manage.py").exists() {
                // Look for common settings modules
                for candidate in &["settings", "config.settings", "project.settings"] {
                    let parts: Vec<&str> = candidate.split('.').collect();
                    let mut path = project_path.to_path_buf();
                    for part in &parts[..parts.len() - 1] {
                        path = path.join(part);
                    }
                    if let Some(last) = parts.last() {
                        path = path.join(format!("{last}.py"));
                    }

                    if path.exists() {
                        cmd.env("DJANGO_SETTINGS_MODULE", candidate);
                        break;
                    }
                }
            }
        }

        let mut child = cmd.spawn().context("Failed to spawn inspector process")?;

        let stdin = child.stdin.take().context("Failed to get stdin handle")?;
        let stdout = BufReader::new(child.stdout.take().context("Failed to get stdout handle")?);

        Ok(Self {
            child,
            stdin,
            stdout,
            _zipapp_file: zipapp_file,
            last_used: Instant::now(),
            python_env: python_env.clone(),
            project_path: project_path.to_path_buf(),
        })
    }

    /// Send a request and receive a response
    pub fn query(&mut self, request: &DjlsRequest) -> Result<DjlsResponse> {
        let request_json = serde_json::to_string(request)?;

        writeln!(self.stdin, "{request_json}")?;
        self.stdin.flush()?;

        let mut response_line = String::new();
        self.stdout
            .read_line(&mut response_line)
            .context("Failed to read response from inspector")?;

        let response: DjlsResponse =
            serde_json::from_str(&response_line).context("Failed to parse inspector response")?;

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
