//! Tarball extraction and checksum verification.

use std::io::Read;

use camino::Utf8Path;
use sha2::Digest;
use sha2::Sha256;

use crate::filter::is_download_relevant;

/// Compute the hex-encoded SHA256 digest of `data`.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Verify that `data` matches the expected SHA256 hex digest.
pub fn verify_sha256(data: &[u8], expected: &str, label: &str) -> anyhow::Result<()> {
    let actual = sha256_hex(data);
    if actual != expected {
        anyhow::bail!("SHA256 mismatch for {label}\n  expected: {expected}\n  actual:   {actual}");
    }
    Ok(())
}

/// Extract relevant files from in-memory tarball bytes.
///
/// Strips the top-level directory from each entry, filters through
/// [`is_download_relevant`], and rejects paths with `..` components
/// to prevent directory traversal.
pub fn extract_tarball(data: &[u8], out_dir: &Utf8Path) -> anyhow::Result<()> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);

    std::fs::create_dir_all(out_dir.as_std_path())?;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_string_lossy().to_string();

        // Strip the top-level directory (e.g., "Django-5.2.11/")
        let relative = entry_path
            .split_once('/')
            .map_or(entry_path.as_str(), |x| x.1);

        if !is_download_relevant(relative) {
            continue;
        }

        if std::path::Path::new(relative)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            anyhow::bail!("Path traversal detected in tarball entry: {entry_path}");
        }

        let dest = out_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent.as_std_path())?;
        }

        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        std::fs::write(dest.as_std_path(), &content)?;
    }

    Ok(())
}
