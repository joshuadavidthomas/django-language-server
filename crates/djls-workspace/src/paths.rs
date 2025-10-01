//! Path and URL conversion utilities
//!
//! This module provides consistent conversion between file paths and URLs,
//! handling platform-specific differences and encoding issues.

use std::str::FromStr;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use tower_lsp_server::lsp_types;
use url::Url;

/// Convert a `file://` URL to a [`Utf8PathBuf`].
///
/// Handles percent-encoding and platform-specific path formats (e.g., Windows drives).
#[must_use]
pub fn url_to_path(url: &Url) -> Option<Utf8PathBuf> {
    // Only handle file:// URLs
    if url.scheme() != "file" {
        return None;
    }

    // Get the path component and decode percent-encoding
    let path = percent_encoding::percent_decode_str(url.path())
        .decode_utf8()
        .ok()?;

    #[cfg(windows)]
    let path = {
        // Remove leading '/' only for Windows drive paths like /C:/...
        // Check if it matches the pattern /X:/ where X is a drive letter
        if path.len() >= 3 {
            let bytes = path.as_bytes();
            if bytes[0] == b'/' && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
                // It's a drive path like /C:/, strip the leading /
                &path[1..]
            } else {
                // Keep as-is for other paths
                &path
            }
        } else {
            &path
        }
    };

    Some(Utf8PathBuf::from(&*path))
}

/// Context for LSP operations, used for error reporting
#[derive(Debug, Clone, Copy)]
pub enum LspContext {
    Completion,
    Diagnostic,
    DidChange,
    DidClose,
    DidOpen,
    DidSave,
    GotoDefinition,
    References,
}

impl std::fmt::Display for LspContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completion => write!(f, "completion"),
            Self::Diagnostic => write!(f, "diagnostic"),
            Self::DidChange => write!(f, "didChange"),
            Self::DidClose => write!(f, "didClose"),
            Self::DidOpen => write!(f, "didOpen"),
            Self::DidSave => write!(f, "didSave"),
            Self::GotoDefinition => write!(f, "gotoDefinition"),
            Self::References => write!(f, "references"),
        }
    }
}

/// Parse an LSP URI to a [`Url`], logging errors if parsing fails.
///
/// This function is designed for use in LSP notification handlers where
/// invalid URIs should be logged but not crash the server.
pub fn parse_lsp_uri(lsp_uri: &lsp_types::Uri, context: LspContext) -> Option<Url> {
    match Url::parse(lsp_uri.as_str()) {
        Ok(url) => Some(url),
        Err(e) => {
            tracing::error!(
                "Invalid URI from LSP client in {}: {} - Error: {}",
                context,
                lsp_uri.as_str(),
                e
            );
            None
        }
    }
}

/// Convert an LSP [`Uri`](lsp_types::Uri) to a [`Utf8PathBuf`].
///
/// This is a convenience wrapper that parses the LSP URI string and converts it.
#[must_use]
pub fn lsp_uri_to_path(lsp_uri: &lsp_types::Uri) -> Option<Utf8PathBuf> {
    let url = Url::parse(lsp_uri.as_str()).ok()?;
    url_to_path(&url)
}

/// Convert a [`Url`] to an LSP [`Uri`](lsp_types::Uri).
#[must_use]
pub fn url_to_lsp_uri(url: &Url) -> Option<lsp_types::Uri> {
    let uri_string = url.to_string();
    lsp_types::Uri::from_str(&uri_string).ok()
}

/// Convert a [`Path`] to a `file://` URL
///
/// Handles both absolute and relative paths. Relative paths are resolved
/// to absolute paths before conversion. This function does not require
/// the path to exist on the filesystem, making it suitable for overlay
/// files and other virtual content.
#[must_use]
pub fn path_to_url(path: &Utf8Path) -> Option<Url> {
    // For absolute paths, convert directly
    if path.is_absolute() {
        return Url::from_file_path(path).ok();
    }

    // For relative paths, make them absolute without requiring existence
    // First try to get the current directory
    let current_dir = std::env::current_dir().ok()?;
    let absolute_path = current_dir.join(path);

    // Try to canonicalize if the file exists (to resolve symlinks, etc.)
    // but if it doesn't exist, use the joined path as-is
    let final_path = std::fs::canonicalize(&absolute_path).unwrap_or(absolute_path);

    Url::from_file_path(final_path).ok()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_url_to_path_valid_file_url() {
        #[cfg(not(windows))]
        {
            let url = Url::parse("file:///home/user/test.py").unwrap();
            assert_eq!(
                url_to_path(&url),
                Some(Utf8PathBuf::from("/home/user/test.py"))
            );
        }
        #[cfg(windows)]
        {
            let url = Url::parse("file:///C:/Users/test.py").unwrap();
            assert_eq!(
                url_to_path(&url),
                Some(Utf8PathBuf::from("C:/Users/test.py"))
            );
        }
    }

    #[test]
    fn test_url_to_path_non_file_scheme() {
        let url = Url::parse("http://example.com/test.py").unwrap();
        assert_eq!(url_to_path(&url), None);
    }

    #[test]
    fn test_url_to_path_percent_encoded() {
        #[cfg(not(windows))]
        {
            let url = Url::parse("file:///home/user/test%20file.py").unwrap();
            assert_eq!(
                url_to_path(&url),
                Some(Utf8PathBuf::from("/home/user/test file.py"))
            );
        }
        #[cfg(windows)]
        {
            let url = Url::parse("file:///C:/Users/test%20file.py").unwrap();
            assert_eq!(
                url_to_path(&url),
                Some(Utf8PathBuf::from("C:/Users/test file.py"))
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn test_url_to_path_windows_drive() {
        let url = Url::parse("file:///C:/Users/test.py").unwrap();
        assert_eq!(
            url_to_path(&url),
            Some(Utf8PathBuf::from("C:/Users/test.py"))
        );
    }

    #[test]
    fn test_parse_lsp_uri_valid() {
        let uri = lsp_types::Uri::from_str("file:///test.py").unwrap();
        let result = parse_lsp_uri(&uri, LspContext::DidOpen);
        assert!(result.is_some());
        assert_eq!(result.unwrap().scheme(), "file");
    }

    // lsp_uri_to_path tests
    #[test]
    fn test_lsp_uri_to_path_valid_file() {
        #[cfg(not(windows))]
        {
            let uri = lsp_types::Uri::from_str("file:///home/user/test.py").unwrap();
            assert_eq!(
                lsp_uri_to_path(&uri),
                Some(Utf8PathBuf::from("/home/user/test.py"))
            );
        }
        #[cfg(windows)]
        {
            let uri = lsp_types::Uri::from_str("file:///C:/Users/test.py").unwrap();
            assert_eq!(
                lsp_uri_to_path(&uri),
                Some(Utf8PathBuf::from("C:/Users/test.py"))
            );
        }
    }

    #[test]
    fn test_lsp_uri_to_path_non_file() {
        let uri = lsp_types::Uri::from_str("http://example.com/test.py").unwrap();
        assert_eq!(lsp_uri_to_path(&uri), None);
    }

    #[test]
    fn test_lsp_uri_to_path_invalid_uri() {
        let uri = lsp_types::Uri::from_str("not://valid").unwrap();
        assert_eq!(lsp_uri_to_path(&uri), None);
    }

    // path_to_url tests
    #[test]
    fn test_path_to_url_absolute() {
        let path = Utf8Path::new("/home/user/test.py");
        let url = path_to_url(path);
        assert!(url.is_some());
        assert_eq!(url.clone().unwrap().scheme(), "file");
        assert!(url.unwrap().path().contains("test.py"));
    }

    #[test]
    fn test_path_to_url_relative() {
        let path = Utf8Path::new("test.py");
        let url = path_to_url(path);
        assert!(url.is_some());
        assert_eq!(url.clone().unwrap().scheme(), "file");
        // Should be resolved to absolute path
        assert!(url.unwrap().path().ends_with("/test.py"));
    }

    #[test]
    fn test_path_to_url_nonexistent_absolute() {
        let path = Utf8Path::new("/definitely/does/not/exist/test.py");
        let url = path_to_url(path);
        assert!(url.is_some());
        assert_eq!(url.unwrap().scheme(), "file");
    }
}
