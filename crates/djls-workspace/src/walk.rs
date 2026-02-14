use camino::Utf8Path;
use camino::Utf8PathBuf;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

/// Options controlling how `walk_files` traverses directories.
///
/// Mirrors ripgrep's file-filtering CLI flags. All options map directly to
/// methods on the `ignore` crate's `WalkBuilder`.
#[derive(Clone, Debug, Default)]
pub struct WalkOptions {
    /// Include hidden files and directories (those starting with `.`).
    pub hidden: bool,
    /// Gitignore-style glob patterns. Prefix with `!` to exclude.
    /// Later patterns take precedence over earlier ones.
    pub globs: Vec<String>,
    /// Disable all ignore files (`.gitignore`, `.ignore`, etc.).
    pub no_ignore: bool,
    /// Follow symbolic links.
    pub follow_links: bool,
    /// Maximum directory recursion depth. `None` means unlimited.
    pub max_depth: Option<usize>,
}

/// Walk the given paths and collect files that pass `predicate`.
///
/// Each entry in `paths` may be a file or a directory:
/// - Files are included directly if `predicate` returns `true`.
/// - Directories are walked recursively; only matching files are collected.
///
/// By default, hidden files/directories are skipped and `.gitignore` rules
/// are respected (via the `ignore` crate). Use [`WalkOptions`] to customize
/// filtering behavior.
///
/// Returns a sorted, deduplicated list of absolute paths.
#[must_use]
pub fn walk_files(
    paths: &[Utf8PathBuf],
    predicate: impl Fn(&Utf8Path) -> bool,
    options: &WalkOptions,
) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            if predicate(path) {
                let resolved = dunce_utf8(path).unwrap_or_else(|_| path.clone());
                files.push(resolved);
            }
            continue;
        }

        if !path.is_dir() {
            continue;
        }

        let mut builder = WalkBuilder::new(path.as_std_path());
        // Call standard_filters first — it sets hidden, gitignore, etc.
        // Then override individual settings after.
        builder
            .standard_filters(!options.no_ignore)
            .hidden(!options.hidden)
            .follow_links(options.follow_links);

        if let Some(depth) = options.max_depth {
            builder.max_depth(Some(depth));
        }

        if !options.globs.is_empty() {
            let mut overrides = OverrideBuilder::new(path.as_std_path());
            for glob in &options.globs {
                // OverrideBuilder returns Err only for invalid globs;
                // skip silently (matching rg behavior of warn + continue).
                let _ = overrides.add(glob);
            }
            if let Ok(built) = overrides.build() {
                builder.overrides(built);
            }
        }

        let walker = builder.build();

        for entry in walker.filter_map(Result::ok) {
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
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

    fn defaults() -> WalkOptions {
        WalkOptions::default()
    }

    #[test]
    fn walks_directory_with_predicate() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::fs::write(dir.path().join("page.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.path().join("style.css"), "body {}").unwrap();
        std::fs::write(dir.path().join("app.js"), "console.log()").unwrap();

        let files = walk_files(&[dir_path], is_html, &defaults());
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"page.html"));
        assert!(!names.contains(&"style.css"));
        assert!(!names.contains(&"app.js"));
    }

    #[test]
    fn hidden_false_skips_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let hidden = dir.path().join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("secret.html"), "<p>secret</p>").unwrap();
        std::fs::write(dir.path().join("visible.html"), "<p>visible</p>").unwrap();

        let files = walk_files(&[dir_path], is_html, &defaults());
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"visible.html"));
        assert!(!names.contains(&"secret.html"));
    }

    #[test]
    fn hidden_true_includes_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let hidden = dir.path().join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("secret.html"), "<p>secret</p>").unwrap();
        std::fs::write(dir.path().join("visible.html"), "<p>visible</p>").unwrap();

        let opts = WalkOptions {
            hidden: true,
            ..defaults()
        };
        let files = walk_files(&[dir_path], is_html, &opts);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"visible.html"));
        assert!(names.contains(&"secret.html"));
    }

    #[test]
    fn respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();

        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();

        let ignored = dir.path().join("ignored");
        std::fs::create_dir_all(&ignored).unwrap();
        std::fs::write(ignored.join("skip.html"), "<p>skip</p>").unwrap();
        std::fs::write(dir.path().join("keep.html"), "<p>keep</p>").unwrap();

        let files = walk_files(&[dir_path], is_html, &defaults());
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"keep.html"));
        assert!(!names.contains(&"skip.html"));
    }

    #[test]
    fn no_ignore_disables_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();

        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();

        let ignored = dir.path().join("ignored");
        std::fs::create_dir_all(&ignored).unwrap();
        std::fs::write(ignored.join("found.html"), "<p>found</p>").unwrap();
        std::fs::write(dir.path().join("keep.html"), "<p>keep</p>").unwrap();

        let opts = WalkOptions {
            no_ignore: true,
            ..defaults()
        };
        let files = walk_files(&[dir_path], is_html, &opts);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"keep.html"));
        assert!(names.contains(&"found.html"));
    }

    #[test]
    fn glob_includes_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::fs::write(dir.path().join("page.html"), "<p>page</p>").unwrap();
        std::fs::write(dir.path().join("other.html"), "<p>other</p>").unwrap();
        std::fs::write(dir.path().join("style.css"), "body {}").unwrap();

        let opts = WalkOptions {
            globs: vec!["page.*".to_string()],
            ..defaults()
        };
        let files = walk_files(&[dir_path], is_html, &opts);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"page.html"));
        assert!(!names.contains(&"other.html"));
    }

    #[test]
    fn glob_excludes_with_negation() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::fs::write(dir.path().join("page.html"), "<p>page</p>").unwrap();
        std::fs::write(dir.path().join("skip.html"), "<p>skip</p>").unwrap();

        let opts = WalkOptions {
            globs: vec!["!skip.*".to_string()],
            ..defaults()
        };
        let files = walk_files(&[dir_path], is_html, &opts);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"page.html"));
        assert!(!names.contains(&"skip.html"));
    }

    #[test]
    fn max_depth_limits_recursion() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        std::fs::write(dir.path().join("top.html"), "<p>top</p>").unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("deep.html"), "<p>deep</p>").unwrap();

        let opts = WalkOptions {
            max_depth: Some(1),
            ..defaults()
        };
        let files = walk_files(&[dir_path], is_html, &opts);
        let names: Vec<&str> = files.iter().filter_map(|p| p.file_name()).collect();

        assert!(names.contains(&"top.html"));
        assert!(!names.contains(&"deep.html"));
    }

    #[test]
    fn follow_links_traverses_symlinks() {
        // Create the symlink target OUTSIDE the walked directory so the
        // file is only reachable through the symlink.
        let walked = tempfile::tempdir().unwrap();
        let walked_path = Utf8PathBuf::from_path_buf(walked.path().to_path_buf()).unwrap();

        let external = tempfile::tempdir().unwrap();
        let target = external.path().join("templates");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("linked.html"), "<p>linked</p>").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, walked.path().join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&target, walked.path().join("link")).unwrap();

        let without = walk_files(&[walked_path.clone()], is_html, &defaults());
        assert!(
            without.is_empty(),
            "file should not be found without follow_links"
        );

        let opts = WalkOptions {
            follow_links: true,
            ..defaults()
        };
        let with = walk_files(&[walked_path], is_html, &opts);
        let names: Vec<&str> = with.iter().filter_map(|p| p.file_name()).collect();
        assert!(
            names.contains(&"linked.html"),
            "file should be found with follow_links"
        );
    }

    #[test]
    fn single_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("single.html");
        std::fs::write(&file_path, "<p>single</p>").unwrap();
        let utf8 = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_files(&[utf8], is_html, &defaults());
        assert_eq!(files.len(), 1);
        assert!(files[0].file_name() == Some("single.html"));
    }

    #[test]
    fn non_matching_file_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("script.py");
        std::fs::write(&file_path, "print('hi')").unwrap();
        let utf8 = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_files(&[utf8], is_html, &defaults());
        assert!(files.is_empty());
    }

    #[test]
    fn deduplicates_results() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let file_path = dir.path().join("page.html");
        std::fs::write(&file_path, "<p>dup</p>").unwrap();
        let utf8_file = Utf8PathBuf::from_path_buf(file_path).unwrap();

        let files = walk_files(&[dir_path, utf8_file], is_html, &defaults());
        let html_count = files
            .iter()
            .filter(|p| p.file_name() == Some("page.html"))
            .count();
        assert_eq!(html_count, 1);
    }
}
