use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use anyhow::Context;
use anyhow::Result;
use serde_json;

use super::zipapp::InspectorFile;
use super::DjlsRequest;
use super::DjlsResponse;
use crate::python::PythonEnvironment;

pub struct InspectorProcess {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    _zipapp_file: InspectorFile,
}

impl InspectorProcess {
    pub fn new(python_env: &PythonEnvironment, project_path: &Path) -> Result<Self> {
        let zipapp_file = InspectorFile::create()?;

        let mut cmd = Command::new(&python_env.python_path);
        cmd.arg(zipapp_file.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .current_dir(project_path);

        if let Ok(pythonpath) = std::env::var("PYTHONPATH") {
            let mut paths = vec![project_path.to_string_lossy().to_string()];
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
}

impl Drop for InspectorProcess {
    fn drop(&mut self) {
        // Try to terminate the child process gracefully
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
