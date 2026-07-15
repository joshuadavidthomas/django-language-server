use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use camino::Utf8Path;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidModuleName {
    #[error("python module name cannot be empty")]
    Empty,
    #[error("python module name cannot contain whitespace")]
    ContainsWhitespace,
    #[error("python module name cannot start with '.'")]
    StartsWithDot,
    #[error("python module name cannot end with '.'")]
    EndsWithDot,
    #[error("python module name cannot contain consecutive dots")]
    ContainsConsecutiveDots,
    #[error("python module name contains invalid segment: {0}")]
    InvalidSegment(String),
    #[error("python module source path must end with '.py'")]
    MustHavePyExtension,
    #[error("python module source path must be relative, got absolute: {0}")]
    SourcePathIsAbsolute(String),
}

/// A dotted Python module name, e.g. `"myapp.models"`.
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PythonModuleName(Arc<str>);

impl PythonModuleName {
    pub fn parse(name: &str) -> Result<Self, InvalidModuleName> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(InvalidModuleName::Empty);
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(InvalidModuleName::ContainsWhitespace);
        }
        validate_python_module_name(trimmed)?;
        Ok(Self(Arc::from(trimmed)))
    }

    pub(crate) fn from_relative_package_path(path: &Utf8Path) -> Result<Self, InvalidModuleName> {
        if path.is_absolute() {
            return Err(InvalidModuleName::SourcePathIsAbsolute(path.to_string()));
        }

        let dotted = path
            .components()
            .map(|component| component.as_str())
            .collect::<Vec<_>>()
            .join(".");

        Self::parse(&dotted)
    }

    pub fn from_relative_source_path(path: &Utf8Path) -> Result<Self, InvalidModuleName> {
        if path.is_absolute() {
            return Err(InvalidModuleName::SourcePathIsAbsolute(path.to_string()));
        }
        if path.extension() != Some("py") {
            return Err(InvalidModuleName::MustHavePyExtension);
        }

        Self::parse(&module_name_from_relative_source_path(path))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        self.as_str()
            .rsplit_once('.')
            .map(|(parent, _)| Self(Arc::from(parent)))
    }

    #[must_use]
    pub(crate) fn into_string(self) -> String {
        self.0.to_string()
    }
}

fn module_name_from_relative_source_path(path: &Utf8Path) -> String {
    let without_ext = path.with_extension("");
    let parts: Vec<&str> = without_ext
        .components()
        .map(|component| component.as_str())
        .collect();
    if parts.last() == Some(&"__init__") {
        parts[..parts.len() - 1].join(".")
    } else {
        parts.join(".")
    }
}

fn validate_python_module_name(name: &str) -> Result<(), InvalidModuleName> {
    if name.starts_with('.') {
        return Err(InvalidModuleName::StartsWithDot);
    }

    if name.ends_with('.') {
        return Err(InvalidModuleName::EndsWithDot);
    }

    if name.contains("..") {
        return Err(InvalidModuleName::ContainsConsecutiveDots);
    }

    for segment in name.split('.') {
        if !is_python_identifier(segment) {
            return Err(InvalidModuleName::InvalidSegment(segment.to_string()));
        }
    }

    Ok(())
}

fn is_python_identifier(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '_' && !first.is_alphabetic() {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_alphanumeric())
}

impl Borrow<str> for PythonModuleName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PythonModuleName {
    type Error = InvalidModuleName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PythonModuleName> for String {
    fn from(value: PythonModuleName) -> Self {
        value.0.to_string()
    }
}

impl fmt::Display for PythonModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::*;

    #[test]
    fn python_module_names_validate_and_normalize() {
        assert_eq!(
            PythonModuleName::from_relative_source_path(Utf8Path::new("pkg/__init__.py")),
            Ok(PythonModuleName::parse("pkg").unwrap())
        );
        assert_eq!(
            PythonModuleName::parse("django..template"),
            Err(InvalidModuleName::ContainsConsecutiveDots)
        );
        assert_eq!(
            PythonModuleName::parse(".django"),
            Err(InvalidModuleName::StartsWithDot)
        );
        assert_eq!(
            PythonModuleName::parse("django."),
            Err(InvalidModuleName::EndsWithDot)
        );
        assert_eq!(
            PythonModuleName::parse("pkg/models"),
            Err(InvalidModuleName::InvalidSegment("pkg/models".to_string()))
        );
        assert_eq!(
            PythonModuleName::parse("my-app.models"),
            Err(InvalidModuleName::InvalidSegment("my-app".to_string()))
        );
        assert_eq!(
            PythonModuleName::parse("123pkg.models"),
            Err(InvalidModuleName::InvalidSegment("123pkg".to_string()))
        );
        assert_eq!(
            PythonModuleName::from_relative_package_path(Utf8Path::new("pkg/bad-name")),
            Err(InvalidModuleName::InvalidSegment("bad-name".to_string()))
        );
        assert_eq!(
            PythonModuleName::from_relative_source_path(Utf8Path::new("pkg/bad-name.py")),
            Err(InvalidModuleName::InvalidSegment("bad-name".to_string()))
        );
        assert_eq!(
            PythonModuleName::from_relative_source_path(Utf8Path::new("pkg/module.txt")),
            Err(InvalidModuleName::MustHavePyExtension)
        );
    }
}
