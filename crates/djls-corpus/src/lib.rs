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
//! let corpus = Corpus::discover().expect("corpus not synced");
//! let django = corpus.latest_django().expect("no Django in corpus");
//! ```
//!
//! # Consumers
//!
//! - `djls-extraction` — golden tests: extract rules from real templatetag
//!   modules and snapshot the results
//! - `djls-server` — integration tests: parse real templates, validate
//!   against extracted rules, assert zero false positives

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;

pub use crate::enumerate::FileKind;

pub mod enumerate;
pub mod manifest;
pub mod sync;

/// A validated corpus root directory.
///
/// Constructed via [`Corpus::discover`] or [`Corpus::from_path`], which
/// validate that the directory exists. Once constructed, the root path
/// is trusted for the lifetime of the value.
pub struct Corpus {
    root: Utf8PathBuf,
}

impl Corpus {
    /// Discover corpus from `DJLS_CORPUS_ROOT` env var or default location.
    ///
    /// Returns `None` if no corpus directory exists.
    #[must_use]
    pub fn discover() -> Option<Self> {
        if let Ok(path) = std::env::var("DJLS_CORPUS_ROOT") {
            let p = Utf8PathBuf::from(path);
            if p.as_std_path().exists() {
                return Some(Self { root: p });
            }
        }

        let workspace_root = Utf8Path::new(env!("CARGO_WORKSPACE_DIR"));
        let default = workspace_root.join("crates/djls-corpus/.corpus");
        if default.as_std_path().exists() {
            return Some(Self { root: default });
        }

        None
    }

    /// Construct from a known path, validating it exists.
    #[must_use]
    pub fn from_path(root: Utf8PathBuf) -> Option<Self> {
        if root.as_std_path().exists() {
            Some(Self { root })
        } else {
            None
        }
    }

    /// The validated corpus root directory.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// Latest synced Django version directory.
    #[must_use]
    pub fn latest_django(&self) -> Option<Utf8PathBuf> {
        let django_dir = self.root.join("packages/Django");
        if !django_dir.as_std_path().exists() {
            return None;
        }
        synced_children(&django_dir).into_iter().last()
    }

    /// Synced subdirectories under a path relative to the corpus root.
    ///
    /// Only returns directories containing a `.complete` marker.
    #[must_use]
    pub fn synced_dirs(&self, relative: &str) -> Vec<Utf8PathBuf> {
        synced_children(&self.root.join(relative))
    }

    /// All extraction target files in the entire corpus.
    #[must_use]
    pub fn extraction_targets(&self) -> Vec<Utf8PathBuf> {
        enumerate::enumerate_files(&self.root, FileKind::ExtractionTarget)
    }

    /// All template files in the entire corpus.
    #[must_use]
    pub fn templates(&self) -> Vec<Utf8PathBuf> {
        enumerate::enumerate_files(&self.root, FileKind::Template)
    }

    /// Enumerate files of a given kind under a specific directory.
    #[must_use]
    pub fn enumerate_files(&self, dir: &Utf8Path, kind: FileKind) -> Vec<Utf8PathBuf> {
        enumerate::enumerate_files(dir, kind)
    }

    /// Extract rules from a single Python file in the corpus.
    ///
    /// Reads the file, derives its module path from the corpus layout,
    /// and runs extraction. Returns `None` if the file cannot be read.
    #[cfg(feature = "extraction")]
    #[must_use]
    pub fn extract_file(&self, path: &Utf8Path) -> Option<djls_extraction::ExtractionResult> {
        let source = std::fs::read_to_string(path.as_std_path()).ok()?;
        let module_path = module_path_from_file(path);
        Some(djls_extraction::extract_rules(&source, &module_path))
    }

    /// Extract and merge all extraction targets under a directory.
    #[cfg(feature = "extraction")]
    #[must_use]
    pub fn extract_dir(&self, dir: &Utf8Path) -> djls_extraction::ExtractionResult {
        let files = enumerate::enumerate_files(dir, FileKind::ExtractionTarget);
        let mut combined = djls_extraction::ExtractionResult::default();
        for path in &files {
            if let Some(result) = self.extract_file(path) {
                combined.merge(result);
            }
        }
        combined
    }
}

/// Collect subdirectories that have been fully synced (contain a `.complete` marker).
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
        .filter(|p| p.join(".complete").as_std_path().exists())
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

    components[start..]
        .iter()
        .map(|s| s.strip_suffix(".py").unwrap_or(s))
        .collect::<Vec<_>>()
        .join(".")
}
