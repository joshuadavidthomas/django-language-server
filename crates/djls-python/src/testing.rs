/// Test utilities for corpus-grounded extraction tests.
///
/// These helpers load Python source from the corpus and extract specific
/// functions for targeted unit testing. The corpus is required — helpers
/// panic with a helpful message if it has not been synced.
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
/// Returns `None` if the file doesn't exist.
///
/// # Panics
///
/// Panics if the corpus has not been synced.
///
/// # Examples
///
/// ```ignore
/// let source = corpus_source("packages/Django/6.0.2/django/template/defaulttags.py");
/// ```
#[must_use]
pub fn corpus_source(relative_path: &str) -> Option<String> {
    let corpus = Corpus::require();
    let path = corpus.root().join(relative_path);
    std::fs::read_to_string(path.as_std_path()).ok()
}

/// Load the full source from the latest version of a package in the corpus.
///
/// `package` is the directory name under `packages/` (e.g. `"django-allauth"`,
/// `"wagtail"`). `relative_to_package` is the path within the versioned
/// directory (e.g. `"allauth/templatetags/allauth.py"`).
///
/// # Panics
///
/// Panics if the corpus has not been synced.
///
/// # Examples
///
/// ```ignore
/// let source = package_source("django-allauth", "allauth/templatetags/allauth.py");
/// ```
#[must_use]
pub fn package_source(package: &str, relative_to_package: &str) -> Option<String> {
    let corpus = Corpus::require();
    let pkg_dir = corpus.latest_package(package)?;
    let full_path = pkg_dir.join(relative_to_package);
    if full_path.as_std_path().exists() {
        let rel = full_path.strip_prefix(corpus.root()).ok()?.to_string();
        corpus_source(&rel)
    } else {
        None
    }
}

/// Load a function from the latest version of a package in the corpus.
///
/// `package` is the directory name under `packages/` (e.g. `"django"`,
/// `"wagtail"`). `relative_to_package` is the file path within the
/// versioned directory. `func_name` is the function to find.
///
/// # Panics
///
/// Panics if the corpus has not been synced.
#[must_use]
pub fn package_function(
    package: &str,
    relative_to_package: &str,
    func_name: &str,
) -> Option<StmtFunctionDef> {
    let source = package_source(package, relative_to_package)?;
    find_function_in_source(&source, func_name)
}

/// Convenience wrapper: load a function from the latest Django version.
#[must_use]
pub fn django_function(relative_to_django: &str, func_name: &str) -> Option<StmtFunctionDef> {
    package_function("django", relative_to_django, func_name)
}

/// Convenience wrapper: load source from the latest Django version.
#[must_use]
pub fn django_source(relative_to_django: &str) -> Option<String> {
    package_source("django", relative_to_django)
}
