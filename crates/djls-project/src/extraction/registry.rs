use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::Keyword;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtExpr;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::statement_visitor::walk_stmt;

use crate::extraction::ext::ExprExt;

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

/// Collect registrations from a pre-parsed module body.
///
/// This avoids re-parsing the source when the caller already has the AST.
#[must_use]
pub fn collect_registrations_from_body(body: &[Stmt]) -> Vec<RegistrationInfo> {
    let mut visitor = RegistrationCollector::default();
    visitor.visit_body(body);
    visitor.registrations
}

#[derive(Default)]
struct RegistrationCollector {
    registrations: Vec<RegistrationInfo>,
}

impl StatementVisitor<'_> for RegistrationCollector {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                collect_from_decorated_function(func_def, &mut self.registrations);
            }
            Stmt::Expr(StmtExpr { value, .. }) => {
                if let Expr::Call(call) = value.as_ref() {
                    collect_from_call_statement(call, &mut self.registrations);
                }
            }
            Stmt::ClassDef(_) => {
                walk_stmt(self, stmt);
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
    if let Expr::Attribute(ExprAttribute { attr, .. }) = expr
        && let Some(kind) = tag_decorator_kind(attr.as_str())
    {
        return Some((func_name.to_string(), kind));
    }

    // Call decorator: `@register.tag(...)` or `@register.simple_tag(name="alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
        && let Expr::Attribute(ExprAttribute { attr, .. }) = func.as_ref()
        && let Some(kind) = tag_decorator_kind(attr.as_str())
    {
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

    None
}

/// Extract a filter name from a decorator expression.
///
/// Returns `Some(name)` if the decorator is a filter registration.
fn filter_name_from_decorator(expr: &Expr, func_name: &str) -> Option<String> {
    // Bare decorator: `@register.filter`
    if let Expr::Attribute(ExprAttribute { attr, .. }) = expr
        && FILTER_DECORATORS.contains(&attr.as_str())
    {
        return Some(func_name.to_string());
    }

    // Call decorator: `@register.filter(name="alias")` or `@register.filter("alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
        && let Expr::Attribute(ExprAttribute { attr, .. }) = func.as_ref()
        && FILTER_DECORATORS.contains(&attr.as_str())
    {
        let name_override = kw_name_from(&arguments.keywords);
        let positional_name = first_string_arg(&arguments.args);
        let name = name_override
            .or(positional_name)
            .unwrap_or_else(|| func_name.to_string());
        return Some(name);
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
        if kwarg_names.contains(&arg.as_str())
            && let Expr::Name(ExprName { id, .. }) = &kw.value
        {
            return Some(id.to_string());
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
#[path = "../../tests/support/extraction_registry.rs"]
mod tests;
