//! Filesystem abstraction for source reads and project discovery.
//!
//! DJLS keeps filesystem access behind this trait so the same source queries can
//! read from the operating system, tests, benchmark source maps, and the LSP
//! overlay that includes unsaved editor buffers.
//!
//! The API is path-based by design. Django template features require filesystem
//! context (`INSTALLED_APPS`, template loaders, settings modules, and source
//! roots), and Salsa inputs are already keyed by `Utf8PathBuf`. URI handling
//! belongs at the LSP boundary, not in semantic project discovery.

use std::io;
use std::sync::Mutex;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use rustc_hash::FxHashMap;

/// Options controlling filesystem traversal.
///
/// The flags intentionally mirror ripgrep's file-filtering CLI options and map
/// directly to the `ignore` crate's `WalkBuilder`. DJLS uses the same traversal
/// policy for command-line checks and Project Facts discovery.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalkOptions {
    /// Include hidden files and directories (those starting with `.`).
    pub hidden: bool,
    /// Gitignore-style glob patterns. Prefix with `!` to exclude.
    /// Later patterns take precedence over earlier patterns.
    pub globs: Vec<String>,
    /// Disable all ignore files (`.gitignore`, `.ignore`, etc.).
    pub no_ignore: bool,
    /// Follow symbolic links.
    pub follow_links: bool,
    /// Maximum directory recursion depth. `None` means unlimited.
    pub max_depth: Option<usize>,
}

impl WalkOptions {
    /// Walk every entry under a root without hidden-file or ignore filtering.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            hidden: true,
            no_ignore: true,
            ..Self::default()
        }
    }

    /// Walk first-party project entries, respecting hidden-file and ignore rules.
    #[must_use]
    pub fn project() -> Self {
        Self::default()
    }

    /// Walk library search path entries without hidden-file or ignore filtering.
    #[must_use]
    pub fn library_search_path() -> Self {
        Self::unrestricted()
    }

    /// Walk immediate children without hidden-file or ignore filtering.
    #[must_use]
    pub fn shallow() -> Self {
        Self {
            max_depth: Some(1),
            ..Self::unrestricted()
        }
    }
}

/// Kind of entry discovered under a walk root.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WalkEntryKind {
    File,
    Directory,
    Other,
}

impl From<std::fs::FileType> for WalkEntryKind {
    fn from(file_type: std::fs::FileType) -> Self {
        if file_type.is_file() {
            Self::File
        } else if file_type.is_dir() {
            Self::Directory
        } else {
            Self::Other
        }
    }
}

/// Known case-sensitivity behavior for a filesystem view.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CaseSensitivity {
    CaseSensitive,
    CaseInsensitive,
    Unknown,
}

impl CaseSensitivity {
    /// Return whether the filesystem is known to be case-sensitive.
    #[must_use]
    pub fn is_case_sensitive(&self) -> bool {
        matches!(self, Self::CaseSensitive)
    }
}

/// An entry discovered under a walk root.
///
/// Each entry carries both the resolved path and the path relative to the root
/// that produced it. Source discovery uses the relative path for module and
/// template-name mapping while preserving the full path for reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkEntry {
    /// Root path passed to `walk_root`.
    pub root: Utf8PathBuf,
    /// Full path to the discovered entry.
    pub path: Utf8PathBuf,
    /// Entry path relative to `root`.
    pub relative: Utf8PathBuf,
    /// Entry type according to this filesystem view.
    pub kind: WalkEntryKind,
}

impl WalkEntry {
    #[must_use]
    pub fn file_root(path: &Utf8Path) -> Self {
        let relative = path
            .file_name()
            .map_or_else(|| Utf8PathBuf::from(path.as_str()), Utf8PathBuf::from);
        Self {
            root: path.to_path_buf(),
            path: path.to_path_buf(),
            relative,
            kind: WalkEntryKind::File,
        }
    }
}

/// Result of walking a filesystem root.
///
/// A walk never hides partial results: entries discovered before a traversal
/// problem stay in [`Directory`](Self::Directory) alongside the problem itself,
/// and the root's disposition is reported from the same observation as the
/// entries rather than requiring a separate probe. Issues carry only stable
/// error kinds so outcomes do not depend on platform-specific error messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootWalk {
    /// The root does not exist (or is neither file nor directory): an
    /// exhaustively empty search, not an error.
    Missing,
    /// The root is itself a file rather than a directory to traverse.
    File(WalkEntry),
    /// The root is a directory that was traversed. Issues weaken any claim of
    /// exhaustiveness but never erase found entries.
    Directory {
        entries: Vec<WalkEntry>,
        issues: Vec<io::ErrorKind>,
    },
    /// The root could not be classified at all; nothing under it is knowable.
    Inaccessible(io::ErrorKind),
}

/// Filesystem view used by source and semantic crates.
///
/// Implementations may read from disk, memory, an overlay, or a benchmark source
/// map. Callers should depend on this trait for project/source discovery instead
/// of reaching for `std::fs` directly.
pub trait FileSystem: Send + Sync {
    /// Read a UTF-8 text file from this filesystem view.
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;
    /// Return whether a path exists as a file or directory.
    fn exists(&self, path: &Utf8Path) -> bool;
    /// Return whether a path is a regular file.
    fn is_file(&self, path: &Utf8Path) -> bool;
    /// Return whether a path is a directory.
    fn is_dir(&self, path: &Utf8Path) -> bool;
    /// Return the known case-sensitivity behavior for this filesystem view.
    fn case_sensitivity(&self) -> CaseSensitivity;
    /// Return whether a path exists with exact on-disk casing after `prefix`.
    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool;
    /// Walk a root using the supplied traversal policy, reporting the root's
    /// disposition alongside any discovered entries and traversal issues.
    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk;
}

impl<T> FileSystem for Mutex<T>
where
    T: FileSystem,
{
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .exists(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_file(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_dir(path)
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .path_exists_case_sensitive(path, prefix)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .walk_root(root, options)
    }
}

/// In-memory filesystem used by tests and stdin-backed command execution.
#[derive(Clone)]
pub struct InMemoryFileSystem {
    files: FxHashMap<Utf8PathBuf, String>,
    case_sensitivity: CaseSensitivity,
}

impl InMemoryFileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: FxHashMap::default(),
            case_sensitivity: CaseSensitivity::CaseSensitive,
        }
    }

    #[must_use]
    pub fn case_insensitive() -> Self {
        Self {
            files: FxHashMap::default(),
            case_sensitivity: CaseSensitivity::CaseInsensitive,
        }
    }

    pub fn add_file(&mut self, path: Utf8PathBuf, content: String) {
        self.files.insert(path, content);
    }

    pub fn remove_file(&mut self, path: &Utf8Path) {
        if self.case_sensitivity.is_case_sensitive() {
            self.files.remove(path);
        } else if let Some(stored_path) = self.matching_file_path(path).cloned() {
            self.files.remove(&stored_path);
        }
    }

    fn matching_file_path(&self, path: &Utf8Path) -> Option<&Utf8PathBuf> {
        if self.case_sensitivity.is_case_sensitive() {
            self.files.get_key_value(path).map(|(path, _)| path)
        } else {
            self.files
                .keys()
                .find(|stored_path| paths_eq_ignore_ascii_case(stored_path, path))
        }
    }

    fn exact_dir_exists(&self, path: &Utf8Path) -> bool {
        !self.files.contains_key(path) && self.files.keys().any(|file| file.starts_with(path))
    }

    fn matching_dir_path(&self, path: &Utf8Path) -> bool {
        if self.case_sensitivity.is_case_sensitive() {
            self.exact_dir_exists(path)
        } else {
            self.matching_file_path(path).is_none()
                && self
                    .files
                    .keys()
                    .any(|file| path_starts_with_ignore_ascii_case(file, path))
        }
    }
}

impl Default for InMemoryFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

fn paths_eq_ignore_ascii_case(left: &Utf8Path, right: &Utf8Path) -> bool {
    let left_components = left.components().collect::<Vec<_>>();
    let right_components = right.components().collect::<Vec<_>>();
    left_components.len() == right_components.len()
        && left_components
            .iter()
            .zip(right_components)
            .all(|(left, right)| left.as_str().eq_ignore_ascii_case(right.as_str()))
}

fn path_starts_with_ignore_ascii_case(path: &Utf8Path, prefix: &Utf8Path) -> bool {
    let path_components = path.components().collect::<Vec<_>>();
    let prefix_components = prefix.components().collect::<Vec<_>>();
    path_components.len() >= prefix_components.len()
        && path_components
            .iter()
            .zip(prefix_components)
            .all(|(path, prefix)| path.as_str().eq_ignore_ascii_case(prefix.as_str()))
}

fn components_after_prefix<'a>(path: &'a Utf8Path, prefix: &Utf8Path) -> Option<Vec<&'a str>> {
    if !path_starts_with_ignore_ascii_case(path, prefix) {
        return None;
    }

    Some(
        path.components()
            .skip(prefix.components().count())
            .map(|component| component.as_str())
            .collect(),
    )
}

impl FileSystem for InMemoryFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.matching_file_path(path)
            .and_then(|path| self.files.get(path))
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.is_file(path) || self.is_dir(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.matching_file_path(path).is_some()
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.matching_dir_path(path)
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        if self.case_sensitivity.is_case_sensitive() {
            return self.exists(path);
        }

        let Some(requested_suffix) = components_after_prefix(path, prefix) else {
            return false;
        };

        self.files.keys().any(|stored_path| {
            let Some(stored_suffix) = components_after_prefix(stored_path, prefix) else {
                return false;
            };
            stored_suffix.starts_with(&requested_suffix)
        })
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        let root_file = self.files.keys().find(|path| {
            if self.case_sensitivity.is_case_sensitive() {
                *path == root
            } else {
                paths_eq_ignore_ascii_case(path, root)
            }
        });
        if let Some(path) = root_file {
            return RootWalk::File(WalkEntry::file_root(path));
        }
        if !self.exists(root) {
            return RootWalk::Missing;
        }

        let mut entries = Vec::new();
        for path in self.files.keys() {
            let file_relative_components: Vec<_> = if self.case_sensitivity.is_case_sensitive() {
                if !path.starts_with(root) {
                    continue;
                }

                let Ok(file_relative) = path.strip_prefix(root) else {
                    continue;
                };
                if file_relative.as_str().is_empty() {
                    continue;
                }

                file_relative
                    .components()
                    .map(|component| component.as_str())
                    .collect()
            } else {
                if !path_starts_with_ignore_ascii_case(path, root) {
                    continue;
                }

                path.components()
                    .skip(root.components().count())
                    .map(|component| component.as_str())
                    .collect()
            };
            if file_relative_components.is_empty() {
                continue;
            }

            let mut entry_path = path
                .components()
                .take(root.components().count())
                .map(|component| component.as_str())
                .collect::<Utf8PathBuf>();
            let mut entry_relative = Utf8PathBuf::new();
            for component in file_relative_components {
                entry_path.push(component);
                entry_relative.push(component);

                if !options.hidden
                    && entry_relative.components().any(|component| {
                        component.as_str().starts_with('.') && component.as_str() != "."
                    })
                {
                    continue;
                }
                if options
                    .max_depth
                    .is_some_and(|max_depth| entry_relative.components().count() > max_depth)
                {
                    continue;
                }
                if entries
                    .iter()
                    .any(|entry: &WalkEntry| entry.path == entry_path)
                {
                    continue;
                }

                let kind = if self.is_file(&entry_path) {
                    WalkEntryKind::File
                } else if self.is_dir(&entry_path) {
                    WalkEntryKind::Directory
                } else {
                    WalkEntryKind::Other
                };
                entries.push(WalkEntry {
                    root: root.to_path_buf(),
                    path: entry_path.clone(),
                    relative: entry_relative.clone(),
                    kind,
                });
            }
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries.dedup_by(|left, right| left.path == right.path);
        RootWalk::Directory {
            entries,
            issues: Vec::new(),
        }
    }
}

/// Standard filesystem implementation that uses `std::fs` and the `ignore` crate.
#[derive(Default)]
pub struct OsFileSystem {
    case_sensitivity: OnceLock<CaseSensitivity>,
}

impl OsFileSystem {
    fn path_exists_case_sensitive_fast(path: &Utf8Path, prefix: &Utf8Path) -> Option<bool> {
        let Ok(canonicalized) = path.as_std_path().canonicalize() else {
            return Some(false);
        };
        let Ok(canonicalized) = Utf8PathBuf::from_path_buf(canonicalized) else {
            return None;
        };

        canonicalized_path_matches_requested_suffix(canonicalized.as_path(), path, prefix)
    }

    fn path_exists_case_sensitive_slow(path: &Utf8Path, prefix: &Utf8Path) -> bool {
        for ancestor in path.ancestors() {
            if ancestor == prefix {
                break;
            }

            let Some(parent) = ancestor.parent() else {
                return false;
            };
            let Some(file_name) = ancestor.file_name() else {
                return false;
            };
            let Ok(entries) = std::fs::read_dir(parent) else {
                return false;
            };

            if !entries.filter_map(Result::ok).any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name == file_name)
            }) {
                return false;
            }
        }

        true
    }
}

fn canonicalized_path_matches_requested_suffix(
    canonicalized: &Utf8Path,
    path: &Utf8Path,
    prefix: &Utf8Path,
) -> Option<bool> {
    let canonicalized = simplify_verbatim_prefix(canonicalized);
    let path = simplify_verbatim_prefix(path);
    let prefix = simplify_verbatim_prefix(prefix);

    if canonicalized.as_str().to_lowercase() != path.as_str().to_lowercase() {
        return None;
    }

    let exempt = prefix.components().count();
    Some(
        path.components()
            .skip(exempt)
            .eq(canonicalized.components().skip(exempt)),
    )
}

fn simplify_verbatim_prefix(path: &Utf8Path) -> &Utf8Path {
    if cfg!(windows) && path.as_str().starts_with(r"\\?\") {
        Utf8Path::new(&path.as_str()[r"\\?\".len()..])
    } else {
        path
    }
}

impl FileSystem for OsFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        path.exists()
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        path.is_dir()
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
            .get()
            .copied()
            .unwrap_or(CaseSensitivity::Unknown)
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        if self
            .case_sensitivity
            .get_or_init(|| {
                #[cfg(not(unix))]
                return CaseSensitivity::Unknown;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;

                    let Ok(original_case_metadata) = prefix.as_std_path().metadata() else {
                        return CaseSensitivity::Unknown;
                    };

                    let upper_case = Utf8PathBuf::from(prefix.as_str().to_uppercase());
                    if upper_case == prefix {
                        return CaseSensitivity::Unknown;
                    }

                    match upper_case.as_std_path().metadata() {
                        Ok(uppercase_metadata) => {
                            if uppercase_metadata.ino() == original_case_metadata.ino() {
                                CaseSensitivity::CaseInsensitive
                            } else {
                                CaseSensitivity::CaseSensitive
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            CaseSensitivity::CaseSensitive
                        }
                        Err(_) => CaseSensitivity::Unknown,
                    }
                }
            })
            .is_case_sensitive()
        {
            return self.exists(path);
        }

        Self::path_exists_case_sensitive_fast(path, prefix)
            .unwrap_or_else(|| Self::path_exists_case_sensitive_slow(path, prefix))
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        let metadata = match root.as_std_path().metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return RootWalk::Missing;
            }
            Err(error) => {
                return RootWalk::Inaccessible(error.kind());
            }
        };
        if metadata.is_file() {
            return RootWalk::File(WalkEntry::file_root(root));
        }
        if !metadata.is_dir() {
            return RootWalk::Missing;
        }

        let mut builder = WalkBuilder::new(root.as_std_path());
        // Call standard_filters first because it sets hidden, gitignore, etc.
        // Override individual settings after.
        builder
            .standard_filters(!options.no_ignore)
            .hidden(!options.hidden)
            .follow_links(options.follow_links)
            .max_depth(options.max_depth);

        if !options.globs.is_empty() {
            let mut overrides = OverrideBuilder::new(root.as_std_path());
            for glob in &options.globs {
                // OverrideBuilder only errors for invalid globs. Match ripgrep's
                // lenient behavior and skip invalid overrides here.
                let _ = overrides.add(glob);
            }
            if let Ok(built) = overrides.build() {
                builder.overrides(built);
            }
        }

        let mut entries = Vec::new();
        let mut issues = Vec::new();
        for result in builder.build() {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    let kind = error
                        .into_io_error()
                        .map_or(io::ErrorKind::Other, |error| error.kind());
                    issues.push(kind);
                    continue;
                }
            };
            let Some(path) = Utf8Path::from_path(entry.path()) else {
                continue;
            };
            if path == root {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let kind = entry
                .file_type()
                .map_or(WalkEntryKind::Other, WalkEntryKind::from);

            entries.push(WalkEntry {
                root: root.to_path_buf(),
                path: path.to_path_buf(),
                relative: relative.to_path_buf(),
                kind,
            });
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries.dedup_by(|left, right| left.path == right.path);
        issues.sort_by_cached_key(|kind| format!("{kind:?}"));
        issues.dedup();
        RootWalk::Directory { entries, issues }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_reads_existing_file() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file("/test.py".into(), "file content".to_string());

        assert_eq!(
            fs.read_to_string(Utf8Path::new("/test.py")).unwrap(),
            "file content"
        );
    }

    #[test]
    fn in_memory_reports_missing_file() {
        let fs = InMemoryFileSystem::new();

        let result = fs.read_to_string(Utf8Path::new("/missing.py"));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn case_insensitive_in_memory_lookups_ignore_ascii_case() {
        let mut fs = InMemoryFileSystem::case_insensitive();
        fs.add_file("/Project/pkg/module.py".into(), "content".to_string());

        assert!(fs.exists(Utf8Path::new("/project/PKG/MODULE.py")));
        assert!(fs.is_file(Utf8Path::new("/project/PKG/MODULE.py")));
        assert!(fs.is_dir(Utf8Path::new("/project/PKG")));
        assert_eq!(
            fs.read_to_string(Utf8Path::new("/project/PKG/MODULE.py"))
                .unwrap(),
            "content"
        );
    }

    #[test]
    fn case_insensitive_in_memory_path_exists_case_sensitive_verifies_suffix_casing() {
        let mut fs = InMemoryFileSystem::case_insensitive();
        fs.add_file("/project/pkg/module.py".into(), String::new());

        assert!(fs.path_exists_case_sensitive(
            Utf8Path::new("/project/pkg/module.py"),
            Utf8Path::new("/project"),
        ));
        assert!(!fs.path_exists_case_sensitive(
            Utf8Path::new("/project/pkg/Module.py"),
            Utf8Path::new("/project"),
        ));
        assert!(fs.path_exists_case_sensitive(
            Utf8Path::new("/PROJECT/pkg/module.py"),
            Utf8Path::new("/PROJECT"),
        ));
    }

    #[test]
    fn case_insensitive_in_memory_path_exists_case_sensitive_verifies_directories() {
        let mut fs = InMemoryFileSystem::case_insensitive();
        fs.add_file("/project/pkg/module.py".into(), String::new());

        assert!(fs.path_exists_case_sensitive(
            Utf8Path::new("/project/pkg"),
            Utf8Path::new("/project"),
        ));
        assert!(
            !fs.path_exists_case_sensitive(
                Utf8Path::new("/project/Pkg"),
                Utf8Path::new("/project"),
            )
        );
    }

    #[test]
    fn in_memory_walks_files_under_root() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file("/project/templates/base.html".into(), "base".to_string());
        fs.add_file(
            "/project/templates/app/page.html".into(),
            "page".to_string(),
        );
        fs.add_file("/project/other.py".into(), "other".to_string());

        let files = dir_entries(fs.walk_root(
            Utf8Path::new("/project/templates"),
            &WalkOptions::unrestricted(),
        ));
        let relatives: Vec<_> = files
            .iter()
            .filter(|entry| entry.kind == WalkEntryKind::File)
            .map(|entry| entry.relative.as_str())
            .collect();

        assert_eq!(relatives, vec!["app/page.html", "base.html"]);
    }

    #[test]
    fn case_insensitive_in_memory_walk_uses_wrong_cased_root() {
        let mut fs = InMemoryFileSystem::case_insensitive();
        fs.add_file("/Project/Templates/base.html".into(), "base".to_string());
        fs.add_file(
            "/Project/Templates/App/page.html".into(),
            "page".to_string(),
        );

        let files = dir_entries(fs.walk_root(
            Utf8Path::new("/project/templates"),
            &WalkOptions::unrestricted(),
        ));
        let paths: Vec<_> = files
            .iter()
            .filter(|entry| entry.kind == WalkEntryKind::File)
            .map(|entry| entry.path.as_str())
            .collect();
        let relatives: Vec<_> = files
            .iter()
            .filter(|entry| entry.kind == WalkEntryKind::File)
            .map(|entry| entry.relative.as_str())
            .collect();

        assert_eq!(
            paths,
            vec![
                "/Project/Templates/App/page.html",
                "/Project/Templates/base.html",
            ]
        );
        assert_eq!(relatives, vec!["App/page.html", "base.html"]);
    }

    #[test]
    fn in_memory_walk_respects_hidden_option() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file("/project/.hidden/secret.html".into(), "secret".to_string());
        fs.add_file("/project/visible.html".into(), "visible".to_string());

        let files = dir_entries(fs.walk_root(Utf8Path::new("/project"), &WalkOptions::default()));
        let relatives: Vec<_> = files
            .iter()
            .filter(|entry| entry.kind == WalkEntryKind::File)
            .map(|entry| entry.relative.as_str())
            .collect();

        assert_eq!(relatives, vec!["visible.html"]);
    }

    #[test]
    fn canonicalized_compare_ignores_prefix_casing_mismatch() {
        assert_eq!(
            canonicalized_path_matches_requested_suffix(
                Utf8Path::new("/Project/foo.py"),
                Utf8Path::new("/project/foo.py"),
                Utf8Path::new("/project"),
            ),
            Some(true)
        );
    }

    #[test]
    fn canonicalized_compare_rejects_suffix_casing_mismatch() {
        assert_eq!(
            canonicalized_path_matches_requested_suffix(
                Utf8Path::new("/Project/foo.py"),
                Utf8Path::new("/project/Foo.py"),
                Utf8Path::new("/project"),
            ),
            Some(false)
        );
    }

    #[test]
    fn canonicalized_compare_defers_non_casing_difference() {
        assert_eq!(
            canonicalized_path_matches_requested_suffix(
                Utf8Path::new("/Project/foo.py"),
                Utf8Path::new("/project/bar.py"),
                Utf8Path::new("/project"),
            ),
            None
        );
    }

    fn temp_path(dir: &tempfile::TempDir) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap()
    }

    fn entry_names(entries: &[WalkEntry]) -> Vec<&str> {
        entries
            .iter()
            .filter(|entry| entry.kind == WalkEntryKind::File)
            .filter_map(|entry| entry.path.file_name())
            .collect()
    }

    #[track_caller]
    fn dir_entries(walk: RootWalk) -> Vec<WalkEntry> {
        match walk {
            RootWalk::Directory { entries, issues } => {
                assert_eq!(issues, Vec::new());
                entries
            }
            other => panic!("expected a traversed directory, got {other:?}"),
        }
    }

    #[test]
    fn os_walk_skips_hidden_directories_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".hidden/secret.html"), "secret").unwrap();
        std::fs::write(dir.path().join("visible.html"), "visible").unwrap();

        let entries = dir_entries(
            OsFileSystem::default().walk_root(&temp_path(&dir), &WalkOptions::default()),
        );
        let names = entry_names(&entries);

        assert!(names.contains(&"visible.html"));
        assert!(!names.contains(&"secret.html"));
    }

    #[test]
    fn os_walk_can_include_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".hidden/secret.html"), "secret").unwrap();
        std::fs::write(dir.path().join("visible.html"), "visible").unwrap();

        let options = WalkOptions {
            hidden: true,
            ..WalkOptions::default()
        };
        let entries = dir_entries(OsFileSystem::default().walk_root(&temp_path(&dir), &options));
        let names = entry_names(&entries);

        assert!(names.contains(&"visible.html"));
        assert!(names.contains(&"secret.html"));
    }

    #[test]
    fn os_walk_respects_gitignore() {
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

        let entries = dir_entries(
            OsFileSystem::default().walk_root(&temp_path(&dir), &WalkOptions::default()),
        );
        let names = entry_names(&entries);

        assert!(names.contains(&"keep.html"));
        assert!(!names.contains(&"skip.html"));
    }

    #[test]
    fn os_walk_no_ignore_disables_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored/found.html"), "found").unwrap();
        std::fs::write(dir.path().join("keep.html"), "keep").unwrap();

        let options = WalkOptions {
            no_ignore: true,
            ..WalkOptions::default()
        };
        let entries = dir_entries(OsFileSystem::default().walk_root(&temp_path(&dir), &options));
        let names = entry_names(&entries);

        assert!(names.contains(&"keep.html"));
        assert!(names.contains(&"found.html"));
    }

    #[test]
    fn os_walk_applies_glob_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("page.html"), "page").unwrap();
        std::fs::write(dir.path().join("other.html"), "other").unwrap();
        std::fs::write(dir.path().join("style.css"), "style").unwrap();

        let options = WalkOptions {
            globs: vec!["page.*".to_string()],
            ..WalkOptions::default()
        };
        let entries = dir_entries(OsFileSystem::default().walk_root(&temp_path(&dir), &options));
        let names = entry_names(&entries);

        assert!(names.contains(&"page.html"));
        assert!(!names.contains(&"other.html"));
        assert!(!names.contains(&"style.css"));
    }

    #[test]
    fn os_walk_limits_max_depth() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("top.html"), "top").unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/deep.html"), "deep").unwrap();

        let options = WalkOptions {
            max_depth: Some(1),
            ..WalkOptions::default()
        };
        let entries = dir_entries(OsFileSystem::default().walk_root(&temp_path(&dir), &options));
        let names = entry_names(&entries);

        assert!(names.contains(&"top.html"));
        assert!(!names.contains(&"deep.html"));
    }

    #[cfg(unix)]
    #[test]
    fn os_detailed_walk_retains_entries_when_another_entry_errors() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("found.html"), "found").unwrap();
        symlink(dir.path(), dir.path().join("loop")).unwrap();
        let options = WalkOptions {
            follow_links: true,
            ..WalkOptions::unrestricted()
        };

        let RootWalk::Directory { entries, issues } =
            OsFileSystem::default().walk_root(&temp_path(&dir), &options)
        else {
            panic!("expected a traversed directory");
        };

        assert_eq!(entry_names(&entries), ["found.html"]);
        assert!(!issues.is_empty());
    }

    #[test]
    fn os_walk_single_file_root_uses_file_name_as_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = Utf8PathBuf::from_path_buf(dir.path().join("single.html")).unwrap();
        std::fs::write(file_path.as_std_path(), "single").unwrap();

        let RootWalk::File(entry) =
            OsFileSystem::default().walk_root(&file_path, &WalkOptions::default())
        else {
            panic!("expected a file root");
        };

        assert_eq!(entry.path, file_path);
        assert_eq!(entry.relative, Utf8PathBuf::from("single.html"));
    }
}
