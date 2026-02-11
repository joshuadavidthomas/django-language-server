//! Corpus of real-world Django projects for grounding tests in reality.
//!
//! This crate is the **single source of truth** for real Python source code
//! and Django templates used across the test suite. It syncs pinned versions
//! of Django, popular third-party packages, and open-source project repos,
//! then provides helpers to enumerate and locate files within them.
//!
//! **All tests that analyze Python source (extraction rules, registrations,
//! filter arities, block specs) should use corpus files, not fabricated
//! snippets.** Template parser tests may use synthetic templates since
//! that's what users type, but extraction tests must be grounded in code
//! that real projects actually ship.
//!
//! # Usage
//!
//! ```no_run
//! use djls_corpus::Corpus;
//!
//! let corpus = Corpus::require();
//! let django = corpus.latest_package("django").expect("no Django in corpus");
//! ```
//!
//! # Consumers
//!
//! - `djls-python` — golden tests: extract rules from real templatetag
//!   modules and snapshot the results
//! - `djls-server` — integration tests: parse real templates, validate
//!   against extracted rules, assert zero false positives

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use walkdir::WalkDir;

pub mod add;
pub(crate) mod archive;
pub mod lock;
pub mod manifest;
pub mod sync;

const CORPUS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.corpus");

/// A validated corpus root directory.
///
/// Constructed via [`Corpus::require`], which validates that the
/// directory exists. Once constructed, the root path is trusted for
/// the lifetime of the value.
pub struct Corpus {
    _private: (),
}

impl Corpus {
    /// Get the corpus, panicking with a helpful message if not synced.
    ///
    /// # Panics
    ///
    /// Panics if the corpus has not been synced.
    #[must_use]
    pub fn require() -> Self {
        assert!(
            Utf8Path::new(CORPUS_DIR).as_std_path().exists(),
            "Corpus not synced. Run: cargo run --bin djls-corpus -- sync",
        );
        Self { _private: () }
    }

    /// The corpus root directory.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        Utf8Path::new(CORPUS_DIR)
    }

    /// Derive the corpus entry directory for a path under the corpus root.
    ///
    /// Corpus entries are direct children of `packages/` or `repos/`.
    /// For example:
    /// - `{root}/packages/django-6.0/...` -> `{root}/packages/django-6.0`
    /// - `{root}/repos/sentry/...` -> `{root}/repos/sentry`
    #[must_use]
    pub fn entry_dir_for_path(&self, path: &Utf8Path) -> Option<Utf8PathBuf> {
        let relative = path.strip_prefix(self.root()).ok()?;

        let mut components = relative.components();
        let category = components.next()?;
        let entry = components.next()?;

        match category.as_str() {
            "packages" | "repos" => Some(self.root().join(category.as_str()).join(entry.as_str())),
            _ => None,
        }
    }

    /// Whether an entry directory represents a Django package.
    ///
    /// True for:
    /// - `packages/django`
    /// - `packages/django-<version>`
    #[must_use]
    pub fn is_django_entry(&self, entry_dir: &Utf8Path) -> bool {
        let Some(entry_name) = entry_dir.file_name() else {
            return false;
        };

        let is_packages = entry_dir
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|cat| cat == "packages");

        if !is_packages {
            return false;
        }

        entry_name == "django"
            || (entry_name.starts_with("django-")
                && entry_name["django-".len()..].starts_with(|c: char| c.is_ascii_digit()))
    }

    /// Latest synced version directory for a package under `packages/`.
    ///
    /// Handles the flat corpus layout where single-version packages are
    /// stored as `packages/{name}/` and multi-version packages as
    /// `packages/{name}-{version}/`.
    #[must_use]
    pub fn latest_package(&self, name: &str) -> Option<Utf8PathBuf> {
        let packages_dir = self.root().join("packages");

        // Single-version: packages/{name}/
        let exact = packages_dir.join(name);
        if exact.join(".complete.json").as_std_path().exists() {
            return Some(exact);
        }

        // Multi-version: packages/{name}-{version}/ — find highest version
        let prefix = format!("{name}-");
        let Ok(entries) = std::fs::read_dir(packages_dir.as_std_path()) else {
            return None;
        };

        let mut best: Option<(Vec<u32>, Utf8PathBuf)> = None;
        for entry in entries.filter_map(Result::ok) {
            let Some(dir_name) = entry.file_name().to_str().map(String::from) else {
                continue;
            };
            if !dir_name.starts_with(&prefix) {
                continue;
            }

            let version_str = &dir_name[prefix.len()..];
            // The suffix after "{name}-" must start with a digit to be a
            // version, otherwise it's a different package (e.g. "django-cms"
            // should not match prefix "django-").
            if !version_str.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }

            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };
            if !path.join(".complete.json").as_std_path().exists() {
                continue;
            }
            let version: Option<Vec<u32>> = version_str
                .split('.')
                .map(|part| {
                    if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
                        return None;
                    }
                    part.parse::<u32>().ok()
                })
                .collect();

            let Some(version) = version else {
                continue;
            };

            let should_replace = match &best {
                None => true,
                Some((best_version, _)) => version > *best_version,
            };

            if should_replace {
                best = Some((version, path));
            }
        }

        best.map(|(_, path)| path)
    }

    /// All synced directories for a package (handles both single and multi-version).
    ///
    /// Returns a sorted list of directories. For multi-version packages like
    /// django, returns `[packages/django-4.2, packages/django-5.2, packages/django-6.0]`.
    /// For single-version packages, returns a single-element list.
    #[must_use]
    pub fn package_dirs(&self, name: &str) -> Vec<Utf8PathBuf> {
        let packages_dir = self.root().join("packages");

        // Single-version: packages/{name}/
        let exact = packages_dir.join(name);
        if exact.join(".complete.json").as_std_path().exists() {
            return vec![exact];
        }

        // Multi-version: packages/{name}-{version}/
        let prefix = format!("{name}-");
        let Ok(entries) = std::fs::read_dir(packages_dir.as_std_path()) else {
            return Vec::new();
        };

        let mut dirs: Vec<Utf8PathBuf> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let dir_name = e.file_name().to_str()?.to_string();
                if !dir_name.starts_with(&prefix) {
                    return None;
                }
                let suffix = &dir_name[prefix.len()..];
                if !suffix.starts_with(|c: char| c.is_ascii_digit()) {
                    return None;
                }
                let path = Utf8PathBuf::from_path_buf(e.path()).ok()?;
                if path.join(".complete.json").as_std_path().exists() {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();

        dirs.sort();
        dirs
    }

    /// Synced subdirectories under a path relative to the corpus root.
    ///
    /// Only returns directories containing a `.complete.json` marker.
    #[must_use]
    pub fn synced_dirs(&self, relative: &str) -> Vec<Utf8PathBuf> {
        synced_children(&self.root().join(relative))
    }

    /// All extraction target files in the entire corpus.
    #[must_use]
    pub fn extraction_targets(&self) -> Vec<Utf8PathBuf> {
        self.extraction_targets_in(self.root())
    }

    /// Extraction target files under a specific directory.
    ///
    /// Matches `**/templatetags/**/*.py` (excluding `__init__.py`)
    /// and `**/template/{defaulttags,defaultfilters,loader_tags}.py`.
    #[must_use]
    pub fn extraction_targets_in(&self, dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut files = Vec::new();

        for entry in WalkDir::new(dir.as_std_path())
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let Some(path) = Utf8Path::from_path(entry.path()) else {
                continue;
            };
            let path_str = path.as_str();

            if path_str.contains("__pycache__") {
                continue;
            }

            let is_py = path.extension().is_some_and(|ext| ext == "py");
            let is_core_template_module = path_str.contains("/template/")
                && matches!(
                    path.file_name(),
                    Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
                );

            if is_py
                && path.file_name() != Some("__init__.py")
                && (path_str.contains("/templatetags/") || is_core_template_module)
            {
                files.push(path.to_owned());
            }
        }

        files.sort();
        files
    }

    /// Template files under a specific directory.
    ///
    /// Matches any file under a `templates/` directory. Excludes files
    /// inside `docs/`, `tests/`, `jinja2/`, and `static/` directories.
    #[must_use]
    pub fn templates_in(&self, dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut files = Vec::new();

        for entry in WalkDir::new(dir.as_std_path())
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let Some(path) = Utf8Path::from_path(entry.path()) else {
                continue;
            };
            let path_str = path.as_str();

            if path_str.contains("/templates/")
                && !path_str.contains("__pycache__")
                && !path_str.contains("/docs/")
                && !path_str.contains("/tests/")
                && !path_str.contains("/jinja2/")
                && !path_str.contains("/static/")
            {
                files.push(path.to_owned());
            }
        }

        files.sort();
        files
    }
}

/// Collect subdirectories that have been fully synced (contain a `.complete.json` marker).
///
/// Returns sorted paths for deterministic iteration.
fn synced_children(parent: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent.as_std_path()) else {
        return Vec::new();
    };

    let mut dirs: Vec<Utf8PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| p.join(".complete.json").as_std_path().exists())
        .collect();

    dirs.sort();
    dirs
}

/// Derive a dotted Python module path from a file path within the corpus.
///
/// Handles the flat corpus layout:
/// - Packages: `.corpus/packages/{name_or_name-version}/{python_code}...`
/// - Repos: `.corpus/repos/{name}/{python_code}...`
///
/// Falls back to a heuristic for non-corpus paths (looks for version-like
/// path components).
///
/// # Examples
///
/// ```
/// # use camino::Utf8Path;
/// # use djls_corpus::module_path_from_file;
/// let path = Utf8Path::new(".corpus/packages/django-6.0.2/django/template/defaulttags.py");
/// assert_eq!(module_path_from_file(path), "django.template.defaulttags");
///
/// let path = Utf8Path::new(".corpus/repos/sentry/sentry/templatetags/sentry_helpers.py");
/// assert_eq!(module_path_from_file(path), "sentry.templatetags.sentry_helpers");
///
/// let path = Utf8Path::new(".corpus/packages/django-allauth/allauth/templatetags/allauth.py");
/// assert_eq!(module_path_from_file(path), "allauth.templatetags.allauth");
/// ```
#[must_use]
pub fn module_path_from_file(file: &Utf8Path) -> String {
    let components: Vec<&str> = file
        .components()
        .filter_map(|c| match c {
            Utf8Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    // Flat layout: "packages" or "repos" followed by {dir_name}/{python_code}...
    // Skip 2 components after marker (the marker itself + the directory name).
    let start = if let Some(pos) = components
        .iter()
        .position(|c| *c == "packages" || *c == "repos")
    {
        pos + 2
    } else {
        // Fallback for non-corpus paths: look for version-like directory
        // (starts with digit, contains '.', not a .py file)
        let mut fallback = 0;
        for (i, component) in components.iter().enumerate() {
            if component.chars().next().is_some_and(|c| c.is_ascii_digit())
                && component.contains('.')
                && !Utf8Path::new(component)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
            {
                fallback = i + 1;
            }
        }
        fallback
    };

    let slice: &[&str] = components.get(start..).unwrap_or(&[]);

    slice
        .iter()
        .map(|s| s.strip_suffix(".py").unwrap_or(s))
        .collect::<Vec<_>>()
        .join(".")
}
