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
//! let django = corpus.latest_django().expect("no Django in corpus");
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

pub mod archive;
pub mod bump;
pub mod lockfile;
pub mod manifest;
pub mod sync;

const CORPUS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.corpus");

/// A validated corpus root directory.
///
/// Constructed via [`Corpus::discover`], which validates that the
/// directory exists. Once constructed, the root path is trusted for
/// the lifetime of the value.
pub struct Corpus;

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
        Self
    }

    /// The corpus root directory.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        Utf8Path::new(CORPUS_DIR)
    }

    /// Latest synced Django version directory.
    #[must_use]
    pub fn latest_django(&self) -> Option<Utf8PathBuf> {
        let django_dir = self.root().join("packages/Django");
        if !django_dir.as_std_path().exists() {
            return None;
        }

        let children = synced_children(&django_dir);

        let mut best: Option<(Vec<u32>, Utf8PathBuf)> = None;
        for child in &children {
            let Some(name) = child.file_name() else {
                continue;
            };

            // Parse "5.2.11" → [5, 2, 11] for numeric comparison
            let version: Option<Vec<u32>> = name
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
                best = Some((version, child.clone()));
            }
        }

        if let Some((_, path)) = best {
            return Some(path);
        }

        children.into_iter().last()
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
/// Handles both corpus layout patterns:
/// - Packages: `.corpus/packages/{name}/{version}/{python_code}...`
/// - Repos: `.corpus/repos/{name}/{ref}/{python_code}...`
///
/// Falls back to a heuristic for non-corpus paths (looks for version-like
/// path components).
///
/// # Examples
///
/// ```
/// # use camino::Utf8Path;
/// # use djls_corpus::module_path_from_file;
/// let path = Utf8Path::new(".corpus/packages/Django/6.0.2/django/template/defaulttags.py");
/// assert_eq!(module_path_from_file(path), "django.template.defaulttags");
///
/// let path = Utf8Path::new(".corpus/repos/sentry/abc123/sentry/templatetags/sentry_helpers.py");
/// assert_eq!(module_path_from_file(path), "sentry.templatetags.sentry_helpers");
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

    // Look for corpus structure markers: "packages" or "repos" followed by
    // {name}/{version_or_ref}/{python_code}... — skip the first 3 after marker.
    let start = if let Some(pos) = components
        .iter()
        .position(|c| *c == "packages" || *c == "repos")
    {
        pos + 3
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
