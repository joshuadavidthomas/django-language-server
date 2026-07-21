//! Tarball extraction for corpus sync.

use std::io::Read;

use camino::Utf8Path;

/// Extract a full repository snapshot from a gzipped tarball.
///
/// Strips the top-level directory from each entry and rejects paths with `..`
/// components to prevent directory traversal. Only regular files are extracted;
/// directories are created implicitly via parent directory creation, and
/// symlinks/hard links are silently skipped.
pub(crate) fn extract_tarball<R: Read>(
    reader: R,
    out_dir: &Utf8Path,
) -> anyhow::Result<Vec<String>> {
    let gz = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);

    std::fs::create_dir_all(out_dir.as_std_path())?;
    let mut warnings = Vec::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        let entry_path = entry.path()?.to_string_lossy().to_string();

        // Skip directory entries — parent dirs are created below as needed.
        if entry_type == tar::EntryType::Directory {
            continue;
        }

        // Skip symlinks and hard links — don't follow them, just ignore.
        if entry_type == tar::EntryType::Symlink || entry_type == tar::EntryType::Link {
            warnings.push(format!("skipping link entry: {entry_path}"));
            continue;
        }

        // Only extract regular files (and Continuous, which is treated as regular).
        if entry_type != tar::EntryType::Regular && entry_type != tar::EntryType::Continuous {
            continue;
        }

        // Strip the top-level directory (e.g., "Django-5.2.11/")
        let relative = entry_path
            .split_once('/')
            .map_or(entry_path.as_str(), |x| x.1);

        let relative_path = std::path::Path::new(relative);
        if relative_path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            anyhow::bail!("Invalid tarball entry path (absolute or traversal): {entry_path}");
        }

        let dest = out_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent.as_std_path())?;
        }

        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        std::fs::write(dest.as_std_path(), &content)?;
    }

    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;

    fn build_test_tarball(populate: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        populate(&mut builder);
        let tar_bytes = builder
            .into_inner()
            .expect("test tar builder should finish");

        let mut gz_bytes = Vec::new();
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).expect("test tarball should compress");
        encoder.finish().expect("test gzip encoder should finish");

        gz_bytes
    }

    fn temp_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test directory path should be UTF-8");
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
                .expect("regular file should be appended to test tarball");
        });

        let (_dir, out) = temp_dir();
        extract_tarball(data.as_slice(), &out).expect("regular file test tarball should extract");

        let extracted = out.join("django/templatetags/i18n.py");
        assert!(extracted.as_std_path().exists(), "file should be extracted");
        assert_eq!(
            std::fs::read_to_string(extracted.as_std_path())
                .expect("extracted test file should be readable"),
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
                .expect("directory should be appended to test tarball");

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
                .expect("regular file should follow directory in test tarball");
        });

        let (_dir, out) = temp_dir();
        extract_tarball(data.as_slice(), &out)
            .expect("test tarball with directory entry should extract");

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
    fn skips_symlink_entries() {
        let data = build_test_tarball(|builder| {
            // Add a symlink entry
            let mut sym_header = tar::Header::new_gnu();
            sym_header.set_entry_type(tar::EntryType::Symlink);
            sym_header.set_size(0);
            sym_header.set_mode(0o777);
            sym_header.set_cksum();
            builder
                .append_link(
                    &mut sym_header,
                    "Django-5.2/django/templatetags/evil.py",
                    "/etc/passwd",
                )
                .expect("symbolic link should be appended to test tarball");

            // Add a real file after the symlink
            let content = b"# tag code";
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
                .expect("regular file should follow symlink in test tarball");
        });

        let (_dir, out) = temp_dir();
        let warnings = extract_tarball(data.as_slice(), &out)
            .expect("test tarball containing a symlink should extract");

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("link entry"));
        // Symlink should not exist
        assert!(
            !out.join("django/templatetags/evil.py")
                .as_std_path()
                .exists()
        );
        // Real file should be extracted
        assert!(
            out.join("django/templatetags/i18n.py")
                .as_std_path()
                .exists()
        );
    }

    #[test]
    fn skips_hard_link_entries() {
        let data = build_test_tarball(|builder| {
            // Add a hard link entry
            let mut link_header = tar::Header::new_gnu();
            link_header.set_entry_type(tar::EntryType::Link);
            link_header.set_size(0);
            link_header.set_mode(0o644);
            link_header.set_cksum();
            builder
                .append_link(
                    &mut link_header,
                    "Django-5.2/django/templatetags/evil.py",
                    "Django-5.2/django/templatetags/i18n.py",
                )
                .expect("hard link should be appended to test tarball");

            // Add a real file after the hard link
            let content = b"# tag code";
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
                .expect("regular file should follow hard link in test tarball");
        });

        let (_dir, out) = temp_dir();
        let warnings = extract_tarball(data.as_slice(), &out)
            .expect("test tarball containing a hard link should extract");

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("link entry"));
        // Hard link should not exist
        assert!(
            !out.join("django/templatetags/evil.py")
                .as_std_path()
                .exists()
        );
        // Real file should be extracted
        assert!(
            out.join("django/templatetags/i18n.py")
                .as_std_path()
                .exists()
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
            .expect("safe traversal-test tar path should be accepted");

        // Overwrite the path bytes directly to include `..`
        let evil_path = b"Django-5.2/django/templatetags/../../templatetags/evil.py";
        header.as_old_mut().name[..evil_path.len()].copy_from_slice(evil_path);
        header.as_old_mut().name[evil_path.len()] = 0;
        header.set_cksum();

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder
                .append(&header, &content[..])
                .expect("traversal entry should be appended to test tarball");
            builder
                .finish()
                .expect("traversal test tarball should finish");
        }

        let mut gz_bytes = Vec::new();
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes)
            .expect("traversal test tarball should compress");
        encoder
            .finish()
            .expect("traversal test gzip encoder should finish");

        let (_dir, out) = temp_dir();
        let err = extract_tarball(gz_bytes.as_slice(), &out)
            .expect_err("tarball path traversal should be rejected")
            .to_string();
        assert!(
            err.contains("Invalid tarball entry path"),
            "error should mention invalid path, got: {err}"
        );
    }

    #[test]
    fn rejects_absolute_paths() {
        // Build the tarball manually to bypass any path normalization.
        let content = b"pwned";
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header
            .set_path("Django-5.2/django/templatetags/safe.py")
            .expect("safe absolute-path test tar path should be accepted");

        // After stripping top-level dir, this becomes "/django/templatetags/evil.py".
        let evil_path = b"Django-5.2//django/templatetags/evil.py";
        header.as_old_mut().name[..evil_path.len()].copy_from_slice(evil_path);
        header.as_old_mut().name[evil_path.len()] = 0;
        header.set_cksum();

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder
                .append(&header, &content[..])
                .expect("absolute entry should be appended to test tarball");
            builder
                .finish()
                .expect("absolute-path test tarball should finish");
        }

        let mut gz_bytes = Vec::new();
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes)
            .expect("absolute-path test tarball should compress");
        encoder
            .finish()
            .expect("absolute-path test gzip encoder should finish");

        let (_dir, out) = temp_dir();
        let err = extract_tarball(gz_bytes.as_slice(), &out)
            .expect_err("absolute tarball path should be rejected")
            .to_string();
        assert!(
            err.contains("Invalid tarball entry path"),
            "error should mention invalid path, got: {err}"
        );
    }
}
