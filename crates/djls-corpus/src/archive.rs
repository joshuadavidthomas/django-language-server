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

/// Build a gzipped tarball in memory from a closure that populates a [`tar::Builder`].
///
/// Used by tests to construct tarballs with specific entry types.
#[cfg(test)]
fn build_test_tarball(populate: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    populate(&mut builder);
    let tar_bytes = builder.into_inner().expect("tar builder finish");

    let mut gz_bytes = Vec::new();
    let mut encoder = flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &tar_bytes).expect("gz write");
    encoder.finish().expect("gz finish");

    gz_bytes
}

/// Extract relevant files from in-memory tarball bytes.
///
/// Strips the top-level directory from each entry, filters through
/// [`is_download_relevant`], and rejects paths with `..` components
/// to prevent directory traversal. Only regular files are extracted;
/// directories are created implicitly via parent directory creation,
/// and symlinks are rejected.
pub fn extract_tarball(data: &[u8], out_dir: &Utf8Path) -> anyhow::Result<()> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);

    std::fs::create_dir_all(out_dir.as_std_path())?;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        let entry_path = entry.path()?.to_string_lossy().to_string();

        // Skip directory entries — parent dirs are created below as needed.
        if entry_type == tar::EntryType::Directory {
            continue;
        }

        // Reject symlinks and hard links to prevent path-based attacks.
        if entry_type == tar::EntryType::Symlink || entry_type == tar::EntryType::Link {
            anyhow::bail!("Refusing to extract link entry from tarball: {entry_path}");
        }

        // Only extract regular files (and Continuous, which is treated as regular).
        if entry_type != tar::EntryType::Regular && entry_type != tar::EntryType::Continuous {
            continue;
        }

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

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;

    fn temp_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        (dir, path)
    }

    #[test]
    fn extracts_regular_files() {
        let data = build_test_tarball(|builder| {
            let content = b"# tag code";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "Django-5.2/django/templatetags/i18n.py",
                    &content[..],
                )
                .unwrap();
        });

        let (_dir, out) = temp_dir();
        extract_tarball(&data, &out).unwrap();

        let extracted = out.join("django/templatetags/i18n.py");
        assert!(extracted.as_std_path().exists(), "file should be extracted");
        assert_eq!(
            std::fs::read_to_string(extracted.as_std_path()).unwrap(),
            "# tag code"
        );
    }

    #[test]
    fn skips_directory_entries() {
        let data = build_test_tarball(|builder| {
            // Add a directory entry
            let mut dir_header = tar::Header::new_gnu();
            dir_header.set_entry_type(tar::EntryType::Directory);
            dir_header.set_size(0);
            dir_header.set_mode(0o755);
            dir_header.set_cksum();
            builder
                .append_data(&mut dir_header, "Django-5.2/django/templatetags/", &[][..])
                .unwrap();

            // Add a regular file after the directory
            let content = b"# tag";
            let mut file_header = tar::Header::new_gnu();
            file_header.set_entry_type(tar::EntryType::Regular);
            file_header.set_size(content.len() as u64);
            file_header.set_mode(0o644);
            file_header.set_cksum();
            builder
                .append_data(
                    &mut file_header,
                    "Django-5.2/django/templatetags/i18n.py",
                    &content[..],
                )
                .unwrap();
        });

        let (_dir, out) = temp_dir();
        extract_tarball(&data, &out).unwrap();

        // The file should exist (created via parent dir creation)
        let file = out.join("django/templatetags/i18n.py");
        assert!(file.as_std_path().exists());

        // The directory entry itself should not have been written as a file
        let dir_as_file = out.join("django/templatetags");
        assert!(
            dir_as_file.as_std_path().is_dir(),
            "directory entry should not become a file"
        );
    }

    #[test]
    fn rejects_symlink_entries() {
        let data = build_test_tarball(|builder| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder
                .append_link(
                    &mut header,
                    "Django-5.2/django/templatetags/evil.py",
                    "/etc/passwd",
                )
                .unwrap();
        });

        let (_dir, out) = temp_dir();
        let result = extract_tarball(&data, &out);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Refusing to extract link entry"),
            "error should mention link rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_hard_link_entries() {
        let data = build_test_tarball(|builder| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Link);
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_link(
                    &mut header,
                    "Django-5.2/django/templatetags/evil.py",
                    "Django-5.2/django/templatetags/i18n.py",
                )
                .unwrap();
        });

        let (_dir, out) = temp_dir();
        let result = extract_tarball(&data, &out);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Refusing to extract link entry"),
            "error should mention link rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        // Build the tarball manually to bypass the tar crate's own `..` rejection.
        let content = b"pwned";
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header
            .set_path("Django-5.2/django/templatetags/safe.py")
            .unwrap();

        // Overwrite the path bytes directly to include `..`
        let evil_path = b"Django-5.2/django/templatetags/../../templatetags/evil.py";
        header.as_old_mut().name[..evil_path.len()].copy_from_slice(evil_path);
        header.as_old_mut().name[evil_path.len()] = 0;
        header.set_cksum();

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.append(&header, &content[..]).unwrap();
            builder.into_inner().unwrap();
        }

        let mut gz_bytes = Vec::new();
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        encoder.finish().unwrap();

        let (_dir, out) = temp_dir();
        let result = extract_tarball(&gz_bytes, &out);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Path traversal detected"),
            "error should mention path traversal, got: {err}"
        );
    }
}
