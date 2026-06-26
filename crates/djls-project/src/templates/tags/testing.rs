//! Vendored fixtures for in-crate extraction unit tests.
//!
//! These snippets are copied from pinned corpus entries and kept intentionally
//! small. Live corpus drift is covered by the integration snapshots in
//! `crates/djls-project/tests/corpus*.rs`.

use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_parser::parse_module;

const ALLAUTH_SOURCE: &str = include_str!("testdata/allauth_tags.py");
const ADMIN_URLS_SOURCE: &str = include_str!("testdata/django_admin_urls.py");
const CUSTOM_SOURCE: &str = include_str!("testdata/django_custom.py");
const DEFAULTFILTERS_SOURCE: &str = include_str!("testdata/django_defaultfilters.py");
const DEFAULTTAGS_SOURCE: &str = include_str!("testdata/django_defaulttags.py");
const I18N_SOURCE: &str = include_str!("testdata/django_i18n.py");
const INCLUSION_SOURCE: &str = include_str!("testdata/django_inclusion.py");
const LOADER_TAGS_SOURCE: &str = include_str!("testdata/django_loader_tags.py");
const TESTTAGS_SOURCE: &str = include_str!("testdata/django_testtags.py");
const TZ_SOURCE: &str = include_str!("testdata/django_tz.py");
const WAGTAILADMIN_TAGS_SOURCE: &str = include_str!("testdata/wagtailadmin_tags.py");

/// Load vendored fixture source by corpus-relative path.
#[must_use]
pub(crate) fn fixture_source(relative_path: &str) -> Option<&'static str> {
    match relative_path {
        "allauth/templatetags/allauth.py" => Some(ALLAUTH_SOURCE),
        "django/contrib/admin/templatetags/admin_urls.py" => Some(ADMIN_URLS_SOURCE),
        "django/template/defaultfilters.py" => Some(DEFAULTFILTERS_SOURCE),
        "django/template/defaulttags.py" => Some(DEFAULTTAGS_SOURCE),
        "django/template/loader_tags.py" => Some(LOADER_TAGS_SOURCE),
        "django/templatetags/i18n.py" => Some(I18N_SOURCE),
        "django/templatetags/tz.py" => Some(TZ_SOURCE),
        "tests/template_tests/templatetags/custom.py" => Some(CUSTOM_SOURCE),
        "tests/template_tests/templatetags/inclusion.py" => Some(INCLUSION_SOURCE),
        "tests/template_tests/templatetags/testtags.py" => Some(TESTTAGS_SOURCE),
        "wagtail/admin/templatetags/wagtailadmin_tags.py" => Some(WAGTAILADMIN_TAGS_SOURCE),
        _ => None,
    }
}

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
    let source = fixture_source(relative_to_django)?;
    find_function_in_source(source, func_name)
}
