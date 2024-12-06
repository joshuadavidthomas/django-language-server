use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Package {
    name: String,
    version: String,
    location: Option<PathBuf>,
}

impl Package {
    fn new(name: String, version: String, location: Option<PathBuf>) -> Self {
        Self {
            name,
            version,
            location,
        }
    }

    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn version(&self) -> &String {
        &self.version
    }

    pub fn location(&self) -> &Option<PathBuf> {
        &self.location
    }
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

#[derive(Clone, Debug)]
pub struct Packages(HashMap<String, Package>);

impl Packages {
    fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn from_executable(executable: &Path) -> Result<Self, PackagingError> {
        let output = Command::new(executable)
            .args([
                "-c",
                r#"
import json
import importlib.metadata

packages = {}
for dist in importlib.metadata.distributions():
    try:
        packages[dist.metadata["Name"]] = {
            "name": dist.metadata["Name"],
            "version": dist.version,
            "location": dist.locate_file("").parent.as_posix() if dist.locate_file("") else None
        }
    except Exception:
        continue

print(json.dumps(packages))
"#,
            ])
            .output()?;

        let output_str = String::from_utf8(output.stdout)?;
        let packages_info: serde_json::Value = serde_json::from_str(&output_str)?;

        Ok(packages_info
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, info)| {
                (
                    name.clone(),
                    Package {
                        name: name.clone(),
                        version: info["version"].as_str().unwrap().to_string(),
                        location: info["location"].as_str().map(PathBuf::from),
                    },
                )
            })
            .collect())
    }

    pub fn has_package(&self, name: &str) -> bool {
        self.0.iter().any(|pkg| pkg.1.name == name)
    }
}

impl FromIterator<(String, Package)> for Packages {
    fn from_iter<T: IntoIterator<Item = (String, Package)>>(iter: T) -> Self {
        Self(HashMap::from_iter(iter))
    }
}

impl fmt::Display for Packages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut packages: Vec<_> = self.0.values().collect();
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

#[derive(Debug, thiserror::Error)]
pub enum PackagingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}
