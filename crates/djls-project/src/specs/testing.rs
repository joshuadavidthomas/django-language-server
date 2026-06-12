//! Vendored fixtures for in-crate extraction unit tests.
//!
//! These snippets are copied from pinned corpus entries and kept intentionally
//! small. Live corpus drift is covered by the integration snapshots in
//! `crates/djls-project/tests/corpus*.rs`.

use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_parser::parse_module;

pub(crate) const ALLAUTH_SOURCE: &str = include_str!("testdata/allauth_tags.py");
pub(crate) const ADMIN_URLS_SOURCE: &str = include_str!("testdata/django_admin_urls.py");
pub(crate) const CUSTOM_SOURCE: &str = include_str!("testdata/django_custom.py");
pub(crate) const DEFAULTFILTERS_SOURCE: &str = include_str!("testdata/django_defaultfilters.py");
pub(crate) const DEFAULTTAGS_SOURCE: &str = include_str!("testdata/django_defaulttags.py");
pub(crate) const I18N_SOURCE: &str = include_str!("testdata/django_i18n.py");
pub(crate) const INCLUSION_SOURCE: &str = include_str!("testdata/django_inclusion.py");
pub(crate) const LOADER_TAGS_SOURCE: &str = include_str!("testdata/django_loader_tags.py");
pub(crate) const TESTTAGS_SOURCE: &str = include_str!("testdata/django_testtags.py");
pub(crate) const TZ_SOURCE: &str = include_str!("testdata/django_tz.py");
pub(crate) const WAGTAILADMIN_TAGS_SOURCE: &str = include_str!("testdata/wagtailadmin_tags.py");

/// Find a function definition by name in Python source.
#[must_use]
pub(crate) fn find_function_in_source(source: &str, func_name: &str) -> Option<StmtFunctionDef> {
    let parsed = parse_module(source).ok()?;
    let module = parsed.into_syntax();
    for stmt in module.body {
        if let Stmt::FunctionDef(func_def) = stmt
            && func_def.name.as_str() == func_name
        {
            return Some(func_def);
        }
    }
    None
}

/// Load a function from the vendored Django fixture matching the corpus path.
#[must_use]
pub(crate) fn django_function(
    relative_to_django: &str,
    func_name: &str,
) -> Option<StmtFunctionDef> {
    let source = match relative_to_django {
        "django/contrib/admin/templatetags/admin_urls.py" => ADMIN_URLS_SOURCE,
        "django/template/defaultfilters.py" => DEFAULTFILTERS_SOURCE,
        "django/template/defaulttags.py" => DEFAULTTAGS_SOURCE,
        "django/template/loader_tags.py" => LOADER_TAGS_SOURCE,
        "django/templatetags/i18n.py" => I18N_SOURCE,
        "django/templatetags/tz.py" => TZ_SOURCE,
        "tests/template_tests/templatetags/custom.py" => CUSTOM_SOURCE,
        "tests/template_tests/templatetags/inclusion.py" => INCLUSION_SOURCE,
        "tests/template_tests/templatetags/testtags.py" => TESTTAGS_SOURCE,
        _ => return None,
    };
    find_function_in_source(source, func_name)
}
