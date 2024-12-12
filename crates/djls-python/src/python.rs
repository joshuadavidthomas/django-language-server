use crate::packaging::{Packages, PackagingError};
use djls_ipc::v1::*;
use djls_ipc::{ProcessError, PythonProcess, TransportError};
use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
pub struct VersionInfo {
    major: u8,
    minor: u8,
    micro: u8,
    releaselevel: ReleaseLevel,
    serial: Option<String>,
}

impl From<python::VersionInfo> for VersionInfo {
    fn from(v: python::VersionInfo) -> Self {
        Self {
            major: v.major as u8,
            minor: v.minor as u8,
            micro: v.micro as u8,
            releaselevel: v.releaselevel().into(),
            serial: Some(v.serial.to_string()),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub enum ReleaseLevel {
    Alpha,
    Beta,
    Candidate,
    Final,
}

impl From<python::ReleaseLevel> for ReleaseLevel {
    fn from(level: python::ReleaseLevel) -> Self {
        match level {
            python::ReleaseLevel::Alpha => ReleaseLevel::Alpha,
            python::ReleaseLevel::Beta => ReleaseLevel::Beta,
            python::ReleaseLevel::Candidate => ReleaseLevel::Candidate,
            python::ReleaseLevel::Final => ReleaseLevel::Final,
        }
    }
}

impl fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.micro)?;
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

impl Python {
    pub fn setup(python: &mut PythonProcess) -> Result<Self, PythonError> {
        let request = messages::Request {
            command: Some(messages::request::Command::PythonGetEnvironment(
                python::GetEnvironmentRequest {},
            )),
        };

        let response = python.send(request).map_err(PythonError::Transport)?;

        match response.result {
            Some(messages::response::Result::PythonGetEnvironment(response)) => response
                .python
                .ok_or_else(|| PythonError::Process(ProcessError::Response))
                .map(Into::into),
            Some(messages::response::Result::Error(e)) => {
                Err(PythonError::Process(ProcessError::Health(e.message)))
            }
            _ => Err(PythonError::Process(ProcessError::Response)),
        }
    }
}

impl From<python::Python> for Python {
    fn from(p: python::Python) -> Self {
        let sys = p.sys.unwrap();
        let sysconfig = p.sysconfig.unwrap();
        let site = p.site.unwrap();

        Self {
            version_info: sys.version_info.unwrap_or_default().into(),
            sysconfig_paths: SysconfigPaths {
                data: PathBuf::from(sysconfig.data),
                include: PathBuf::from(sysconfig.include),
                platinclude: PathBuf::from(sysconfig.platinclude),
                platlib: PathBuf::from(sysconfig.platlib),
                platstdlib: PathBuf::from(sysconfig.platstdlib),
                purelib: PathBuf::from(sysconfig.purelib),
                scripts: PathBuf::from(sysconfig.scripts),
                stdlib: PathBuf::from(sysconfig.stdlib),
            },
            sys_prefix: PathBuf::from(sys.prefix),
            sys_base_prefix: PathBuf::from(sys.base_prefix),
            sys_executable: PathBuf::from(sys.executable),
            sys_path: sys.path.into_iter().map(PathBuf::from).collect(),
            packages: site.packages.into(),
        }
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
    #[error("Process error: {0}")]
    Process(#[from] ProcessError),
    #[error("Failed to locate Python executable: {0}")]
    PythonNotFound(#[from] which::Error),
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}
