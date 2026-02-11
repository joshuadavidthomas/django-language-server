use camino::Utf8Path;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidName {
    #[error("name cannot be empty")]
    Empty,
    #[error("name cannot contain whitespace")]
    ContainsWhitespace,
    #[error("python module path cannot start with '.'")]
    ModuleStartsWithDot,
    #[error("python module path cannot end with '.'")]
    ModuleEndsWithDot,
    #[error("python module path cannot contain consecutive dots")]
    ModuleContainsConsecutiveDots,
    #[error("python module file path must end with '.py'")]
    ModuleMustHavePyExtension,
}

fn validate_non_empty_no_whitespace(value: &str) -> Result<&str, InvalidName> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(InvalidName::Empty);
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(InvalidName::ContainsWhitespace);
    }
    Ok(trimmed)
}

fn validate_python_module_path(path: &str) -> Result<(), InvalidName> {
    if path.starts_with('.') {
        return Err(InvalidName::ModuleStartsWithDot);
    }

    if path.ends_with('.') {
        return Err(InvalidName::ModuleEndsWithDot);
    }

    if path.contains("..") {
        return Err(InvalidName::ModuleContainsConsecutiveDots);
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LibraryName(String);

impl LibraryName {
    pub fn parse(name: &str) -> Result<Self, InvalidName> {
        let trimmed = validate_non_empty_no_whitespace(name)?;
        Ok(Self(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LibraryName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<LibraryName> for String {
    fn from(value: LibraryName) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TemplateSymbolName(String);

impl TemplateSymbolName {
    pub fn parse(name: &str) -> Result<Self, InvalidName> {
        let trimmed = validate_non_empty_no_whitespace(name)?;
        Ok(Self(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TemplateSymbolName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TemplateSymbolName> for String {
    fn from(value: TemplateSymbolName) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PyModuleName(String);

impl PyModuleName {
    pub fn parse(name: &str) -> Result<Self, InvalidName> {
        let trimmed = validate_non_empty_no_whitespace(name)?;
        validate_python_module_path(trimmed)?;
        Ok(Self(trimmed.to_string()))
    }

    pub fn from_relative_package(path: &Utf8Path) -> Result<Self, InvalidName> {
        let dotted = path
            .components()
            .map(|component| component.as_str())
            .collect::<Vec<_>>()
            .join(".");

        Self::parse(&dotted)
    }

    pub fn from_relative_python_module(path: &Utf8Path) -> Result<Self, InvalidName> {
        if path.extension() != Some("py") {
            return Err(InvalidName::ModuleMustHavePyExtension);
        }

        let module_path = path.with_extension("");
        Self::from_relative_package(module_path.as_path())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PyModuleName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PyModuleName> for String {
    fn from(value: PyModuleName) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_empty_or_whitespace_names() {
        assert_eq!(LibraryName::parse(""), Err(InvalidName::Empty));
        assert_eq!(LibraryName::parse("   \t"), Err(InvalidName::Empty));
        assert_eq!(
            TemplateSymbolName::parse("my tag"),
            Err(InvalidName::ContainsWhitespace)
        );
    }

    #[test]
    fn module_name_parse_rejects_invalid_paths() {
        assert_eq!(
            PyModuleName::parse("django..template"),
            Err(InvalidName::ModuleContainsConsecutiveDots)
        );
        assert_eq!(
            PyModuleName::parse(".django"),
            Err(InvalidName::ModuleStartsWithDot)
        );
        assert_eq!(
            PyModuleName::parse("django."),
            Err(InvalidName::ModuleEndsWithDot)
        );
        assert_eq!(
            PyModuleName::from_relative_python_module(Utf8Path::new("pkg/module.txt")),
            Err(InvalidName::ModuleMustHavePyExtension)
        );
    }

    #[test]
    fn serde_deserialization_enforces_invariants() {
        let valid: LibraryName = serde_json::from_str("\"humanize\"").unwrap();
        assert_eq!(valid.as_str(), "humanize");

        let invalid: Result<LibraryName, _> = serde_json::from_str("\"\"");
        assert!(invalid.is_err());
    }
}
