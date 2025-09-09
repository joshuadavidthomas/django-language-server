use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use serde_json;
use tempfile::NamedTempFile;

use super::{DjlsRequest, DjlsResponse};
use crate::python::PythonEnvironment;

// Embed the inspector zipapp at compile time
const INSPECTOR_PYZ: &[u8] = include_bytes!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "/python/dist/djls_inspector.pyz"
));

/// Inspector process that communicates with the Python zipapp
pub struct InspectorProcess {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    _zipapp_file: NamedTempFile, // Keep the temp file alive for the process lifetime
}

impl InspectorProcess {
    /// Start a new inspector process
    pub fn new(python_env: &PythonEnvironment, project_path: &Path) -> Result<Self> {
        // Write the embedded zipapp to a temp file with .pyz extension
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
        
        let zipapp_path = zipapp_file.path();

        let mut cmd = Command::new(&python_env.python_path);
        cmd.arg(zipapp_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .current_dir(project_path);

        // Set up Python environment variables
        if let Ok(pythonpath) = std::env::var("PYTHONPATH") {
            let mut paths = vec![project_path.to_string_lossy().to_string()];
            paths.push(pythonpath);
            cmd.env("PYTHONPATH", paths.join(":"));
        } else {
            cmd.env("PYTHONPATH", project_path);
        }

        // Set Django settings module if available
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
                    path = path.join(format!("{}.py", parts.last().unwrap()));

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
        })
    }

    /// Send a request and receive a response
    pub fn query(&mut self, request: &DjlsRequest) -> Result<DjlsResponse> {
        // Serialize request to JSON
        let request_json = serde_json::to_string(request)?;

        // Send request (with newline)
        writeln!(self.stdin, "{request_json}")?;
        self.stdin.flush()?;

        // Read response
        let mut response_line = String::new();
        self.stdout
            .read_line(&mut response_line)
            .context("Failed to read response from inspector")?;

        // Parse response
        let response: DjlsResponse =
            serde_json::from_str(&response_line).context("Failed to parse inspector response")?;

        Ok(response)
    }

    /// Check if the process is still running
    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true, // Still running
            _ => false,       // Exited or error
        }
    }
}

impl Drop for InspectorProcess {
    fn drop(&mut self) {
        // Try to terminate the child process gracefully
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
