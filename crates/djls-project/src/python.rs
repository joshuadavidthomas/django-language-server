use crate::db::Db;
use crate::system;
use pyo3::prelude::*;
use std::fmt;
use std::path::{Path, PathBuf};

#[salsa::tracked]
pub fn find_python_environment(db: &dyn Db) -> Option<PythonEnvironment> {
    let project_path = db.metadata().root();
    let venv_path = db.metadata().venv();
    PythonEnvironment::new(
        project_path.as_path(),
        venv_path.map(|p| p.to_str()).flatten(),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct PythonEnvironment {
    python_path: PathBuf,
    sys_path: Vec<PathBuf>,
    sys_prefix: PathBuf,
}

impl PythonEnvironment {
    fn new(project_path: &Path, venv_path: Option<&str>) -> Option<Self> {
        if let Some(path) = venv_path {
            let prefix = PathBuf::from(path);
            if let Some(env) = Self::from_venv_prefix(&prefix) {
                return Some(env);
            }
            // Invalid explicit path, continue searching...
        }

        if let Ok(virtual_env) = system::env_var("VIRTUAL_ENV") {
            let prefix = PathBuf::from(virtual_env);
            if let Some(env) = Self::from_venv_prefix(&prefix) {
                return Some(env);
            }
        }

        for venv_dir in &[".venv", "venv", "env", ".env"] {
            let potential_venv = project_path.join(venv_dir);
            if potential_venv.is_dir() {
                if let Some(env) = Self::from_venv_prefix(&potential_venv) {
                    return Some(env);
                }
            }
        }

        Self::from_system_python()
    }

    fn from_venv_prefix(prefix: &Path) -> Option<Self> {
        #[cfg(unix)]
        let python_path = prefix.join("bin").join("python");
        #[cfg(windows)]
        let python_path = prefix.join("Scripts").join("python.exe");

        if !prefix.is_dir() || !python_path.exists() {
            return None;
        }

        #[cfg(unix)]
        let bin_dir = prefix.join("bin");
        #[cfg(windows)]
        let bin_dir = prefix.join("Scripts");

        let mut sys_path = Vec::new();
        sys_path.push(bin_dir);

        if let Some(site_packages) = Self::find_site_packages(prefix) {
            if site_packages.is_dir() {
                sys_path.push(site_packages);
            }
        }

        Some(Self {
            python_path: python_path.clone(),
            sys_path,
            sys_prefix: prefix.to_path_buf(),
        })
    }

    pub fn activate(&self, py: Python) -> PyResult<()> {
        let sys = py.import("sys")?;
        let py_path = sys.getattr("path")?;

        for path in &self.sys_path {
            if let Some(path_str) = path.to_str() {
                py_path.call_method1("append", (path_str,))?;
            }
        }

        Ok(())
    }

    fn from_system_python() -> Option<Self> {
        let python_path = match system::find_executable("python") {
            Ok(p) => p,
            Err(_) => return None,
        };
        let bin_dir = python_path.parent()?;
        let prefix = bin_dir.parent()?;

        let mut sys_path = Vec::new();
        sys_path.push(bin_dir.to_path_buf());

        if let Some(site_packages) = Self::find_site_packages(prefix) {
            if site_packages.is_dir() {
                sys_path.push(site_packages);
            }
        }

        Some(Self {
            python_path: python_path.clone(),
            sys_path,
            sys_prefix: prefix.to_path_buf(),
        })
    }

    #[cfg(unix)]
    fn find_site_packages(prefix: &Path) -> Option<PathBuf> {
        let lib_dir = prefix.join("lib");
        if !lib_dir.is_dir() {
            return None;
        }
        std::fs::read_dir(lib_dir)
            .ok()?
            .filter_map(Result::ok)
            .find(|e| {
                e.file_type().is_ok_and(|ft| ft.is_dir())
                    && e.file_name().to_string_lossy().starts_with("python")
            })
            .map(|e| e.path().join("site-packages"))
    }

    #[cfg(windows)]
    fn find_site_packages(prefix: &Path) -> Option<PathBuf> {
        Some(prefix.join("Lib").join("site-packages"))
    }
}

impl fmt::Display for PythonEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Python path: {}", self.python_path.display())?;
        writeln!(f, "Sys prefix: {}", self.sys_prefix.display())?;
        writeln!(f, "Sys paths:")?;
        for path in &self.sys_path {
            writeln!(f, "  {}", path.display())?;
        }
        Ok(())
    }
}

