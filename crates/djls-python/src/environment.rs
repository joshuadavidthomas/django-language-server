use crate::python::{Interpreter, PythonError};
use pyo3::prelude::*;
use std::fmt;
use std::path::PathBuf;

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
        Python::with_gil(|pyo3_py| {
            let sys = pyo3_py.import("sys")?;
            let executable = PathBuf::from(sys.getattr("executable")?.extract::<String>()?);
            let py = Interpreter::from_sys_executable(&executable)?;
            let root = py.sys_prefix().clone();
            Ok(Self::new(root, py))
        })
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
    #[error("Python error: {0}")]
    Python(#[from] PyErr),

    #[error("Runtime error: {0}")]
    Runtime(#[from] PythonError),

    #[error("Environment initialization failed: {0}")]
    Init(String),
}
