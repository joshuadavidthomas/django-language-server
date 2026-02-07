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
//! # Consumers
//!
//! - `djls-extraction` — golden tests: extract rules from real templatetag
//!   modules and snapshot the results
//! - `djls-server` — integration tests: parse real templates, validate
//!   against extracted rules, assert zero false positives

use std::path::Path;
use std::path::PathBuf;

pub mod enumerate;
pub mod manifest;
pub mod sync;

/// Get corpus root from environment, or use default if it exists.
///
/// Checks `DJLS_CORPUS_ROOT` env var first, then falls back to the
/// default location at `crates/djls-corpus/.corpus/` relative to the
/// workspace root (derived from `CARGO_MANIFEST_DIR` of the calling crate).
#[must_use]
pub fn find_corpus_root(calling_crate_manifest_dir: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("DJLS_CORPUS_ROOT") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try default location relative to workspace root.
    // calling_crate_manifest_dir is e.g. "/.../crates/djls-extraction"
    let workspace_root = PathBuf::from(calling_crate_manifest_dir)
        .parent()
        .and_then(|p| p.parent())?
        .to_path_buf();
    let default = workspace_root.join("crates/djls-corpus/.corpus");
    if default.exists() {
        return Some(default);
    }

    None
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
/// # use std::path::Path;
/// # use djls_corpus::module_path_from_file;
/// let path = Path::new(".corpus/packages/Django/6.0.2/django/template/defaulttags.py");
/// assert_eq!(module_path_from_file(path), "django.template.defaulttags");
///
/// let path = Path::new(".corpus/repos/sentry/abc123/sentry/templatetags/sentry_helpers.py");
/// assert_eq!(module_path_from_file(path), "sentry.templatetags.sentry_helpers");
/// ```
#[must_use]
pub fn module_path_from_file(file: &Path) -> String {
    let components: Vec<&str> = file
        .components()
        .filter_map(|c| {
            if let std::path::Component::Normal(s) = c {
                s.to_str()
            } else {
                None
            }
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
                && !Path::new(component)
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
