use camino::Utf8Path;
use camino::Utf8PathBuf;
use walkdir::WalkDir;

/// Walk the given paths and collect files that pass `predicate`.
///
/// Each entry in `paths` may be a file or a directory:
/// - Files are included directly if `predicate` returns `true`.
/// - Directories are walked recursively; only matching files are collected.
/// - Directories where `skip_dir` returns `true` are pruned from traversal.
///
/// Returns a sorted, deduplicated list of absolute paths.
#[must_use]
pub fn walk_files(
    paths: &[Utf8PathBuf],
    predicate: impl Fn(&Utf8Path) -> bool,
    skip_dir: impl Fn(&Utf8Path) -> bool,
) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            if predicate(path) {
                let resolved = dunce_utf8(path).unwrap_or_else(|_| path.clone());
                files.push(resolved);
            }
        } else if path.is_dir() {
            for entry in WalkDir::new(path)
                .into_iter()
                .filter_entry(|e| {
                    if e.depth() == 0 || !e.file_type().is_dir() {
                        return true;
                    }
                    camino::Utf8Path::from_path(e.path()).is_none_or(|p| !skip_dir(p))
                })
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let Some(utf8) = camino::Utf8Path::from_path(entry.path()) else {
                    continue;
                };
                if predicate(utf8) {
                    let resolved = dunce_utf8(utf8).unwrap_or_else(|_| utf8.to_owned());
                    files.push(resolved);
                }
            }
        }
    }

    files.sort();
    files.dedup();
    files
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

    fn is_html(path: &Utf8Path) -> bool {
        path.extension() == Some("html")
    }

    fn no_skip(_path: &Utf8Path) -> bool {
        false
    }

    fn skip_hidden(path: &Utf8Path) -> bool {
        path.file_name().is_some_and(|name| name.starts_with('.'))
    }

    #[test]
    fn walks_directory_with_predicate() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::fs::write(dir.path().join("page.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.path().join("style.css"), "body {}").unwrap();
        std::fs::write(dir.path().join("app.js"), "console.log()").unwrap();

        let files = walk_files(&[dir_path], is_html, no_skip);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"page.html"));
        assert!(!names.contains(&"style.css"));
        assert!(!names.contains(&"app.js"));
    }

    #[test]
    fn skip_dir_prunes_directories() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let hidden = dir.path().join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("secret.html"), "<p>secret</p>").unwrap();
        std::fs::write(dir.path().join("visible.html"), "<p>visible</p>").unwrap();

        let files = walk_files(&[dir_path], is_html, skip_hidden);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"visible.html"));
        assert!(!names.contains(&"secret.html"));
    }

    #[test]
    fn no_skip_includes_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let hidden = dir.path().join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("secret.html"), "<p>secret</p>").unwrap();
        std::fs::write(dir.path().join("visible.html"), "<p>visible</p>").unwrap();

        let files = walk_files(&[dir_path], is_html, no_skip);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"visible.html"));
        assert!(names.contains(&"secret.html"));
    }

    #[test]
    fn single_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("single.html");
        std::fs::write(&file_path, "<p>single</p>").unwrap();
        let utf8 = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_files(&[utf8], is_html, no_skip);
        assert_eq!(files.len(), 1);
        assert!(files[0].file_name() == Some("single.html"));
    }

    #[test]
    fn non_matching_file_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("script.py");
        std::fs::write(&file_path, "print('hi')").unwrap();
        let utf8 = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_files(&[utf8], is_html, no_skip);
        assert!(files.is_empty());
    }

    #[test]
    fn deduplicates_results() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let file_path = dir.path().join("page.html");
        std::fs::write(&file_path, "<p>dup</p>").unwrap();
        let utf8_file = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_files(&[dir_path, utf8_file], is_html, no_skip);
        let html_count = files
            .iter()
            .filter(|p| p.file_name() == Some("page.html"))
            .count();
        assert_eq!(html_count, 1);
    }
}
