mod clean;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use clean::clean_utf8_path;
pub use clean::Utf8PathClean;

/// Django's `safe_join` equivalent - join paths and ensure result is within base
pub fn safe_join(base: &Utf8Path, name: &str) -> Result<Utf8PathBuf, SafeJoinError> {
    let candidate = base.join(name);
    let cleaned = clean_utf8_path(&candidate);

    if cleaned.starts_with(base) {
        Ok(cleaned)
    } else {
        Err(SafeJoinError::OutsideBase {
            base: base.to_path_buf(),
            attempted: name.to_string(),
            resolved: cleaned,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SafeJoinError {
    #[error("Path '{attempted}' would resolve to '{resolved}' which is outside base '{base}'")]
    OutsideBase {
        base: Utf8PathBuf,
        attempted: String,
        resolved: Utf8PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_join_allows_normal_path() {
        let base = Utf8Path::new("/templates");
        assert_eq!(
            safe_join(base, "myapp/base.html").unwrap(),
            Utf8PathBuf::from("/templates/myapp/base.html")
        );
    }

    #[test]
    fn test_safe_join_blocks_parent_escape() {
        let base = Utf8Path::new("/templates");
        assert!(safe_join(base, "../../etc/passwd").is_err());
    }
}
