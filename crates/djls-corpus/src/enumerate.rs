//! Find extraction-relevant Python files and template files in the corpus.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use walkdir::WalkDir;

/// What kind of corpus files to enumerate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusFileKind {
    /// Python modules containing templatetag/filter registrations.
    ///
    /// Matches `**/templatetags/**/*.py` (excluding `__init__.py`)
    /// and `**/template/{defaulttags,defaultfilters,loader_tags}.py`.
    ExtractionTarget,

    /// Django template files (`.html`, `.txt`) inside `**/templates/`.
    ///
    /// Excludes files inside `docs/`, `tests/`, `jinja2/`, and `static/`
    /// directories. Jinja2 templates use different syntax, and `static/`
    /// directories may contain `AngularJS` or other non-Django templates.
    Template,
}

/// Enumerate corpus files of a given kind under a directory.
#[must_use]
pub fn enumerate_files(root: &Utf8Path, kind: CorpusFileKind) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();

    for entry in WalkDir::new(root.as_std_path())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let Some(path) = Utf8Path::from_path(entry.path()) else {
            continue;
        };
        let path_str = path.as_str();

        if in_pycache(path_str) {
            continue;
        }

        let matches = match kind {
            CorpusFileKind::ExtractionTarget => {
                has_py_extension(path)
                    && path.file_name() != Some("__init__.py")
                    && (in_templatetags_dir(path_str) || is_core_template_module(path))
            }
            CorpusFileKind::Template => {
                has_template_extension(path)
                    && in_templates_dir(path_str)
                    && !path_str.contains("/docs/")
                    && !path_str.contains("/tests/")
                    && !path_str.contains("/jinja2/")
                    && !path_str.contains("/static/")
            }
        };

        if matches {
            files.push(path.to_owned());
        }
    }

    files.sort();
    files
}

pub(crate) fn in_pycache(path_str: &str) -> bool {
    path_str.contains("__pycache__")
}

pub(crate) fn has_py_extension(path: &Utf8Path) -> bool {
    path.extension().is_some_and(|ext| ext == "py")
}

pub(crate) fn has_template_extension(path: &Utf8Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("txt"))
}

pub(crate) fn in_templatetags_dir(path_str: &str) -> bool {
    path_str.contains("/templatetags/")
}

pub(crate) fn is_core_template_module(path: &Utf8Path) -> bool {
    let path_str = path.as_str();
    path_str.contains("/template/")
        && matches!(
            path.file_name(),
            Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
        )
}

pub(crate) fn in_templates_dir(path_str: &str) -> bool {
    path_str.contains("/templates/")
}
