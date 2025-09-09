use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Guard to ensure temporary files are cleaned up
pub struct TempFileGuard {
    path: PathBuf,
}

impl TempFileGuard {
    /// Create a new temporary file with the given content
    pub fn new(content: &[u8], prefix: &str, suffix: &str) -> Result<Self> {
        // Create a unique temp file name
        let temp_dir = std::env::temp_dir();
        let pid = std::process::id();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        let filename = format!("{prefix}_{pid}_{timestamp}{suffix}",);
        let path = temp_dir.join(filename);

        // Write the content to the file
        let mut file = std::fs::File::create(&path).context("Failed to create temp file")?;
        file.write_all(content)
            .context("Failed to write to temp file")?;
        file.sync_all().context("Failed to sync temp file")?;

        Ok(Self { path })
    }

    /// Get the path to the temporary file
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        // Best effort cleanup - ignore errors
        let _ = std::fs::remove_file(&self.path);
    }
}

