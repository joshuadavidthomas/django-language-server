// Interned types for string deduplication in djls-semantic
// This provides memory efficiency by storing each unique string only once

use camino::Utf8PathBuf;

/// Interned tag name for deduplication (e.g., "block", "for", "extends")
#[salsa::interned(debug)]
pub struct TagName {
    pub text: String,
}

/// Interned variable path for deduplication (e.g., ["user", "profile", "name"])
#[salsa::interned(debug)]
pub struct VariablePath {
    pub segments: Vec<String>,
}

/// Interned template path for deduplication (e.g., "base.html", "includes/header.html")
#[salsa::interned(debug)]
pub struct TemplatePath {
    pub path: Utf8PathBuf,
}

/// Interned argument list for tag arguments
#[salsa::interned(debug)]
pub struct ArgumentList {
    pub args: Vec<String>,
}

/// Filter call with name and arguments
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FilterCall {
    pub name: String,
    pub args: Vec<String>,
}

/// Interned filter chain for variables
#[salsa::interned(debug)]
pub struct FilterChain {
    pub filters: Vec<FilterCall>,
}
