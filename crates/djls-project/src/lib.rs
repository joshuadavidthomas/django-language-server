mod templatetags;

pub use templatetags::TemplateTags;

use pyo3::prelude::*;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::*;

#[derive(Debug)]
pub struct DjangoProject {
    path: PathBuf,
    template_tags: Option<TemplateTags>,
}

impl DjangoProject {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
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
        Python::with_gil(|py| {
            // Add project to Python path
            let sys = py.import("sys")?;
            let py_path = sys.getattr("path")?;
            py_path.call_method1("append", (self.path.to_str().unwrap(),))?;

            // Setup Django
            let django = py.import("django")?;
            django.call_method0("setup")?;

            self.template_tags = Some(TemplateTags::from_python(py)?);

            Ok(())
        })
    }

    pub fn template_tags(&self) -> Option<&TemplateTags> {
        self.template_tags.as_ref()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
