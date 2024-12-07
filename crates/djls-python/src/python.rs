use crate::include_script;
use crate::packaging::{Packages, PackagingError};
use crate::runner::{Runner, RunnerError, ScriptRunner};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};
use which::which;

#[derive(Clone, Debug, Deserialize)]
pub struct VersionInfo {
    major: u8,
    minor: u8,
    patch: u8,
    suffix: Option<String>,
}

impl fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(suffix) = &self.suffix {
            write!(f, "{}", suffix)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SysconfigPaths {
    data: PathBuf,
    include: PathBuf,
    platinclude: PathBuf,
    platlib: PathBuf,
    platstdlib: PathBuf,
    purelib: PathBuf,
    scripts: PathBuf,
    stdlib: PathBuf,
}

impl fmt::Display for SysconfigPaths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "data: {}", self.data.display())?;
        writeln!(f, "include: {}", self.include.display())?;
        writeln!(f, "platinclude: {}", self.platinclude.display())?;
        writeln!(f, "platlib: {}", self.platlib.display())?;
        writeln!(f, "platstdlib: {}", self.platstdlib.display())?;
        writeln!(f, "purelib: {}", self.purelib.display())?;
        writeln!(f, "scripts: {}", self.scripts.display())?;
        write!(f, "stdlib: {}", self.stdlib.display())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Python {
    version_info: VersionInfo,
    sysconfig_paths: SysconfigPaths,
    sys_prefix: PathBuf,
    sys_base_prefix: PathBuf,
    sys_executable: PathBuf,
    sys_path: Vec<PathBuf>,
    packages: Packages,
}

#[derive(Debug, Deserialize)]
pub struct PythonSetup(Python);

impl ScriptRunner for PythonSetup {
    const SCRIPT: &'static str = include_script!("python_setup");
}

impl From<PythonSetup> for Python {
    fn from(setup: PythonSetup) -> Self {
        setup.0
    }
}

impl Python {
    pub fn initialize() -> Result<Self, PythonError> {
        let executable = which("python")?;
        Ok(PythonSetup::run_with_path(&executable)?.into())
    }
}

impl Runner for Python {
    fn get_executable(&self) -> &Path {
        &self.sys_executable
    }
}

impl fmt::Display for Python {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Version: {}", self.version_info)?;
        writeln!(f, "Executable: {}", self.sys_executable.display())?;
        writeln!(f, "Prefix: {}", self.sys_prefix.display())?;
        writeln!(f, "Base Prefix: {}", self.sys_base_prefix.display())?;
        writeln!(f, "Paths:")?;
        for path in &self.sys_path {
            writeln!(f, "{}", path.display())?;
        }
        writeln!(f, "Sysconfig Paths:")?;
        write!(f, "{}", self.sysconfig_paths)?;
        writeln!(f, "\nInstalled Packages:")?;
        write!(f, "{}", self.packages)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PythonError {
    #[error("Python execution failed: {0}")]
    Execution(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Packaging error: {0}")]
    Packaging(#[from] PackagingError),

    #[error("Integer parsing error: {0}")]
    Parse(#[from] std::num::ParseIntError),

    #[error("Failed to locate Python executable: {0}")]
    PythonNotFound(#[from] which::Error),

    #[error(transparent)]
    Runner(#[from] Box<RunnerError>),

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

impl From<RunnerError> for PythonError {
    fn from(err: RunnerError) -> Self {
        PythonError::Runner(Box::new(err))
    }
}
