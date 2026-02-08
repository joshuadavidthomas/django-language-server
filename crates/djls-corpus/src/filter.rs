//! File relevance predicates for the corpus.
//!
//! Centralizes the domain knowledge of "what files does this corpus care
//! about" so both sync (tarball extraction) and discovery (file enumeration)
//! use the same definitions.

use camino::Utf8Path;

/// Whether a path is relevant for corpus download (broad filter).
///
/// Used during tarball extraction to decide what to keep. This is the
/// union of all extraction-target and template predicates. Discovery
/// methods in [`super::Corpus`] apply stricter filtering on top (e.g.
/// excluding `__init__.py`, `docs/`, `tests/`).
#[must_use]
pub fn is_download_relevant(path: &str) -> bool {
    if path.contains("__pycache__") {
        return false;
    }

    let utf8 = Utf8Path::new(path);

    if utf8.extension().is_some_and(|ext| ext == "py") {
        return path.contains("/templatetags/")
            || (path.contains("/template/")
                && matches!(
                    utf8.file_name(),
                    Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
                ));
    }

    if utf8
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("txt"))
    {
        return path.contains("/templates/");
    }

    false
}

/// Whether a path is one of Django's core template modules.
///
/// These live at `django/template/{defaulttags,defaultfilters,loader_tags}.py`
/// and are extraction targets even though they're outside `templatetags/`.
#[must_use]
pub fn is_core_template_module(path: &Utf8Path) -> bool {
    path.as_str().contains("/template/")
        && matches!(
            path.file_name(),
            Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
        )
}
