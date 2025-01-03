mod templatetags;

pub use templatetags::TemplateTags;

use pyo3::prelude::*;
use std::fmt;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::*;
use which::which;

#[derive(Debug)]
pub struct DjangoProject {
    path: PathBuf,
    env: Option<PythonEnvironment>,
    template_tags: Option<TemplateTags>,
}

impl DjangoProject {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            env: None,
            template_tags: None,
        }
    }

    pub fn from_initialize_params(params: &InitializeParams) -> Option<Self> {
        // Try current directory first
        let path = std::env::current_dir()
            .ok()
            // Fall back to workspace root if provided
            .or_else(|| {
                params
                    .root_uri
                    .as_ref()
                    .and_then(|uri| uri.to_file_path().ok())
            });

        path.map(Self::new)
    }

    pub fn initialize(&mut self) -> PyResult<()> {
        let python_env = PythonEnvironment::new().ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>("Could not find Python in PATH")
        })?;

        Python::with_gil(|py| {
            let sys = py.import("sys")?;
            let py_path = sys.getattr("path")?;

            if let Some(path_str) = self.path.to_str() {
                py_path.call_method1("insert", (0, path_str))?;
            }

            for path in &python_env.sys_path {
                if let Some(path_str) = path.to_str() {
                    py_path.call_method1("append", (path_str,))?;
                }
            }

            self.env = Some(python_env);

            match py.import("django") {
                Ok(django) => {
                    django.call_method0("setup")?;
                    self.template_tags = Some(TemplateTags::from_python(py)?);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Failed to import Django: {}", e);
                    Err(e)
                }
            }
        })
    }

    pub fn template_tags(&self) -> Option<&TemplateTags> {
        self.template_tags.as_ref()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for DjangoProject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Project path: {}", self.path.display())?;
        if let Some(py_env) = &self.env {
            write!(f, "{}", py_env)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PythonEnvironment {
    python_path: PathBuf,
    sys_path: Vec<PathBuf>,
    sys_prefix: PathBuf,
}

impl PythonEnvironment {
    fn new() -> Option<Self> {
        let python_path = which("python").ok()?;
        let prefix = python_path.parent()?.parent()?;

        let mut sys_path = Vec::new();
        sys_path.push(prefix.join("bin"));

        if let Some(site_packages) = Self::find_site_packages(prefix) {
            sys_path.push(site_packages);
        }

        Some(Self {
            python_path: python_path.clone(),
            sys_path,
            sys_prefix: prefix.to_path_buf(),
        })
    }

    #[cfg(windows)]
    fn find_site_packages(prefix: &Path) -> Option<PathBuf> {
        Some(prefix.join("Lib").join("site-packages"))
    }

    #[cfg(not(windows))]
    fn find_site_packages(prefix: &Path) -> Option<PathBuf> {
        std::fs::read_dir(prefix.join("lib"))
            .ok()?
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().starts_with("python"))
            .map(|e| e.path().join("site-packages"))
    }
}

impl fmt::Display for PythonEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Sys prefix: {}", self.sys_prefix.display())?;
        writeln!(f, "Sys paths:")?;
        for path in &self.sys_path {
            writeln!(f, "  {}", path.display())?;
        }
        Ok(())
    }
}
