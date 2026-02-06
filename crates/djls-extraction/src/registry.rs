use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::Keyword;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtExpr;
use ruff_python_ast::StmtFunctionDef;

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
}

/// Collect all tag and filter registrations from a Python module source.
///
/// Walks the AST looking for:
/// - `@register.tag` / `@register.simple_tag` / `@register.inclusion_tag` /
///   `@register.filter` decorators on function definitions
/// - `register.tag("name", func)` / `register.filter("name", func)` call
///   expressions as standalone statements
#[cfg(feature = "parser")]
#[must_use]
pub fn collect_registrations(source: &str) -> Vec<RegistrationInfo> {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return Vec::new();
    };
    let module = parsed.into_syntax();
    let mut registrations = Vec::new();
    collect_from_body(&module.body, &mut registrations);
    registrations
}

#[cfg(feature = "parser")]
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
#[cfg(feature = "parser")]
fn collect_from_decorated_function(
    func_def: &StmtFunctionDef,
    registrations: &mut Vec<RegistrationInfo>,
) {
    let func_name = func_def.name.as_str();

    for decorator in &func_def.decorator_list {
        // Try tag decorator
        if let Some((name, kind)) =
            tag_name_from_decorator(&decorator.expression, func_name)
        {
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
    let func_name = kw_func_name(&call.arguments.keywords);

    let args = &call.arguments.args;

    if args.len() >= 2 {
        // `register.tag("name", func)` — first arg is string name, second is callable
        if let Some(name) = expr_string_value(&args[0]) {
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
        // Fallback: use the callable name as the registration name
        if let Expr::Name(ExprName { id, .. }) = &args[0] {
            return Some((id.to_string(), kind, Some(id.to_string())));
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
    let func_name = kw_func_name_filter(&call.arguments.keywords);

    let args = &call.arguments.args;

    if args.len() >= 2 {
        // `register.filter("name", func)`
        if let Some(name) = expr_string_value(&args[0]) {
            let fn_name = callable_name(&args[1]).or(func_name);
            return Some((name_override.unwrap_or(name), fn_name));
        }
    }

    if args.len() == 1 {
        let fn_name = callable_name(&args[0]).or(func_name.clone());
        if let Some(name) = name_override {
            return Some((name, fn_name));
        }
        if let Expr::Name(ExprName { id, .. }) = &args[0] {
            return Some((id.to_string(), Some(id.to_string())));
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
        if let Some(s) = expr_string_value(&kw.value) {
            return Some(s);
        }
    }
    None
}

/// Extract the `compile_function=` or `func=` keyword argument name (for tag calls).
fn kw_func_name(keywords: &[Keyword]) -> Option<String> {
    for kw in keywords {
        let Some(arg) = &kw.arg else { continue };
        if arg.as_str() == "compile_function" || arg.as_str() == "func" {
            if let Expr::Name(ExprName { id, .. }) = &kw.value {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Extract the `filter_func=` or `func=` keyword argument name (for filter calls).
fn kw_func_name_filter(keywords: &[Keyword]) -> Option<String> {
    for kw in keywords {
        let Some(arg) = &kw.arg else { continue };
        if arg.as_str() == "filter_func" || arg.as_str() == "func" {
            if let Expr::Name(ExprName { id, .. }) = &kw.value {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Extract a string value from an expression (string literal only).
fn expr_string_value(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = expr {
        return Some(value.to_str().to_string());
    }
    None
}

/// Extract the first positional argument's string value.
fn first_string_arg(args: &[Expr]) -> Option<String> {
    args.first().and_then(expr_string_value)
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

    #[test]
    fn decorator_bare_tag() {
        let source = r"
from django import template
register = template.Library()

@register.tag
def my_tag(parser, token):
    pass
";
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "my_tag");
        assert_eq!(regs[0].kind, RegistrationKind::Tag);
        assert_eq!(regs[0].func_name.as_deref(), Some("my_tag"));
    }

    #[test]
    fn decorator_simple_tag_with_name_kwarg() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_tag(name="greeting")
def hello(name):
    return f"Hello, {name}!"
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "greeting");
        assert_eq!(regs[0].kind, RegistrationKind::SimpleTag);
        assert_eq!(regs[0].func_name.as_deref(), Some("hello"));
    }

    #[test]
    fn decorator_inclusion_tag() {
        let source = r#"
from django import template
register = template.Library()

@register.inclusion_tag("results.html")
def show_results(poll):
    return {"choices": poll.choice_set.all()}
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "show_results");
        assert_eq!(regs[0].kind, RegistrationKind::InclusionTag);
    }

    #[test]
    fn decorator_filter_bare() {
        let source = r#"
from django import template
register = template.Library()

@register.filter
def cut(value, arg):
    return value.replace(arg, "")
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "cut");
        assert_eq!(regs[0].kind, RegistrationKind::Filter);
    }

    #[test]
    fn decorator_filter_with_name_kwarg() {
        let source = r#"
from django import template
register = template.Library()

@register.filter(name="mycut")
def cut(value, arg):
    return value.replace(arg, "")
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "mycut");
        assert_eq!(regs[0].kind, RegistrationKind::Filter);
    }

    #[test]
    fn call_style_tag_registration() {
        let source = r#"
from django import template
register = template.Library()

def do_for(parser, token):
    pass

register.tag("for", do_for)
"#;
        let regs = collect_registrations(source);
        // Should find both: the function def (no decorators) and the call-style registration
        // Only the call-style produces a registration since do_for has no decorators
        let tag_regs: Vec<_> = regs
            .iter()
            .filter(|r| r.kind == RegistrationKind::Tag)
            .collect();
        assert_eq!(tag_regs.len(), 1);
        assert_eq!(tag_regs[0].name, "for");
        assert_eq!(tag_regs[0].func_name.as_deref(), Some("do_for"));
    }

    #[test]
    fn call_style_filter_registration() {
        let source = r#"
from django import template
register = template.Library()

def my_func(value):
    return value.upper()

register.filter("upper_it", my_func)
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "upper_it");
        assert_eq!(regs[0].kind, RegistrationKind::Filter);
        assert_eq!(regs[0].func_name.as_deref(), Some("my_func"));
    }

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

    #[test]
    fn multiple_registrations() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def my_tag(parser, token):
    pass

@register.simple_tag
def greeting():
    return "Hello"

@register.filter
def lower_it(value):
    return value.lower()

register.tag("explicit", do_explicit)
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 4);
        assert_eq!(regs[0].name, "my_tag");
        assert_eq!(regs[0].kind, RegistrationKind::Tag);
        assert_eq!(regs[1].name, "greeting");
        assert_eq!(regs[1].kind, RegistrationKind::SimpleTag);
        assert_eq!(regs[2].name, "lower_it");
        assert_eq!(regs[2].kind, RegistrationKind::Filter);
        assert_eq!(regs[3].name, "explicit");
        assert_eq!(regs[3].kind, RegistrationKind::Tag);
    }

    #[test]
    fn tag_with_positional_string_name() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("custom_name")
def my_tag(parser, token):
    pass
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "custom_name");
        assert_eq!(regs[0].kind, RegistrationKind::Tag);
    }

    #[test]
    fn call_style_tag_with_method_callable() {
        let source = r#"
from django import template
register = template.Library()

register.tag("for", ForNode.handle)
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "for");
        assert_eq!(regs[0].func_name.as_deref(), Some("ForNode.handle"));
    }

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
    fn simple_block_tag_decorator() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_block_tag
def my_block(content):
    return f"<div>{content}</div>"
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "my_block");
        assert_eq!(regs[0].kind, RegistrationKind::SimpleBlockTag);
    }

    #[test]
    fn empty_source() {
        let regs = collect_registrations("");
        assert!(regs.is_empty());
    }

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

    #[test]
    fn filter_with_positional_string_name() {
        let source = r#"
from django import template
register = template.Library()

@register.filter("custom_filter")
def my_filter(value):
    return value
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "custom_filter");
        assert_eq!(regs[0].kind, RegistrationKind::Filter);
    }

    #[test]
    fn filter_with_is_safe_kwarg() {
        let source = r"
from django import template
register = template.Library()

@register.filter(is_safe=True)
def my_filter(value):
    return value
";
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "my_filter");
        assert_eq!(regs[0].kind, RegistrationKind::Filter);
    }

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
