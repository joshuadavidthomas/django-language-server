use std::io;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::DiscoveredSourceFile;
use djls_source::FileSetSummary;
use djls_source::SourceRoot;
use djls_source::SourceRootId;

use crate::walk_files;
use crate::WalkOptions;

pub type FileLoadPredicate = Box<dyn Fn(&Utf8Path) -> bool + Send + Sync>;

/// A neutral file-loading request over already-constructed source roots.
///
/// This loader preserves caller-provided roots. Root normalization,
/// deduplication, and duplicate-root project facts belong to the project-owned
/// root-construction seam introduced in the loading phases.
pub struct FilesForRootsRequest {
    roots: Vec<SourceRoot>,
    predicate: FileLoadPredicate,
    options: WalkOptions,
}

impl FilesForRootsRequest {
    #[must_use]
    pub fn new(roots: Vec<SourceRoot>, predicate: FileLoadPredicate, options: WalkOptions) -> Self {
        Self {
            roots,
            predicate,
            options,
        }
    }

    #[must_use]
    pub fn roots(&self) -> &[SourceRoot] {
        &self.roots
    }

    #[must_use]
    pub fn options(&self) -> &WalkOptions {
        &self.options
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesForRootsResult {
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
    root_issues: Vec<WorkspaceRootIssue>,
}

impl FilesForRootsResult {
    #[must_use]
    pub fn roots(&self) -> &[SourceRoot] {
        &self.roots
    }

    #[must_use]
    pub fn files(&self) -> &[DiscoveredSourceFile] {
        &self.files
    }

    #[must_use]
    pub fn summary(&self) -> &FileSetSummary {
        &self.summary
    }

    /// Root preflight issues detected before delegating to `walk_files`.
    ///
    /// This is not complete traversal evidence. Descendant walk errors are not
    /// reported until `walk_files` grows a typed traversal-error result.
    #[must_use]
    pub fn root_issues(&self) -> &[WorkspaceRootIssue] {
        &self.root_issues
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceRootIssue {
    MissingRoot {
        root: SourceRootId,
        path: Utf8PathBuf,
    },
    UnreadableRoot {
        root: SourceRootId,
        path: Utf8PathBuf,
        error_kind: io::ErrorKind,
    },
}

#[must_use]
pub fn load_files_for_roots(request: FilesForRootsRequest) -> FilesForRootsResult {
    let mut roots = Vec::new();
    let mut files = Vec::new();
    let mut issues = Vec::new();

    for root in request.roots {
        match root.path().try_exists() {
            Ok(true) => {}
            Ok(false) => {
                issues.push(WorkspaceRootIssue::MissingRoot {
                    root: root.id().clone(),
                    path: root.path().to_owned(),
                });
                roots.push(root);
                continue;
            }
            Err(error) => {
                issues.push(WorkspaceRootIssue::UnreadableRoot {
                    root: root.id().clone(),
                    path: root.path().to_owned(),
                    error_kind: error.kind(),
                });
                roots.push(root);
                continue;
            }
        }

        if let Err(error) = root.path().read_dir_utf8() {
            issues.push(WorkspaceRootIssue::UnreadableRoot {
                root: root.id().clone(),
                path: root.path().to_owned(),
                error_kind: error.kind(),
            });
        }

        let paths = walk_files(
            &[root.path().to_owned()],
            |path| (request.predicate)(path),
            &request.options,
        );
        files.extend(
            paths
                .into_iter()
                .map(|path| DiscoveredSourceFile::new(path, root.id().clone())),
        );
        roots.push(root);
    }

    // Merge per-root `walk_files` results while keeping the same path when it
    // belongs to distinct root identities.
    files.sort_by(|left, right| {
        left.path()
            .cmp(right.path())
            .then(left.root().cmp(right.root()))
    });
    files.dedup_by(|left, right| left.path() == right.path() && left.root() == right.root());
    let summary = FileSetSummary::new(files.len());

    FilesForRootsResult {
        roots,
        files,
        summary,
        root_issues: issues,
    }
}

#[cfg(test)]
mod tests {
    use djls_source::FileKind;
    use djls_source::FileRootKind;

    use super::*;

    fn utf8(path: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap()
    }

    fn root(path: Utf8PathBuf) -> SourceRoot {
        SourceRoot::new(SourceRootId::new(path.clone()), path, FileRootKind::Project)
    }

    fn html_predicate() -> FileLoadPredicate {
        Box::new(|path| path.extension() == Some("html"))
    }

    fn any_predicate() -> FileLoadPredicate {
        Box::new(|_| true)
    }

    #[test]
    fn delegates_to_walk_files_and_preserves_sorting_and_deduplication() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = utf8(dir.path());
        std::fs::write(dir.path().join("b.html"), "b").unwrap();
        std::fs::write(dir.path().join("a.html"), "a").unwrap();

        let request = FilesForRootsRequest::new(
            vec![root(dir_path.clone()), root(dir_path)],
            html_predicate(),
            WalkOptions::default(),
        );
        let result = load_files_for_roots(request);
        let names: Vec<&str> = result
            .files()
            .iter()
            .filter_map(|file| file.path().file_name())
            .collect();

        assert_eq!(names, vec!["a.html", "b.html"]);
        assert_eq!(result.summary().included_files(), 2);
    }

    #[test]
    fn excludes_gitignored_files_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored/skip.html"), "skip").unwrap();
        std::fs::write(dir.path().join("keep.html"), "keep").unwrap();

        let request = FilesForRootsRequest::new(
            vec![root(utf8(dir.path()))],
            html_predicate(),
            WalkOptions::default(),
        );
        let result = load_files_for_roots(request);
        let names: Vec<&str> = result
            .files()
            .iter()
            .filter_map(|file| file.path().file_name())
            .collect();

        assert_eq!(names, vec!["keep.html"]);
    }

    #[test]
    fn excludes_hidden_files_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden.html"), "hidden").unwrap();
        std::fs::write(dir.path().join("visible.html"), "visible").unwrap();

        let request = FilesForRootsRequest::new(
            vec![root(utf8(dir.path()))],
            html_predicate(),
            WalkOptions::default(),
        );
        let result = load_files_for_roots(request);
        let names: Vec<&str> = result
            .files()
            .iter()
            .filter_map(|file| file.path().file_name())
            .collect();

        assert_eq!(names, vec!["visible.html"]);
    }

    #[test]
    fn applies_caller_provided_predicate_without_interpreting_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.py"), "print('hi')").unwrap();
        std::fs::write(dir.path().join("page.html"), "html").unwrap();

        let request = FilesForRootsRequest::new(
            vec![root(utf8(dir.path()))],
            Box::new(|path| path.file_name() == Some("app.py")),
            WalkOptions::default(),
        );
        let result = load_files_for_roots(request);
        let files = result.files();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path().file_name(), Some("app.py"));
        assert_eq!(files[0].kind(), FileKind::Python);
    }

    #[test]
    fn produces_included_counts_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("one.py"), "").unwrap();
        std::fs::write(dir.path().join("two.py"), "").unwrap();

        let request = FilesForRootsRequest::new(
            vec![root(utf8(dir.path()))],
            any_predicate(),
            WalkOptions::default(),
        );
        let result = load_files_for_roots(request);

        assert_eq!(result.summary(), &FileSetSummary::new(2));
    }

    #[test]
    fn reports_missing_roots_as_root_preflight_issues_without_readiness_policy() {
        let dir = tempfile::tempdir().unwrap();
        let missing = utf8(dir.path()).join("missing");
        let request = FilesForRootsRequest::new(
            vec![root(missing.clone())],
            any_predicate(),
            WalkOptions::default(),
        );
        let result = load_files_for_roots(request);

        assert!(result.files().is_empty());
        assert_eq!(result.root_issues().len(), 1);
        assert!(matches!(
            &result.root_issues()[0],
            WorkspaceRootIssue::MissingRoot { path, .. } if path == &missing
        ));
    }

    #[test]
    fn preserves_root_identity_for_duplicate_and_overlapping_roots() {
        let dir = tempfile::tempdir().unwrap();
        let parent = utf8(dir.path());
        let child = parent.join("child");
        std::fs::create_dir_all(child.as_std_path()).unwrap();
        std::fs::write(child.join("page.html").as_std_path(), "page").unwrap();

        let parent_root = SourceRoot::new(
            SourceRootId::new(Utf8PathBuf::from("/identity/parent")),
            parent.clone(),
            FileRootKind::Project,
        );
        let child_root = SourceRoot::new(
            SourceRootId::new(Utf8PathBuf::from("/identity/child")),
            child,
            FileRootKind::Project,
        );
        let request = FilesForRootsRequest::new(
            vec![parent_root.clone(), child_root.clone()],
            html_predicate(),
            WalkOptions::default(),
        );
        let result = load_files_for_roots(request);
        let roots: Vec<&SourceRootId> = result
            .files()
            .iter()
            .map(DiscoveredSourceFile::root)
            .collect();

        assert!(roots.contains(&parent_root.id()));
        assert!(roots.contains(&child_root.id()));
        assert!(result
            .roots()
            .iter()
            .any(|root| root.id() == parent_root.id()));
        assert!(result
            .roots()
            .iter()
            .any(|root| root.id() == child_root.id()));
    }
}
