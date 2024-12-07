use crate::python::{Python, PythonError};
use serde::ser::Error;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub trait Runner {
    fn get_executable(&self) -> &Path;

    fn run_module(&self, command: &str) -> std::io::Result<String> {
        let output = Command::new(self.get_executable())
            .arg("-m")
            .arg(command)
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Command failed: {}", error),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn run_module_with_args(&self, command: &str, args: &[&str]) -> std::io::Result<String> {
        let output = Command::new(self.get_executable())
            .arg("-m")
            .arg(command)
            .args(args)
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Command failed: {}", error),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn run_python_code(&self, code: &str) -> std::io::Result<String> {
        let output = Command::new(self.get_executable())
            .args(["-c", code])
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Python execution failed: {}", error),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn run_python_code_with_args(&self, code: &str, args: &str) -> std::io::Result<String> {
        let output = Command::new(self.get_executable())
            .args(["-c", code, args])
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Python execution failed: {}", error),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn run_script<T>(&self, script: &str) -> Result<T, serde_json::Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        let result = self
            .run_python_code(script)
            .map_err(|e| serde_json::Error::custom(e.to_string()))?;
        serde_json::from_str(&result)
    }

    fn run_script_with_args<T>(&self, script: &str, args: &str) -> Result<T, serde_json::Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        let result = self
            .run_python_code_with_args(script, args)
            .map_err(|e| serde_json::Error::custom(e.to_string()))?;
        serde_json::from_str(&result)
    }
}

pub struct SimpleRunner {
    executable: PathBuf,
}

impl SimpleRunner {
    pub fn new(executable: PathBuf) -> Self {
        Self { executable }
    }
}

impl Runner for SimpleRunner {
    fn get_executable(&self) -> &Path {
        &self.executable
    }
}

pub trait ScriptRunner: Sized {
    const SCRIPT: &'static str;

    fn run_with_exe<R: Runner>(runner: &R) -> Result<Self, RunnerError>
    where
        Self: for<'de> Deserialize<'de>,
    {
        let result = runner.run_script(Self::SCRIPT).map_err(RunnerError::from)?;
        Ok(result)
    }

    fn run_with_exe_args<R: Runner>(runner: &R, args: &str) -> Result<Self, RunnerError>
    where
        Self: for<'de> Deserialize<'de>,
    {
        let result = runner
            .run_script_with_args(Self::SCRIPT, args)
            .map_err(RunnerError::from)?;
        Ok(result)
    }

    fn run_with_path(executable: &Path) -> Result<Self, RunnerError>
    where
        Self: for<'de> Deserialize<'de>,
    {
        let runner = &SimpleRunner::new(executable.to_path_buf());
        Self::run_with_exe(runner)
    }

    fn run_with_path_args(executable: &Path, args: &str) -> Result<Self, RunnerError>
    where
        Self: for<'de> Deserialize<'de>,
    {
        let runner = &SimpleRunner::new(executable.to_path_buf());
        Self::run_with_exe_args(runner, args)
    }

    fn run_with_py(python: &Python) -> Result<Self, RunnerError>
    where
        Self: for<'de> Deserialize<'de>,
    {
        Self::run_with_exe(python)
    }

    fn run_with_py_args(python: &Python, args: &str) -> Result<Self, RunnerError>
    where
        Self: for<'de> Deserialize<'de>,
    {
        Self::run_with_exe_args(python, args)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Python(#[from] PythonError),

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

#[macro_export]
macro_rules! include_script {
    ($name:expr) => {
        include_str!(concat!(
            env!("CARGO_WORKSPACE_DIR"),
            "/python/djls/",
            $name,
            ".py"
        ))
    };
}
