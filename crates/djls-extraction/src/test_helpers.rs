/// Test utilities for corpus-grounded extraction tests.
///
/// These helpers load Python source from the corpus and extract specific
/// functions for targeted unit testing. All corpus-dependent helpers return
/// `Option` and skip gracefully when the corpus is not synced.
use djls_corpus::Corpus;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_parser::parse_module;

/// Find a function definition by name in Python source.
///
/// Parses the source with Ruff and walks top-level statements to find
/// a `def` with the given name. Returns `None` if not found.
///
/// # Examples
///
/// ```ignore
/// let source = "def foo(): pass\ndef bar(): pass";
/// let func = find_function_in_source(source, "bar").unwrap();
/// assert_eq!(func.name.as_str(), "bar");
/// ```
#[must_use]
pub fn find_function_in_source(source: &str, func_name: &str) -> Option<StmtFunctionDef> {
    let parsed = parse_module(source).ok()?;
    let module = parsed.into_syntax();
    for stmt in module.body {
        if let Stmt::FunctionDef(func_def) = stmt {
            if func_def.name.as_str() == func_name {
                return Some(func_def);
            }
        }
    }
    None
}

/// Load the full source of a corpus file by path relative to the corpus root.
///
/// Returns `None` if the corpus is not synced or the file doesn't exist.
///
/// # Examples
///
/// ```ignore
/// let source = corpus_source("packages/Django/6.0.2/django/template/defaulttags.py");
/// ```
#[must_use]
pub fn corpus_source(relative_path: &str) -> Option<String> {
    let corpus = Corpus::discover()?;
    let path = corpus.root().join(relative_path);
    std::fs::read_to_string(path.as_std_path()).ok()
}

/// Load a specific function from a corpus file.
///
/// Combines [`corpus_source`] and [`find_function_in_source`] — loads the
/// file from the corpus and finds the named function. Returns `None` if:
/// - The corpus is not synced
/// - The file doesn't exist
/// - The function is not found in the file
///
/// # Examples
///
/// ```ignore
/// let func = corpus_function(
///     "packages/Django/6.0.2/django/template/defaulttags.py",
///     "do_for",
/// );
/// ```
#[must_use]
pub fn corpus_function(relative_path: &str, func_name: &str) -> Option<StmtFunctionDef> {
    let source = corpus_source(relative_path)?;
    find_function_in_source(&source, func_name)
}

/// Resolve a corpus path for the latest Django version.
///
/// Given a path relative to the Django package root (e.g.,
/// `"django/template/defaulttags.py"`), returns the full corpus-relative
/// path using the latest synced Django version. Returns `None` if the
/// corpus is not synced or no Django version is available.
///
/// # Examples
///
/// ```ignore
/// let path = latest_django_path("django/template/defaulttags.py");
/// // Returns something like "packages/Django/6.0.2/django/template/defaulttags.py"
/// ```
#[must_use]
pub fn latest_django_path(relative_to_django: &str) -> Option<String> {
    let corpus = Corpus::discover()?;
    let django_dir = corpus.latest_django()?;
    let full_path = django_dir.join(relative_to_django);
    if full_path.as_std_path().exists() {
        Some(
            full_path
                .strip_prefix(corpus.root())
                .ok()?
                .to_string(),
        )
    } else {
        None
    }
}

/// Load a function from the latest Django version in the corpus.
///
/// Convenience wrapper that combines [`latest_django_path`] and
/// [`corpus_function`].
///
/// # Examples
///
/// ```ignore
/// let func = django_function("django/template/defaulttags.py", "do_for");
/// ```
#[must_use]
pub fn django_function(
    relative_to_django: &str,
    func_name: &str,
) -> Option<StmtFunctionDef> {
    let path = latest_django_path(relative_to_django)?;
    corpus_function(&path, func_name)
}

/// Load the full source from the latest Django version in the corpus.
///
/// # Examples
///
/// ```ignore
/// let source = django_source("django/template/defaulttags.py");
/// ```
#[must_use]
pub fn django_source(relative_to_django: &str) -> Option<String> {
    let path = latest_django_path(relative_to_django)?;
    corpus_source(&path)
}
