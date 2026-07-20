use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use camino::Utf8Path;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::python::evaluation::StructuralOrd;

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

impl InvalidModuleName {
    fn structural_rank(&self) -> u8 {
        match self {
            Self::ContainsConsecutiveDots => 0,
            Self::ContainsWhitespace => 1,
            Self::Empty => 2,
            Self::EndsWithDot => 3,
            Self::InvalidSegment(_) => 4,
            Self::MustHavePyExtension => 5,
            Self::SourcePathIsAbsolute(_) => 6,
            Self::StartsWithDot => 7,
        }
    }
}

impl StructuralOrd for InvalidModuleName {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        let ordering = self.structural_rank().cmp(&other.structural_rank());
        if ordering != Ordering::Equal {
            return ordering;
        }
        match (self, other) {
            (Self::Empty, Self::Empty)
            | (Self::ContainsWhitespace, Self::ContainsWhitespace)
            | (Self::StartsWithDot, Self::StartsWithDot)
            | (Self::EndsWithDot, Self::EndsWithDot)
            | (Self::ContainsConsecutiveDots, Self::ContainsConsecutiveDots)
            | (Self::MustHavePyExtension, Self::MustHavePyExtension) => Ordering::Equal,
            (Self::InvalidSegment(left), Self::InvalidSegment(right))
            | (Self::SourcePathIsAbsolute(left), Self::SourcePathIsAbsolute(right)) => {
                left.cmp(right)
            }
            (
                Self::Empty
                | Self::ContainsWhitespace
                | Self::StartsWithDot
                | Self::EndsWithDot
                | Self::ContainsConsecutiveDots
                | Self::InvalidSegment(_)
                | Self::MustHavePyExtension
                | Self::SourcePathIsAbsolute(_),
                _,
            ) => unreachable!("equal module-name-error ranks identify the same variant"),
        }
    }
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
        if trimmed.starts_with('.') {
            return Err(InvalidModuleName::StartsWithDot);
        }
        if trimmed.ends_with('.') {
            return Err(InvalidModuleName::EndsWithDot);
        }
        if trimmed.contains("..") {
            return Err(InvalidModuleName::ContainsConsecutiveDots);
        }
        for segment in trimmed.split('.') {
            if !is_python_identifier(segment) {
                return Err(InvalidModuleName::InvalidSegment(segment.to_string()));
            }
        }

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

        let without_ext = path.with_extension("");
        let parts: Vec<&str> = without_ext
            .components()
            .map(|component| component.as_str())
            .collect();
        let module_name = if parts.last() == Some(&"__init__") {
            parts[..parts.len() - 1].join(".")
        } else {
            parts.join(".")
        };

        Self::parse(&module_name)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Extend this module by exactly one identifier segment.
    pub(crate) fn exact_child(&self, segment: &str) -> Option<Self> {
        is_python_identifier(segment)
            .then(|| Self(Arc::from(format!("{}.{}", self.as_str(), segment))))
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

    #[test]
    fn exact_child_accepts_one_identifier_segment_only() {
        let package = PythonModuleName::parse("pkg").unwrap();

        assert_eq!(package.exact_child("child").unwrap().as_str(), "pkg.child");
        assert!(package.exact_child("child.grandchild").is_none());
        assert!(package.exact_child("bad-name").is_none());
        assert!(package.exact_child(" child ").is_none());
    }
}
