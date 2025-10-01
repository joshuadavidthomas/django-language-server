//! Path and URL conversion utilities
//!
//! This module provides consistent conversion between file paths and URLs,
//! handling platform-specific differences and encoding issues.

use camino::Utf8Path;
use camino::Utf8PathBuf;
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
