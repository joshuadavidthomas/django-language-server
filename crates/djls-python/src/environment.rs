use crate::python::{Interpreter, PythonError};
use std::fmt;
use std::path::PathBuf;
use which::which;

#[derive(Debug)]
pub struct PythonEnvironment {
    root: PathBuf,
    py: Interpreter,
}

impl PythonEnvironment {
    fn new(root: PathBuf, py: Interpreter) -> Self {
        Self { root, py }
    }

    pub fn initialize() -> Result<Self, EnvironmentError> {
        let executable = which("python")?;
        let py = Interpreter::from_sys_executable(&executable)?;
        let root = py.sys_prefix().clone();
        Ok(Self::new(root, py))
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn py(&self) -> &Interpreter {
        &self.py
    }
}

impl fmt::Display for PythonEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Python Environment")?;
        writeln!(f, "Root: {}", self.root.display())?;
        writeln!(f)?;
        writeln!(f, "Interpreter")?;
        writeln!(f, "{}", self.py)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("Failed to locate Python executable: {0}")]
    PythonNotFound(#[from] which::Error),

    #[error("Runtime error: {0}")]
    Runtime(#[from] PythonError),

    #[error("Environment initialization failed: {0}")]
    Init(String),
}
