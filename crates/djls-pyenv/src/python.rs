use pyo3::prelude::*;
use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Deserialize)]
pub struct VersionInfo {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    pub suffix: Option<String>,
}

impl VersionInfo {
    fn new(major: u8, minor: u8, patch: u8, suffix: Option<String>) -> Self {
        Self {
            major,
            minor,
            patch,
            suffix,
        }
    }

    pub fn from_python(py: Python) -> PyResult<Self> {
        let version_info = py.version_info();

        Ok(Self::new(
            version_info.major,
            version_info.minor,
            version_info.patch,
            version_info.suffix.map(String::from),
        ))
    }

    pub fn from_executable(executable: &PathBuf) -> Result<Self, PythonError> {
        let output = Command::new(executable)
            .args(["-c", "import sys; print(sys.version.split()[0])"])
            .output()?;

        let version_str = String::from_utf8(output.stdout)?.trim().to_string();
        let parts: Vec<&str> = version_str.split('.').collect();

        let major: u8 = parts[0].parse()?;
        let minor: u8 = parts[1].parse()?;

        let last_part = parts[2];
        let (patch_str, suffix) = if last_part.chars().any(|c| !c.is_ascii_digit()) {
            let idx = last_part.find(|c: char| !c.is_ascii_digit()).unwrap();
            (&last_part[..idx], Some(last_part[idx..].to_string()))
        } else {
            (last_part, None)
        };
        let patch: u8 = patch_str.parse()?;

        Ok(Self::new(major, minor, patch, suffix))
    }
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

#[derive(Debug, Deserialize)]
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

impl SysconfigPaths {
    pub fn from_python(py: Python) -> PyResult<Self> {
        let sysconfig = py.import("sysconfig")?;
        let paths = sysconfig.call_method0("get_paths")?;

        Ok(Self {
            data: PathBuf::from(paths.get_item("data").unwrap().extract::<String>()?),
            include: PathBuf::from(paths.get_item("include").unwrap().extract::<String>()?),
            platinclude: PathBuf::from(paths.get_item("platinclude").unwrap().extract::<String>()?),
            platlib: PathBuf::from(paths.get_item("platlib").unwrap().extract::<String>()?),
            platstdlib: PathBuf::from(paths.get_item("platstdlib").unwrap().extract::<String>()?),
            purelib: PathBuf::from(paths.get_item("purelib").unwrap().extract::<String>()?),
            scripts: PathBuf::from(paths.get_item("scripts").unwrap().extract::<String>()?),
            stdlib: PathBuf::from(paths.get_item("stdlib").unwrap().extract::<String>()?),
        })
    }

    pub fn from_executable(executable: &PathBuf) -> Result<Self, PythonError> {
        let output = Command::new(executable)
            .args([
                "-c",
                r#"
import json
import sysconfig
paths = sysconfig.get_paths()
print(json.dumps(paths))
"#,
            ])
            .output()?;

        let output_str = String::from_utf8(output.stdout)?;
        Ok(serde_json::from_str(&output_str)?)
    }
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

#[derive(Debug)]
pub struct Interpreter {
    version_info: VersionInfo,
    sysconfig_paths: SysconfigPaths,
    sys_prefix: PathBuf,
    sys_base_prefix: PathBuf,
    sys_executable: PathBuf,
    sys_path: Vec<PathBuf>,
}

impl Interpreter {
    fn new(
        version_info: VersionInfo,
        sysconfig_paths: SysconfigPaths,
        sys_prefix: PathBuf,
        sys_base_prefix: PathBuf,
        sys_executable: PathBuf,
        sys_path: Vec<PathBuf>,
    ) -> Self {
        Self {
            version_info,
            sysconfig_paths,
            sys_prefix,
            sys_base_prefix,
            sys_executable,
            sys_path,
        }
    }

    pub fn version_info(&self) -> &VersionInfo {
        &self.version_info
    }

    pub fn sysconfig_paths(&self) -> &SysconfigPaths {
        &self.sysconfig_paths
    }

    pub fn sys_prefix(&self) -> &PathBuf {
        &self.sys_prefix
    }

    pub fn sys_base_prefix(&self) -> &PathBuf {
        &self.sys_base_prefix
    }

    pub fn sys_executable(&self) -> &PathBuf {
        &self.sys_executable
    }

    pub fn sys_path(&self) -> &Vec<PathBuf> {
        &self.sys_path
    }

    pub fn for_build(py: Python) -> PyResult<Self> {
        let sys = py.import("sys")?;

        Ok(Self::new(
            VersionInfo::from_python(py)?,
            SysconfigPaths::from_python(py)?,
            PathBuf::from(sys.getattr("prefix")?.extract::<String>()?),
            PathBuf::from(sys.getattr("base_prefix")?.extract::<String>()?),
            PathBuf::from(sys.getattr("executable")?.extract::<String>()?),
            sys.getattr("path")?
                .extract::<Vec<String>>()?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        ))
    }

    pub fn for_runtime(executable: &PathBuf) -> Result<Self, PythonError> {
        let output = Command::new(executable)
            .args([
                "-c",
                r#"
import sys, json
print(json.dumps({
    'prefix': sys.prefix,
    'base_prefix': sys.base_prefix,
    'executable': sys.executable,
    'path': [p for p in sys.path if p],
}))
"#,
            ])
            .output()?;

        let output_str = String::from_utf8(output.stdout)?;
        let sys_info: serde_json::Value = serde_json::from_str(&output_str)?;

        Ok(Self::new(
            VersionInfo::from_executable(executable)?,
            SysconfigPaths::from_executable(executable)?,
            PathBuf::from(sys_info["prefix"].as_str().unwrap()),
            PathBuf::from(sys_info["base_prefix"].as_str().unwrap()),
            PathBuf::from(sys_info["executable"].as_str().unwrap()),
            sys_info["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| PathBuf::from(p.as_str().unwrap()))
                .collect(),
        ))
    }
}

impl fmt::Display for Interpreter {
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
        write!(f, "{}", self.sysconfig_paths)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PythonError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Integer parsing error: {0}")]
    Parse(#[from] std::num::ParseIntError),
}
