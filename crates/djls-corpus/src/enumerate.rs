//! Find extraction-relevant Python files and template files in the corpus.

use std::path::Path;
use std::path::PathBuf;

use walkdir::WalkDir;

/// Find all Python files relevant to extraction in the corpus.
///
/// Matches:
/// - `**/templatetags/**/*.py` (excluding `__init__.py` and `__pycache__`)
/// - `**/template/defaulttags.py`, `defaultfilters.py`, `loader_tags.py`
#[must_use]
pub fn enumerate_extraction_files(corpus_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for entry in WalkDir::new(corpus_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let path_str = path.to_string_lossy();

        // Skip __pycache__
        if path_str.contains("__pycache__") {
            continue;
        }

        // Must be a .py file
        if path.extension().is_none_or(|ext| ext != "py") {
            continue;
        }

        // Skip __init__.py — rarely contains registrations
        if path.file_name().is_some_and(|n| n == "__init__.py") {
            continue;
        }

        // Pattern 1: **/templatetags/**/*.py
        if path_str.contains("/templatetags/") {
            files.push(path.to_path_buf());
            continue;
        }

        // Pattern 2: Django core template modules
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path_str.contains("/template/")
            && matches!(
                file_name,
                "defaulttags.py" | "defaultfilters.py" | "loader_tags.py"
            )
        {
            files.push(path.to_path_buf());
        }
    }

    files.sort();
    files.dedup();
    files
}

/// Find all Django template files (`.html`, `.txt`) in a directory tree.
///
/// Matches files inside `**/templates/` directories.
#[must_use]
pub fn enumerate_template_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let path_str = path.to_string_lossy();

        // Must be inside a templates/ directory
        if !path_str.contains("/templates/") {
            continue;
        }

        // Must be .html or .txt
        let is_template = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("txt"));

        if is_template {
            files.push(path.to_path_buf());
        }
    }

    files.sort();
    files.dedup();
    files
}
