use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileKind;
use walkdir::WalkDir;

/// Walk the given paths and collect all files that match [`FileKind::Template`].
///
/// Each entry in `paths` may be a file or a directory:
/// - Files are included directly if their extension matches a template kind.
/// - Directories are walked recursively; only template files are collected.
///
/// Hidden files and directories (names starting with `.`) are skipped.
///
/// Returns a sorted, deduplicated list of absolute paths.
#[must_use]
pub fn walk_template_files(paths: &[Utf8PathBuf]) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            if is_template(path) {
                if let Ok(canonical) = dunce_utf8(path) {
                    files.push(canonical);
                } else {
                    files.push(path.clone());
                }
            }
        } else if path.is_dir() {
            for entry in WalkDir::new(path)
                .into_iter()
                .filter_entry(|e| e.depth() == 0 || !is_hidden(e))
                .flatten()
            {
                if entry.file_type().is_file() {
                    if let Some(utf8) = camino::Utf8Path::from_path(entry.path()) {
                        if is_template(utf8) {
                            if let Ok(canonical) = dunce_utf8(utf8) {
                                files.push(canonical);
                            } else {
                                files.push(utf8.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }

    files.sort();
    files.dedup();
    files
}

fn is_template(path: &Utf8Path) -> bool {
    FileKind::from(path) == FileKind::Template
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| name.starts_with('.'))
}

fn dunce_utf8(path: &Utf8Path) -> std::io::Result<Utf8PathBuf> {
    let canonical = path.as_std_path().canonicalize()?;
    #[cfg(windows)]
    let canonical = dunce::simplified(&canonical).to_path_buf();
    Utf8PathBuf::from_path_buf(canonical)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF-8 path"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_directory_for_templates() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::fs::write(dir.path().join("page.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.path().join("style.css"), "body {}").unwrap();
        std::fs::write(dir.path().join("base.djhtml"), "{% block %}{% endblock %}").unwrap();

        let files = walk_template_files(&[dir_path]);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"page.html"));
        assert!(names.contains(&"base.djhtml"));
        assert!(!names.contains(&"style.css"));
    }

    #[test]
    fn skips_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let hidden = dir.path().join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("secret.html"), "<p>secret</p>").unwrap();
        std::fs::write(dir.path().join("visible.html"), "<p>visible</p>").unwrap();

        let files = walk_template_files(&[dir_path]);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"visible.html"));
        assert!(!names.contains(&"secret.html"));
    }

    #[test]
    fn single_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("single.html");
        std::fs::write(&file_path, "<p>single</p>").unwrap();
        let utf8 = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_template_files(&[utf8]);
        assert_eq!(files.len(), 1);
        assert!(files[0].file_name() == Some("single.html"));
    }

    #[test]
    fn non_template_file_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("script.py");
        std::fs::write(&file_path, "print('hi')").unwrap();
        let utf8 = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_template_files(&[utf8]);
        assert!(files.is_empty());
    }

    #[test]
    fn deduplicates_results() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let file_path = dir.path().join("page.html");
        std::fs::write(&file_path, "<p>dup</p>").unwrap();
        let utf8_file = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_template_files(&[dir_path, utf8_file]);
        let html_count = files
            .iter()
            .filter(|p| p.file_name() == Some("page.html"))
            .count();
        assert_eq!(html_count, 1);
    }
}
