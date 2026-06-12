use djls_testing::Corpus;

use super::*;

fn corpus_source(relative_path: &str) -> Option<String> {
    let corpus = Corpus::require();
    let path = corpus.root().join(relative_path);
    std::fs::read_to_string(path.as_std_path()).ok()
}

fn package_source(package: &str, relative_to_package: &str) -> Option<String> {
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

fn django_source(relative_to_django: &str) -> Option<String> {
    package_source("django", relative_to_django)
}

fn collect_registrations(source: &str) -> Vec<RegistrationInfo> {
    let parsed = ruff_python_parser::parse_module(source).expect("valid Python");
    let module = parsed.into_syntax();
    collect_registrations_from_body(&module.body)
}

fn find_reg<'a>(regs: &'a [RegistrationInfo], name: &str) -> &'a RegistrationInfo {
    regs.iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("registration '{name}' not found"))
}

// Corpus: `autoescape` in django/template/defaulttags.py uses `@register.tag` (bare)
#[test]
fn decorator_bare_tag() {
    let source = django_source("django/template/defaulttags.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "autoescape");
    assert_eq!(reg.kind, RegistrationKind::Tag);
    assert_eq!(reg.func_name.as_deref(), Some("autoescape"));
}

// Corpus: `querystring` in django/template/defaulttags.py uses
// `@register.simple_tag(name="querystring", takes_context=True)`
#[test]
fn decorator_simple_tag_with_name_kwarg() {
    let source = django_source("django/template/defaulttags.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "querystring");
    assert_eq!(reg.kind, RegistrationKind::SimpleTag);
    assert_eq!(reg.func_name.as_deref(), Some("querystring"));
}

// Corpus: `inclusion_no_params` in tests/template_tests/templatetags/inclusion.py uses
// `@register.inclusion_tag("inclusion.html")`
#[test]
fn decorator_inclusion_tag() {
    let source = django_source("tests/template_tests/templatetags/inclusion.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "inclusion_no_params");
    assert_eq!(reg.kind, RegistrationKind::InclusionTag);
}

// Corpus: `cut` in django/template/defaultfilters.py uses `@register.filter` (bare)
#[test]
fn decorator_filter_bare() {
    let source = django_source("django/template/defaultfilters.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "cut");
    assert_eq!(reg.kind, RegistrationKind::Filter);
}

// Corpus: `escapejs` in django/template/defaultfilters.py uses
// `@register.filter("escapejs")` — positional string name, func is `escapejs_filter`
#[test]
fn decorator_filter_with_positional_string_name() {
    let source = django_source("django/template/defaultfilters.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "escapejs");
    assert_eq!(reg.kind, RegistrationKind::Filter);
    assert_eq!(reg.func_name.as_deref(), Some("escapejs_filter"));
}

// Corpus: `other_echo` in tests/template_tests/templatetags/testtags.py uses
// `register.tag("other_echo", echo)` — call-style registration
#[test]
fn call_style_tag_registration() {
    let source = django_source("tests/template_tests/templatetags/testtags.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "other_echo");
    assert_eq!(reg.kind, RegistrationKind::Tag);
    assert_eq!(reg.func_name.as_deref(), Some("echo"));
}

// Corpus: `intcomma` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
// `register.filter("intcomma", intcomma)` — call-style filter registration
#[test]
fn call_style_filter_registration() {
    let source =
        package_source("wagtail", "wagtail/admin/templatetags/wagtailadmin_tags.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "intcomma");
    assert_eq!(reg.kind, RegistrationKind::Filter);
    assert_eq!(reg.func_name.as_deref(), Some("intcomma"));
}

// Corpus: `for` in django/template/defaulttags.py uses `@register.tag("for")`
// — positional string name overrides function name `do_for`
#[test]
fn tag_with_positional_string_name() {
    let source = django_source("django/template/defaulttags.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "for");
    assert_eq!(reg.kind, RegistrationKind::Tag);
    assert_eq!(reg.func_name.as_deref(), Some("do_for"));
}

// Corpus: `addslashes` in django/template/defaultfilters.py uses
// `@register.filter(is_safe=True)` — name defaults to function name
#[test]
fn filter_with_is_safe_kwarg() {
    let source = django_source("django/template/defaultfilters.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "addslashes");
    assert_eq!(reg.kind, RegistrationKind::Filter);
    assert_eq!(reg.func_name.as_deref(), Some("addslashes"));
}

// Corpus: `partialdef` in django/template/defaulttags.py uses
// `@register.tag(name="partialdef")` — name kwarg overrides func name `partialdef_func`
#[test]
fn tag_with_name_kwarg() {
    let source = django_source("django/template/defaulttags.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "partialdef");
    assert_eq!(reg.kind, RegistrationKind::Tag);
    assert_eq!(reg.func_name.as_deref(), Some("partialdef_func"));
}

// Corpus: `dialog` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
// `register.tag("dialog", DialogNode.handle)` — call-style with method callable
#[test]
fn call_style_tag_with_method_callable() {
    let source =
        package_source("wagtail", "wagtail/admin/templatetags/wagtailadmin_tags.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "dialog");
    assert_eq!(reg.kind, RegistrationKind::Tag);
    assert_eq!(reg.func_name.as_deref(), Some("DialogNode.handle"));
}

// Corpus: `div` in tests/template_tests/templatetags/custom.py uses
// `@register.simple_block_tag` (bare decorator)
#[test]
fn simple_block_tag_decorator() {
    let source = django_source("tests/template_tests/templatetags/custom.py").unwrap();
    let regs = collect_registrations(&source);
    let reg = find_reg(&regs, "div");
    assert_eq!(reg.kind, RegistrationKind::SimpleBlockTag);
}

// Corpus: defaulttags.py has many registrations (tags + simple_tags)
#[test]
fn multiple_registrations() {
    let source = django_source("django/template/defaulttags.py").unwrap();
    let regs = collect_registrations(&source);
    assert!(
        regs.len() > 10,
        "expected many registrations in defaulttags.py, got {}",
        regs.len()
    );
    let tags: Vec<_> = regs
        .iter()
        .filter(|r| r.kind == RegistrationKind::Tag)
        .collect();
    assert!(
        tags.len() > 5,
        "expected multiple Tag registrations, got {}",
        tags.len()
    );
    assert!(regs.iter().any(|r| r.name == "for"));
    assert!(regs.iter().any(|r| r.name == "if"));
    assert!(regs.iter().any(|r| r.name == "autoescape"));
}

// Corpus: testtags.py has decorator @register.tag + call-style register.tag
// Tests that both decorator and call-style registrations are discovered
#[test]
fn mixed_decorator_and_call_style() {
    let source = django_source("tests/template_tests/templatetags/testtags.py").unwrap();
    let regs = collect_registrations(&source);
    let tag_regs: Vec<_> = regs
        .iter()
        .filter(|r| r.kind == RegistrationKind::Tag)
        .collect();
    assert_eq!(tag_regs.len(), 2);
    assert!(tag_regs.iter().any(|r| r.name == "echo"));
    assert!(tag_regs.iter().any(|r| r.name == "other_echo"));
}

// Edge case: @register.tag() with empty parens — function name used as tag name.
// Corpus: no clean isolatable example of empty parens (all corpus uses bare or with args).
#[test]
fn function_name_fallback() {
    let source = r"
from django import template
register = template.Library()

@register.tag()
def current_time(parser, token):
    pass
";
    let regs = collect_registrations(source);
    assert_eq!(regs.len(), 1);
    assert_eq!(regs[0].name, "current_time");
    assert_eq!(regs[0].kind, RegistrationKind::Tag);
}

// Edge case: register.simple_tag(my_func, name="alias") — call-style with func positional
// and name kwarg. Rare pattern, not found cleanly in corpus.
#[test]
fn simple_tag_func_positional() {
    let source = r#"
from django import template
register = template.Library()

register.simple_tag(my_func, name="alias")
"#;
    let regs = collect_registrations(source);
    assert_eq!(regs.len(), 1);
    assert_eq!(regs[0].name, "alias");
    assert_eq!(regs[0].kind, RegistrationKind::SimpleTag);
    assert_eq!(regs[0].func_name.as_deref(), Some("my_func"));
}

#[test]
fn empty_source() {
    let regs = collect_registrations("");
    assert!(regs.is_empty());
}

// Edge case: source with no registration patterns
#[test]
fn no_registrations() {
    let source = r"
def regular_function():
    pass

class MyClass:
    pass
";
    let regs = collect_registrations(source);
    assert!(regs.is_empty());
}

// Edge case: register.tag(do_something) — single func arg, no name string.
// Valid Django API but rare. Not found cleanly in corpus.
#[test]
fn call_style_single_func_no_name() {
    let source = r"
from django import template
register = template.Library()

register.tag(do_something)
";
    let regs = collect_registrations(source);
    assert_eq!(regs.len(), 1);
    assert_eq!(regs[0].name, "do_something");
    assert_eq!(regs[0].kind, RegistrationKind::Tag);
}

// Edge case: register.filter(my_filter_func) — single func arg, no name string.
// Valid Django API but rare. Not found cleanly in corpus.
#[test]
fn call_style_filter_single_func_no_name() {
    let source = r"
from django import template
register = template.Library()

register.filter(my_filter_func)
";
    let regs = collect_registrations(source);
    assert_eq!(regs.len(), 1);
    assert_eq!(regs[0].name, "my_filter_func");
    assert_eq!(regs[0].kind, RegistrationKind::Filter);
}

// Edge case: name kwarg overrides positional string arg.
// Tests priority: name= kwarg wins over positional string.
#[test]
fn name_kwarg_overrides_positional_for_tag() {
    let source = r#"
from django import template
register = template.Library()

@register.tag("positional_name", name="kwarg_name")
def my_tag(parser, token):
    pass
"#;
    let regs = collect_registrations(source);
    assert_eq!(regs.len(), 1);
    assert_eq!(regs[0].name, "kwarg_name");
}
