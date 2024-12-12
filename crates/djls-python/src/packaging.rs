use djls_ipc::{JsonResponse, PythonProcess, TransportError, TransportMessage, TransportResponse};
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

impl TryFrom<JsonResponse> for ImportCheck {
    type Error = TransportError;

    fn try_from(response: JsonResponse) -> Result<Self, Self::Error> {
        response
            .data()
            .clone()
            .ok_or_else(|| TransportError::Process("No data in response".to_string()))
            .and_then(|data| serde_json::from_value(data).map_err(TransportError::Json))
    }
}

impl ImportCheck {
    pub fn can_import(&self) -> bool {
        self.can_import
    }

    pub fn check(
        python: &mut PythonProcess,
        modules: Option<Vec<String>>,
    ) -> Result<bool, PackagingError> {
        let message = TransportMessage::Json("has_import".to_string());
        let response = python.send(message, modules)?;
        match response {
            TransportResponse::Json(json_str) => {
                let json_response: JsonResponse = serde_json::from_str(&json_str)?;
                let check = Self::try_from(json_response)?;
                Ok(check.can_import)
            }
            _ => Err(PackagingError::Transport(TransportError::Process(
                "Unexpected response type".to_string(),
            ))),
        }
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

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}
