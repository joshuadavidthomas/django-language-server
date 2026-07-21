use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidTemplateIdentifier {
    #[error("template identifier cannot be empty")]
    Empty,
    #[error("template identifier cannot contain whitespace")]
    ContainsWhitespace,
}

fn validate_template_identifier(value: &str) -> Result<&str, InvalidTemplateIdentifier> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(InvalidTemplateIdentifier::Empty);
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(InvalidTemplateIdentifier::ContainsWhitespace);
    }
    Ok(trimmed)
}

macro_rules! template_identifier {
    ($(#[doc = $doc:literal])* $Name:ident) => {
        $(#[doc = $doc])*
        #[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $Name(Arc<str>);

        impl $Name {
            pub fn parse(name: &str) -> Result<Self, InvalidTemplateIdentifier> {
                let trimmed = validate_template_identifier(name)?;
                Ok(Self(Arc::from(trimmed)))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $Name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $Name {
            type Error = InvalidTemplateIdentifier;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }

        impl From<$Name> for String {
            fn from(value: $Name) -> Self {
                value.0.to_string()
            }
        }

        impl fmt::Display for $Name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.as_str().fmt(f)
            }
        }
    };
}

template_identifier! {
    /// The name used in a Django `{% load %}` tag for a template library.
    LibraryName
}

template_identifier! {
    /// The registered name of a Django template tag or filter.
    TemplateSymbolName
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_empty_or_whitespace_names() {
        assert_eq!(
            LibraryName::parse(""),
            Err(InvalidTemplateIdentifier::Empty)
        );
        assert_eq!(
            LibraryName::parse("   \t"),
            Err(InvalidTemplateIdentifier::Empty)
        );
        assert_eq!(
            TemplateSymbolName::parse("my tag"),
            Err(InvalidTemplateIdentifier::ContainsWhitespace)
        );
    }

    #[test]
    fn serde_deserialization_enforces_invariants() {
        let valid: LibraryName =
            serde_json::from_str("\"humanize\"").expect("test library name should be valid");
        assert_eq!(valid.as_str(), "humanize");

        let invalid: Result<LibraryName, _> = serde_json::from_str("\"\"");
        assert!(invalid.is_err());
    }
}
