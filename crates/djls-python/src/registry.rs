use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::Keyword;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtExpr;
use ruff_python_ast::StmtFunctionDef;

use crate::blocks;
use crate::dataflow;
use crate::ext::ExprExt;
use crate::filters;
use crate::signature;
use crate::types::BlockTagSpec;
use crate::types::FilterArity;
use crate::types::TagRule;
use crate::SymbolKind;

/// Decorator helper names on `django.template.Library` that register filters.
const FILTER_DECORATORS: &[&str] = &["filter"];

/// Information about a single tag or filter registration found in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationInfo {
    pub name: String,
    pub kind: RegistrationKind,
    pub func_name: Option<String>,
}

/// The style of registration, distinguishing decorator helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationKind {
    Tag,
    SimpleTag,
    InclusionTag,
    SimpleBlockTag,
    Filter,
}

/// Output of [`RegistrationKind::extract`], distinguishing filter vs tag results.
pub enum ExtractionOutput {
    Filter(FilterArity),
    Tag {
        rule: Option<TagRule>,
        block_spec: Option<BlockTagSpec>,
    },
}

impl RegistrationKind {
    #[must_use]
    pub fn symbol_kind(self) -> SymbolKind {
        match self {
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                SymbolKind::Tag
            }
            Self::Filter => SymbolKind::Filter,
        }
    }

    /// Run the appropriate extraction for this registration kind.
    ///
    /// For filters, extracts argument arity from the function signature.
    /// For tag variants, extracts validation rules (via signature analysis or
    /// dataflow analysis) and block structure.
    #[must_use]
    pub fn extract(self, func: &StmtFunctionDef) -> ExtractionOutput {
        match self {
            Self::Filter => ExtractionOutput::Filter(filters::extract_filter_arity(func)),
            Self::SimpleTag | Self::InclusionTag => {
                let is_simple_tag = matches!(self, Self::SimpleTag);
                let rule = signature::extract_parse_bits_rule(func, is_simple_tag);
                let rule = rule.has_content().then_some(rule);
                let block_spec = blocks::extract_block_spec(func);
                ExtractionOutput::Tag { rule, block_spec }
            }
            Self::Tag | Self::SimpleBlockTag => {
                let rule = dataflow::analyze_compile_function(func);
                let rule = rule.has_content().then_some(rule);
                let block_spec = blocks::extract_block_spec(func);
                ExtractionOutput::Tag { rule, block_spec }
            }
        }
    }
}

/// Collect registrations from a pre-parsed module body.
///
/// This avoids re-parsing the source when the caller already has the AST.
#[must_use]
pub fn collect_registrations_from_body(body: &[Stmt]) -> Vec<RegistrationInfo> {
    let mut registrations = Vec::new();
    collect_from_body(body, &mut registrations);
    registrations
}

fn collect_from_body(body: &[Stmt], registrations: &mut Vec<RegistrationInfo>) {
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                collect_from_decorated_function(func_def, registrations);
            }
            Stmt::Expr(StmtExpr { value, .. }) => {
                if let Expr::Call(call) = value.as_ref() {
                    collect_from_call_statement(call, registrations);
                }
            }
            Stmt::ClassDef(class_def) => {
                collect_from_body(&class_def.body, registrations);
            }
            _ => {}
        }
    }
}

/// Extract registrations from a decorated function definition.
///
/// Handles patterns like:
/// - `@register.tag` (bare decorator)
/// - `@register.simple_tag(name="alias")`
/// - `@register.tag("name")`
/// - `@register.filter`
fn collect_from_decorated_function(
    func_def: &StmtFunctionDef,
    registrations: &mut Vec<RegistrationInfo>,
) {
    let func_name = func_def.name.as_str();

    for decorator in &func_def.decorator_list {
        // Try tag decorator
        if let Some((name, kind)) = tag_name_from_decorator(&decorator.expression, func_name) {
            registrations.push(RegistrationInfo {
                name,
                kind,
                func_name: Some(func_name.to_string()),
            });
            continue;
        }

        // Try filter decorator
        if let Some(name) = filter_name_from_decorator(&decorator.expression, func_name) {
            registrations.push(RegistrationInfo {
                name,
                kind: RegistrationKind::Filter,
                func_name: Some(func_name.to_string()),
            });
        }
    }
}

/// Extract a tag name from a decorator expression.
///
/// Returns `Some((name, kind))` if the decorator is a tag registration.
fn tag_name_from_decorator(expr: &Expr, func_name: &str) -> Option<(String, RegistrationKind)> {
    // Bare decorator: `@register.tag`
    if let Expr::Attribute(ExprAttribute { attr, .. }) = expr {
        if let Some(kind) = tag_decorator_kind(attr.as_str()) {
            return Some((func_name.to_string(), kind));
        }
    }

    // Call decorator: `@register.tag(...)` or `@register.simple_tag(name="alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    {
        if let Expr::Attribute(ExprAttribute { attr, .. }) = func.as_ref() {
            if let Some(kind) = tag_decorator_kind(attr.as_str()) {
                // Priority: name= kwarg > first positional string (for @register.tag only) > func_name
                let name_override = kw_name_from(&arguments.keywords);

                let positional_name = if attr.as_str() == "tag" {
                    first_string_arg(&arguments.args)
                } else {
                    None
                };

                let name = name_override
                    .or(positional_name)
                    .unwrap_or_else(|| func_name.to_string());

                return Some((name, kind));
            }
        }
    }

    None
}

/// Extract a filter name from a decorator expression.
///
/// Returns `Some(name)` if the decorator is a filter registration.
fn filter_name_from_decorator(expr: &Expr, func_name: &str) -> Option<String> {
    // Bare decorator: `@register.filter`
    if let Expr::Attribute(ExprAttribute { attr, .. }) = expr {
        if FILTER_DECORATORS.contains(&attr.as_str()) {
            return Some(func_name.to_string());
        }
    }

    // Call decorator: `@register.filter(name="alias")` or `@register.filter("alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    {
        if let Expr::Attribute(ExprAttribute { attr, .. }) = func.as_ref() {
            if FILTER_DECORATORS.contains(&attr.as_str()) {
                let name_override = kw_name_from(&arguments.keywords);
                let positional_name = first_string_arg(&arguments.args);
                let name = name_override
                    .or(positional_name)
                    .unwrap_or_else(|| func_name.to_string());
                return Some(name);
            }
        }
    }

    None
}

/// Extract registrations from a call expression statement.
///
/// Handles patterns like:
/// - `register.tag("name", compile_func)`
/// - `register.tag("name", SomeNode.handle)`
/// - `register.filter("name", filter_func)`
/// - `register.simple_tag(func, name="alias")`
fn collect_from_call_statement(call: &ExprCall, registrations: &mut Vec<RegistrationInfo>) {
    // Try tag call-style registration
    if let Some((name, kind, func_name)) = tag_registration_from_call(call) {
        registrations.push(RegistrationInfo {
            name,
            kind,
            func_name,
        });
        return;
    }

    // Try filter call-style registration
    if let Some((name, func_name)) = filter_registration_from_call(call) {
        registrations.push(RegistrationInfo {
            name,
            kind: RegistrationKind::Filter,
            func_name,
        });
    }
}

/// Extract tag registration info from a call expression.
///
/// Returns `Some((name, kind, func_name))` for patterns like:
/// - `register.tag("name", func)`
/// - `register.simple_tag(func, name="alias")`
fn tag_registration_from_call(
    call: &ExprCall,
) -> Option<(String, RegistrationKind, Option<String>)> {
    let Expr::Attribute(ExprAttribute { attr, .. }) = call.func.as_ref() else {
        return None;
    };
    let kind = tag_decorator_kind(attr.as_str())?;

    let name_override = kw_name_from(&call.arguments.keywords);
    let func_name = kw_callable_name(&call.arguments.keywords, &["compile_function", "func"]);

    let args = &call.arguments.args;

    if args.len() >= 2 {
        // `register.tag("name", func)` — first arg is string name, second is callable
        if let Some(name) = args[0].string_literal() {
            let fn_name = callable_name(&args[1]).or(func_name);
            return Some((name_override.unwrap_or(name), kind, fn_name));
        }
    }

    if args.len() == 1 {
        // `register.simple_tag(func, name="alias")` or `register.tag(func)`
        let fn_name = callable_name(&args[0]).or(func_name.clone());
        if let Some(name) = name_override {
            return Some((name, kind, fn_name));
        }
        // Fallback: use the callable name as the registration name.
        // Handles both simple names (`do_for`) and attribute callables (`ForNode.handle`).
        if let Some(name) = callable_name(&args[0]) {
            return Some((name.clone(), kind, Some(name)));
        }
    }

    // No positional args but has name= kwarg
    if let Some(name) = name_override {
        return Some((name, kind, func_name));
    }

    None
}

/// Extract filter registration info from a call expression.
///
/// Returns `Some((name, func_name))` for patterns like:
/// - `register.filter("name", func)`
/// - `register.filter(func, name="alias")`
fn filter_registration_from_call(call: &ExprCall) -> Option<(String, Option<String>)> {
    let Expr::Attribute(ExprAttribute { attr, .. }) = call.func.as_ref() else {
        return None;
    };
    if !FILTER_DECORATORS.contains(&attr.as_str()) {
        return None;
    }

    let name_override = kw_name_from(&call.arguments.keywords);
    let func_name = kw_callable_name(&call.arguments.keywords, &["filter_func", "func"]);

    let args = &call.arguments.args;

    if args.len() >= 2 {
        // `register.filter("name", func)`
        if let Some(name) = args[0].string_literal() {
            let fn_name = callable_name(&args[1]).or(func_name);
            return Some((name_override.unwrap_or(name), fn_name));
        }
    }

    if args.len() == 1 {
        let fn_name = callable_name(&args[0]).or(func_name.clone());
        if let Some(name) = name_override {
            return Some((name, fn_name));
        }
        // Fallback: use the callable name as the registration name.
        // Handles both simple names (`my_func`) and attribute callables (`MyClass.method`).
        if let Some(name) = callable_name(&args[0]) {
            return Some((name.clone(), Some(name)));
        }
    }

    if let Some(name) = name_override {
        return Some((name, func_name));
    }

    None
}

/// Map decorator attr name to `RegistrationKind`.
fn tag_decorator_kind(attr: &str) -> Option<RegistrationKind> {
    match attr {
        "tag" => Some(RegistrationKind::Tag),
        "simple_tag" => Some(RegistrationKind::SimpleTag),
        "inclusion_tag" => Some(RegistrationKind::InclusionTag),
        "simple_block_tag" => Some(RegistrationKind::SimpleBlockTag),
        _ => None,
    }
}

/// Extract the `name=` keyword argument value as a string.
fn kw_name_from(keywords: &[Keyword]) -> Option<String> {
    kw_constant_str(keywords, "name")
}

/// Extract a keyword argument's string constant value by argument name.
fn kw_constant_str(keywords: &[Keyword], name: &str) -> Option<String> {
    for kw in keywords {
        let Some(arg) = &kw.arg else { continue };
        if arg.as_str() != name {
            continue;
        }
        if let Some(s) = kw.value.string_literal() {
            return Some(s);
        }
    }
    None
}

/// Extract a callable name from keyword arguments by checking the given keyword names.
fn kw_callable_name(keywords: &[Keyword], kwarg_names: &[&str]) -> Option<String> {
    for kw in keywords {
        let Some(arg) = &kw.arg else { continue };
        if kwarg_names.contains(&arg.as_str()) {
            if let Expr::Name(ExprName { id, .. }) = &kw.value {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Extract the first positional argument's string value.
fn first_string_arg(args: &[Expr]) -> Option<String> {
    args.first().and_then(ExprExt::string_literal)
}

/// Best-effort callable name extraction for debugging / registration mapping.
fn callable_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(ExprName { id, .. }) => Some(id.to_string()),
        Expr::Attribute(ExprAttribute { value, attr, .. }) => {
            let base = callable_name(value)?;
            Some(format!("{base}.{}", attr.as_str()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::django_source;

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
        let source = crate::test_helpers::corpus_source(
            "packages/wagtail/7.3/wagtail/admin/templatetags/wagtailadmin_tags.py",
        )
        .unwrap();
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
        let source = crate::test_helpers::corpus_source(
            "packages/wagtail/7.3/wagtail/admin/templatetags/wagtailadmin_tags.py",
        )
        .unwrap();
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
}
