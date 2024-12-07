use crate::include_script;
use crate::python::Python;
use crate::runner::{RunnerError, ScriptRunner};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    name: String,
    version: String,
    location: Option<PathBuf>,
}

impl fmt::Display for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.version)?;
        if let Some(location) = &self.location {
            write!(f, " ({})", location.display())?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Packages(HashMap<String, Package>);

impl Packages {
    pub fn packages(&self) -> Vec<&Package> {
        self.0.values().collect()
    }
}

impl FromIterator<(String, Package)> for Packages {
    fn from_iter<T: IntoIterator<Item = (String, Package)>>(iter: T) -> Self {
        Self(HashMap::from_iter(iter))
    }
}

impl fmt::Display for Packages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut packages: Vec<_> = self.packages();
        packages.sort_by(|a, b| a.name.cmp(&b.name));

        if packages.is_empty() {
            writeln!(f, "  (no packages installed)")?;
        } else {
            for package in packages {
                writeln!(f, "{}", package)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct ImportCheck {
    can_import: bool,
}

impl ScriptRunner for ImportCheck {
    const SCRIPT: &'static str = include_script!("has_import");
}

impl ImportCheck {
    pub fn can_import(&self) -> bool {
        self.can_import
    }

    pub fn check(py: &Python, module: &str) -> Result<bool, RunnerError> {
        let result = ImportCheck::run_with_py_args(py, module)?;
        Ok(result.can_import)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PackagingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Runner(#[from] Box<RunnerError>),

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

impl From<RunnerError> for PackagingError {
    fn from(err: RunnerError) -> Self {
        PackagingError::Runner(Box::new(err))
    }
}
