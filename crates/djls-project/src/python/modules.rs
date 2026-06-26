use std::borrow::Borrow;
use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidModulePath {
    #[error("python module path cannot be empty")]
    Empty,
    #[error("python module path cannot contain whitespace")]
    ContainsWhitespace,
    #[error("python module path cannot start with '.'")]
    StartsWithDot,
    #[error("python module path cannot end with '.'")]
    EndsWithDot,
    #[error("python module path cannot contain consecutive dots")]
    ContainsConsecutiveDots,
    #[error("python module path contains invalid segment: {0}")]
    InvalidSegment(String),
    #[error("python module file path must end with '.py'")]
    MustHavePyExtension,
    #[error("python module path must be relative, got absolute: {0}")]
    IsAbsolute(String),
}

/// A dotted Python module path, e.g. `"myapp.models"`.
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PythonModulePath(String);

impl PythonModulePath {
    pub fn parse(path: &str) -> Result<Self, InvalidModulePath> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(InvalidModulePath::Empty);
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(InvalidModulePath::ContainsWhitespace);
        }
        validate_python_module_path(trimmed)?;
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn from_relative_package(path: &Utf8Path) -> Result<Self, InvalidModulePath> {
        if path.is_absolute() {
            return Err(InvalidModulePath::IsAbsolute(path.to_string()));
        }

        let dotted = path
            .components()
            .map(|component| component.as_str())
            .collect::<Vec<_>>()
            .join(".");

        Self::parse(&dotted)
    }

    pub fn from_relative_python_module(path: &Utf8Path) -> Result<Self, InvalidModulePath> {
        if path.is_absolute() {
            return Err(InvalidModulePath::IsAbsolute(path.to_string()));
        }
        if path.extension() != Some("py") {
            return Err(InvalidModulePath::MustHavePyExtension);
        }

        Self::parse(&dotted_module_path_from_relative_path(path))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

fn dotted_module_path_from_relative_path(path: &Utf8Path) -> String {
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

fn validate_python_module_path(path: &str) -> Result<(), InvalidModulePath> {
    if path.starts_with('.') {
        return Err(InvalidModulePath::StartsWithDot);
    }

    if path.ends_with('.') {
        return Err(InvalidModulePath::EndsWithDot);
    }

    if path.contains("..") {
        return Err(InvalidModulePath::ContainsConsecutiveDots);
    }

    for segment in path.split('.') {
        if !is_python_identifier(segment) {
            return Err(InvalidModulePath::InvalidSegment(segment.to_string()));
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

impl Borrow<str> for PythonModulePath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PythonModulePath {
    type Error = InvalidModulePath;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PythonModulePath> for String {
    fn from(value: PythonModulePath) -> Self {
        value.0
    }
}

impl fmt::Display for PythonModulePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PythonModule {
    module_path: PythonModulePath,
    path: Utf8PathBuf,
    file: File,
}

impl PythonModule {
    pub(crate) fn new(module_path: PythonModulePath, path: Utf8PathBuf, file: File) -> Self {
        Self {
            module_path,
            path,
            file,
        }
    }

    #[must_use]
    pub fn module_path(&self) -> &PythonModulePath {
        &self.module_path
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for PythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonModule")
            .field("module_path", &self.module_path)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_module_paths_validate_and_normalize() {
        assert_eq!(
            PythonModulePath::from_relative_python_module(Utf8Path::new("pkg/__init__.py")),
            Ok(PythonModulePath::parse("pkg").unwrap())
        );
        assert_eq!(
            PythonModulePath::parse("django..template"),
            Err(InvalidModulePath::ContainsConsecutiveDots)
        );
        assert_eq!(
            PythonModulePath::parse(".django"),
            Err(InvalidModulePath::StartsWithDot)
        );
        assert_eq!(
            PythonModulePath::parse("django."),
            Err(InvalidModulePath::EndsWithDot)
        );
        assert_eq!(
            PythonModulePath::parse("pkg/models"),
            Err(InvalidModulePath::InvalidSegment("pkg/models".to_string()))
        );
        assert_eq!(
            PythonModulePath::parse("my-app.models"),
            Err(InvalidModulePath::InvalidSegment("my-app".to_string()))
        );
        assert_eq!(
            PythonModulePath::parse("123pkg.models"),
            Err(InvalidModulePath::InvalidSegment("123pkg".to_string()))
        );
        assert_eq!(
            PythonModulePath::from_relative_package(Utf8Path::new("pkg/bad-name")),
            Err(InvalidModulePath::InvalidSegment("bad-name".to_string()))
        );
        assert_eq!(
            PythonModulePath::from_relative_python_module(Utf8Path::new("pkg/bad-name.py")),
            Err(InvalidModulePath::InvalidSegment("bad-name".to_string()))
        );
        assert_eq!(
            PythonModulePath::from_relative_python_module(Utf8Path::new("pkg/module.txt")),
            Err(InvalidModulePath::MustHavePyExtension)
        );
    }
}
