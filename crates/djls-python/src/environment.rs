use crate::python::{Interpreter, PythonError};
use pyo3::prelude::*;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct PythonEnvironment {
    root: PathBuf,
    build: Interpreter,
    runtime: Interpreter,
}

impl PythonEnvironment {
    fn new(root: PathBuf, build: Interpreter, runtime: Interpreter) -> Self {
        Self {
            root,
            build,
            runtime,
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn build(&self) -> &Interpreter {
        &self.build
    }

    pub fn runtime(&self) -> &Interpreter {
        &self.runtime
    }

    pub fn initialize() -> Result<Self, EnvironmentError> {
        Python::with_gil(|py| {
            let build = Interpreter::for_build(py)?;
            let runtime = Interpreter::for_runtime(build.sys_executable())?;
            let root = runtime.sys_prefix().clone();

            Ok(Self::new(root, build, runtime))
        })
    }
}

impl fmt::Display for PythonEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Python Environment")?;
        writeln!(f, "Root: {}", self.root.display())?;
        writeln!(f)?;
        writeln!(f, "Build Interpreter")?;
        writeln!(f, "{}", self.build)?;
        writeln!(f)?;
        writeln!(f, "Runtime Interpreter")?;
        write!(f, "{}", self.runtime)
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
