use djls_ipc::v1::*;
use djls_ipc::{ProcessError, TransportError};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    dist_name: String,
    dist_version: String,
    dist_location: Option<PathBuf>,
}

impl From<python::Package> for Package {
    fn from(p: python::Package) -> Self {
        Package {
            dist_name: p.dist_name,
            dist_version: p.dist_version,
            dist_location: p.dist_location.map(PathBuf::from),
        }
    }
}

impl fmt::Display for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.dist_name, self.dist_version)?;
        if let Some(location) = &self.dist_location {
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

impl From<HashMap<String, python::Package>> for Packages {
    fn from(packages: HashMap<String, python::Package>) -> Self {
        Packages(packages.into_iter().map(|(k, v)| (k, v.into())).collect())
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
        packages.sort_by(|a, b| a.dist_name.cmp(&b.dist_name));

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

#[derive(Debug, thiserror::Error)]
pub enum PackagingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("Process error: {0}")]
    Process(#[from] ProcessError),

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}
