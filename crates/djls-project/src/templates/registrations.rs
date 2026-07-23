use std::collections::BTreeMap;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Keyword;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtExpr;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;

use super::filters::FilterArityMap;
use super::libraries::TemplateLibraryId;
use super::names::TemplateSymbolName;
use super::symbols::SymbolDefinition;
use super::symbols::SymbolKey;
use super::symbols::TemplateSymbol;
use super::symbols::TemplateSymbolKind;
use super::tags::BlockSpec;
use super::tags::BlockSpecs;
use super::tags::TagRuleMap;
use super::tags::blocks::EndTagEvidence;
use crate::ast::ExprExt;
use crate::db::Db as ProjectDb;
use crate::python::RecoveredPythonModule;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;

/// Decorator helper names on `django.template.Library` that register filters.
const FILTER_DECORATORS: &[&str] = &["filter"];

/// Information about a single tag or filter registration found in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistrationInfo {
    name: String,
    kind: RegistrationKind,
    func_name: Option<String>,
}

/// The style of registration, distinguishing decorator helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegistrationKind {
    Tag,
    SimpleTag,
    InclusionTag,
    SimpleBlockTag,
    Filter,
}

#[derive(Debug, Default)]
struct RegistrationSourceAnalysis {
    registrations: Vec<RegistrationInfo>,
    defines_library: bool,
    inventory_open: bool,
    saw_register_use: bool,
}

struct RegisterUseVisitor {
    found: bool,
}

impl<'a> Visitor<'a> for RegisterUseVisitor {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if expr.name_target() == Some("register") {
            self.found = true;
            return;
        }
        visitor::walk_expr(self, expr);
    }
}

fn contains_register(expr: &Expr) -> bool {
    let mut visitor = RegisterUseVisitor { found: false };
    visitor.visit_expr(expr);
    visitor.found
}

fn statement_contains_register(stmt: &Stmt) -> bool {
    let mut visitor = RegisterUseVisitor { found: false };
    visitor.visit_stmt(stmt);
    visitor.found
}

fn body_contains_register(body: &[Stmt]) -> bool {
    body.iter().any(statement_contains_register)
}

fn collect_from_class_body(body: &[Stmt], analysis: &mut RegistrationSourceAnalysis) {
    let mut register_is_module_binding = true;
    for stmt in body {
        if let Stmt::FunctionDef(function) = stmt
            && register_is_module_binding
        {
            collect_from_decorated_function(function, analysis);
            if body_contains_register(&function.body) {
                analysis.inventory_open = true;
            }
            continue;
        }
        if let Stmt::ClassDef(class) = stmt {
            collect_from_class_body(&class.body, analysis);
            continue;
        }
        if let Stmt::Assign(assign) = stmt
            && assign
                .targets
                .iter()
                .any(|target| target.name_target() == Some("register"))
        {
            register_is_module_binding = false;
        }
    }
}

fn is_register_inventory_target(expr: &Expr) -> bool {
    if let Expr::Subscript(subscript) = expr {
        return is_register_inventory_target(&subscript.value);
    }
    expr.path_segments().is_some_and(|path| {
        matches!(path.as_slice(), [register, inventory, ..]
            if register == "register" && matches!(inventory.as_str(), "tags" | "filters"))
    })
}

fn call_rooted_at_register(call: &ExprCall) -> bool {
    call.func
        .path_segments()
        .is_some_and(|path| path.first().is_some_and(|root| root == "register"))
}

fn call_escapes_register(call: &ExprCall) -> bool {
    contains_register(&call.func)
        || call.arguments.args.iter().any(contains_register)
        || call
            .arguments
            .keywords
            .iter()
            .any(|keyword| contains_register(&keyword.value))
}

fn is_fresh_canonical_library(
    expr: &Expr,
    template_is_django: bool,
    library_constructor: Option<&str>,
) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
        return false;
    }
    if template_is_django
        && call.func.path_segments().is_some_and(|path| {
            matches!(path.as_slice(), [template, library]
                if template == "template" && library == "Library")
        })
    {
        return true;
    }
    call.func
        .name_target()
        .is_some_and(|name| Some(name) == library_constructor)
}

fn is_canonical_library_import(syntax: &FromImportSyntax, module_name: &str) -> bool {
    (syntax.level() == 0
        && matches!(
            syntax.module(),
            Some("django.template" | "django.template.library")
        ))
        || (syntax.level() == 1
            && syntax.module() == Some("library")
            && module_name.starts_with("django.template."))
}

#[allow(clippy::too_many_lines)]
fn analyze_registrations_from_body_in_module(
    body: &[Stmt],
    module_name: &str,
) -> RegistrationSourceAnalysis {
    let mut analysis = RegistrationSourceAnalysis::default();
    let mut template_is_django = false;
    let mut library_constructor = None;

    for stmt in body {
        match stmt {
            Stmt::Import(import) => {
                for clause in DirectImportClause::lower(import) {
                    if clause.bound() == "template" {
                        template_is_django = false;
                    }
                    if library_constructor == Some(clause.bound()) {
                        library_constructor = None;
                    }
                    if clause.bound() == "register" {
                        analysis.registrations.clear();
                        analysis.defines_library = true;
                        analysis.inventory_open = true;
                    }
                }
            }
            Stmt::ImportFrom(import) => {
                let syntax = FromImportSyntax::lower(import);
                if syntax.has_star() {
                    template_is_django = false;
                    library_constructor = None;
                }
                let canonical_library_import = is_canonical_library_import(&syntax, module_name);
                for member in syntax.named_members() {
                    if member.bound() == "template" {
                        template_is_django = syntax.level() == 0
                            && syntax.module() == Some("django")
                            && member.imported() == "template";
                    }
                    if library_constructor == Some(member.bound()) {
                        library_constructor = None;
                    }
                    if canonical_library_import && member.imported() == "Library" {
                        library_constructor = Some(member.bound());
                    }
                    if member.bound() == "register" {
                        analysis.registrations.clear();
                        analysis.defines_library = true;
                        analysis.inventory_open = true;
                    }
                }
            }
            Stmt::Assign(assign) => {
                let binds_register = assign
                    .targets
                    .iter()
                    .any(|target| target.name_target() == Some("register"));
                let binds_template = assign
                    .targets
                    .iter()
                    .any(|target| target.name_target() == Some("template"));
                let shares_register_binding = binds_register
                    && (assign.targets.len() != 1
                        || assign.targets[0].name_target() != Some("register"));
                if assign.targets.iter().any(|target| {
                    target
                        .name_target()
                        .is_some_and(|name| library_constructor == Some(name))
                }) {
                    library_constructor = None;
                }
                if binds_register {
                    let fresh_canonical = is_fresh_canonical_library(
                        &assign.value,
                        template_is_django,
                        library_constructor,
                    );
                    if !fresh_canonical {
                        analysis.registrations.clear();
                    }
                    analysis.defines_library = true;
                    analysis.inventory_open |=
                        binds_template || shares_register_binding || !fresh_canonical;
                }
                if assign.targets.iter().any(is_register_inventory_target)
                    || (contains_register(&assign.value) && !binds_register)
                {
                    analysis.inventory_open = true;
                }
                if binds_template {
                    template_is_django = false;
                }
            }
            Stmt::AnnAssign(assign) => {
                if assign.target.name_target() == Some("register") {
                    analysis.registrations.clear();
                    analysis.defines_library = true;
                    analysis.inventory_open = true;
                }
                if is_register_inventory_target(&assign.target)
                    || assign.value.as_deref().is_some_and(contains_register)
                {
                    analysis.inventory_open = true;
                }
                if assign.target.name_target() == Some("template") {
                    template_is_django = false;
                }
                if assign
                    .target
                    .name_target()
                    .is_some_and(|name| library_constructor == Some(name))
                {
                    library_constructor = None;
                }
            }
            Stmt::AugAssign(assign) => {
                if assign.target.name_target() == Some("register")
                    || is_register_inventory_target(&assign.target)
                    || contains_register(&assign.value)
                {
                    analysis.inventory_open = true;
                }
                if assign.target.name_target() == Some("template") {
                    template_is_django = false;
                }
                if assign
                    .target
                    .name_target()
                    .is_some_and(|name| library_constructor == Some(name))
                {
                    library_constructor = None;
                }
            }
            Stmt::Delete(delete) => {
                if delete.targets.iter().any(|target| {
                    target.name_target() == Some("register")
                        || is_register_inventory_target(target)
                        || contains_register(target)
                }) {
                    analysis.inventory_open = true;
                }
                if delete
                    .targets
                    .iter()
                    .any(|target| target.name_target() == Some("template"))
                {
                    template_is_django = false;
                }
                if delete.targets.iter().any(|target| {
                    target
                        .name_target()
                        .is_some_and(|name| library_constructor == Some(name))
                }) {
                    library_constructor = None;
                }
            }
            Stmt::FunctionDef(function) => {
                if function.name.as_str() == "template" {
                    template_is_django = false;
                }
                if library_constructor == Some(function.name.as_str()) {
                    library_constructor = None;
                }
                if function.name.as_str() == "register" {
                    analysis.registrations.clear();
                    analysis.defines_library = true;
                    analysis.inventory_open = true;
                }
                collect_from_decorated_function(function, &mut analysis);
                if body_contains_register(&function.body) {
                    analysis.saw_register_use = true;
                    analysis.inventory_open = true;
                }
            }
            Stmt::ClassDef(class) => {
                if class.name.as_str() == "template" {
                    template_is_django = false;
                }
                if library_constructor == Some(class.name.as_str()) {
                    library_constructor = None;
                }
                if class.name.as_str() == "register" {
                    analysis.registrations.clear();
                    analysis.defines_library = true;
                    analysis.inventory_open = true;
                }
                collect_from_class_body(&class.body, &mut analysis);
                if statement_contains_register(stmt) {
                    analysis.saw_register_use = true;
                    analysis.inventory_open = true;
                }
            }
            Stmt::Expr(StmtExpr { value, .. }) => {
                if let Expr::Call(call) = value.as_ref() {
                    collect_from_call_statement(call, &mut analysis);
                } else if contains_register(value) {
                    analysis.saw_register_use = true;
                    analysis.inventory_open = true;
                }
            }
            Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Try(_) => {
                template_is_django = false;
                library_constructor = None;
                if statement_contains_register(stmt) {
                    analysis.saw_register_use = true;
                    analysis.inventory_open = true;
                }
            }
            Stmt::Return(_)
            | Stmt::TypeAlias(_)
            | Stmt::Raise(_)
            | Stmt::Assert(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {
                if statement_contains_register(stmt) {
                    analysis.saw_register_use = true;
                    analysis.inventory_open = true;
                }
            }
        }
    }

    if analysis.saw_register_use && !analysis.defines_library {
        analysis.defines_library = true;
        analysis.inventory_open = true;
    }
    analysis
}

#[cfg(test)]
fn analyze_registrations_from_body(body: &[Stmt]) -> RegistrationSourceAnalysis {
    analyze_registrations_from_body_in_module(body, "")
}

/// Collect registrations from a pre-parsed module body.
///
/// This avoids re-parsing the source when the caller already has the AST.
#[cfg(test)]
#[must_use]
pub(crate) fn collect_registrations_from_body(body: &[Stmt]) -> Vec<RegistrationInfo> {
    analyze_registrations_from_body(body).registrations
}

fn for_each_registration(
    analysis: &RegistrationSourceAnalysis,
    body: &[Stmt],
    module_name: &str,
    mut f: impl FnMut(&RegistrationInfo, Option<&StmtFunctionDef>, SymbolKey),
) {
    let func_defs = collect_func_defs(body);

    for reg in &analysis.registrations {
        let func = reg.func_name.as_deref().and_then(|name| {
            func_defs
                .iter()
                .find(|func| func.name.as_str() == name)
                .copied()
        });

        let kind = reg.kind;
        let key = SymbolKey {
            registration_module: module_name.to_string(),
            name: reg.name.clone(),
            kind: kind.symbol_kind(),
        };

        f(reg, func, key);
    }
}

/// Collect module-level function definitions that can own definite registrations.
fn collect_func_defs(body: &[Stmt]) -> Vec<&StmtFunctionDef> {
    body.iter()
        .filter_map(|stmt| {
            if let Stmt::FunctionDef(function) = stmt {
                Some(function)
            } else {
                None
            }
        })
        .collect()
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
    analysis: &mut RegistrationSourceAnalysis,
) {
    let func_name = func_def.name.as_str();

    for decorator in &func_def.decorator_list {
        let expression = &decorator.expression;
        if !registration_decorator_rooted_at_register(expression) {
            if contains_register(expression) {
                analysis.inventory_open = true;
                analysis.saw_register_use = true;
            }
            continue;
        }
        analysis.saw_register_use = true;

        if registration_decorator_has_dynamic_name(expression) {
            analysis.inventory_open = true;
            continue;
        }

        if let Some((name, kind)) = tag_name_from_decorator(expression, func_name) {
            analysis.registrations.push(RegistrationInfo {
                name,
                kind,
                func_name: Some(func_name.to_string()),
            });
            continue;
        }

        if let Some(name) = filter_name_from_decorator(expression, func_name) {
            analysis.registrations.push(RegistrationInfo {
                name,
                kind: RegistrationKind::Filter,
                func_name: Some(func_name.to_string()),
            });
        } else {
            analysis.inventory_open = true;
        }
    }
}

fn direct_register_helper(expr: &Expr) -> Option<&str> {
    let Expr::Attribute(ExprAttribute { value, attr, .. }) = expr else {
        return None;
    };
    (value.name_target() == Some("register")).then_some(attr.as_str())
}

fn registration_decorator_rooted_at_register(expr: &Expr) -> bool {
    let helper = if matches!(expr, Expr::Attribute(_)) {
        direct_register_helper(expr)
    } else if let Expr::Call(call) = expr {
        direct_register_helper(&call.func)
    } else {
        None
    };
    helper.is_some_and(|helper| {
        tag_decorator_kind(helper).is_some() || FILTER_DECORATORS.contains(&helper)
    })
}

fn has_dynamic_name_keyword(keywords: &[Keyword]) -> bool {
    keywords.iter().any(|keyword| {
        keyword.arg.as_ref().is_some_and(|arg| arg == "name")
            && keyword.value.string_literal().is_none()
    })
}

fn registration_arguments_are_unsupported(helper: &str, call: &ExprCall) -> bool {
    if call
        .arguments
        .args
        .iter()
        .any(|arg| matches!(arg, Expr::Starred(_)))
    {
        return true;
    }
    call.arguments.keywords.iter().any(|keyword| {
        let Some(name) = keyword
            .arg
            .as_ref()
            .map(ruff_python_ast::Identifier::as_str)
        else {
            return true;
        };
        match helper {
            "tag" => !matches!(name, "name" | "compile_function" | "func"),
            "filter" => false,
            "simple_tag" => !matches!(name, "func" | "takes_context" | "name"),
            "inclusion_tag" => !matches!(name, "filename" | "func" | "takes_context" | "name"),
            "simple_block_tag" => !matches!(name, "func" | "takes_context" | "name" | "end_name"),
            _ => true,
        }
    })
}

fn registration_decorator_has_dynamic_name(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if has_dynamic_name_keyword(&call.arguments.keywords) {
        return true;
    }
    direct_register_helper(&call.func).is_some_and(|helper| {
        if registration_arguments_are_unsupported(helper, call) {
            return true;
        }
        let args = &call.arguments.args;
        match helper {
            "tag" | "filter" => {
                args.len() > 1
                    || args
                        .first()
                        .is_some_and(|arg| arg.string_literal().is_none())
            }
            "simple_tag" | "simple_block_tag" => !args.is_empty(),
            "inclusion_tag" => args.len() > 1,
            _ => true,
        }
    })
}

fn registration_call_has_dynamic_name(call: &ExprCall) -> bool {
    if has_dynamic_name_keyword(&call.arguments.keywords) {
        return true;
    }
    direct_register_helper(&call.func).is_some_and(|helper| {
        registration_arguments_are_unsupported(helper, call)
            || (matches!(helper, "tag" | "filter")
                && call.arguments.args.len() >= 2
                && call.arguments.args[0].string_literal().is_none())
    })
}

/// Extract a tag name from a decorator expression.
///
/// Returns `Some((name, kind))` if the decorator is a tag registration.
fn tag_name_from_decorator(expr: &Expr, func_name: &str) -> Option<(String, RegistrationKind)> {
    // Bare decorator: `@register.tag`
    if let Some(attr) = direct_register_helper(expr)
        && let Some(kind) = tag_decorator_kind(attr)
    {
        return Some((func_name.to_string(), kind));
    }

    // Call decorator: `@register.tag(...)` or `@register.simple_tag(name="alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
        && let Some(attr) = direct_register_helper(func)
        && let Some(kind) = tag_decorator_kind(attr)
    {
        // Priority: name= kwarg > first positional string (for @register.tag only) > func_name
        let name_override = kw_name_from(&arguments.keywords);

        let positional_name = if attr == "tag" {
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
    if let Some(attr) = direct_register_helper(expr)
        && FILTER_DECORATORS.contains(&attr)
    {
        return Some(func_name.to_string());
    }

    // Call decorator: `@register.filter(name="alias")` or `@register.filter("alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
        && let Some(attr) = direct_register_helper(func)
        && FILTER_DECORATORS.contains(&attr)
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
fn collect_from_call_statement(call: &ExprCall, analysis: &mut RegistrationSourceAnalysis) {
    if !call_rooted_at_register(call) {
        if call_escapes_register(call) {
            analysis.saw_register_use = true;
            analysis.inventory_open = true;
        }
        return;
    }
    analysis.saw_register_use = true;

    let Some(helper) = direct_register_helper(&call.func) else {
        analysis.inventory_open = true;
        return;
    };
    if tag_decorator_kind(helper).is_none() && !FILTER_DECORATORS.contains(&helper) {
        analysis.inventory_open = true;
        return;
    }
    if registration_call_has_dynamic_name(call) {
        analysis.inventory_open = true;
        return;
    }

    if let Some((name, kind, func_name)) = tag_registration_from_call(call) {
        analysis.registrations.push(RegistrationInfo {
            name,
            kind,
            func_name,
        });
        return;
    }

    if let Some((name, func_name)) = filter_registration_from_call(call) {
        analysis.registrations.push(RegistrationInfo {
            name,
            kind: RegistrationKind::Filter,
            func_name,
        });
    } else {
        analysis.inventory_open = true;
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
    let helper = direct_register_helper(&call.func)?;
    let kind = tag_decorator_kind(helper)?;
    let name_override = kw_name_from(&call.arguments.keywords);
    let keyword_func = kw_callable_name(&call.arguments.keywords, &["compile_function", "func"]);
    let args = &call.arguments.args;

    match helper {
        "tag" => match &args[..] {
            [name, callable] => {
                let name = name.string_literal()?.to_string();
                let func_name = callable_name(callable).or(keyword_func);
                Some((name, kind, func_name))
            }
            [name] if name.string_literal().is_some() => {
                let name = name.string_literal()?.to_string();
                let func_name = keyword_func?;
                Some((name, kind, Some(func_name)))
            }
            [callable] if name_override.is_none() => {
                let func_name = callable_name(callable)?;
                Some((func_name.clone(), kind, Some(func_name)))
            }
            [] => {
                let func_name = keyword_func?;
                let name = name_override.unwrap_or_else(|| func_name.clone());
                Some((name, kind, Some(func_name)))
            }
            _ => None,
        },
        "simple_tag" | "simple_block_tag" => match &args[..] {
            [callable] => {
                let func_name = callable_name(callable).or(keyword_func)?;
                let name = name_override.unwrap_or_else(|| func_name.clone());
                Some((name, kind, Some(func_name)))
            }
            [] => {
                let func_name = keyword_func?;
                let name = name_override.unwrap_or_else(|| func_name.clone());
                Some((name, kind, Some(func_name)))
            }
            _ => None,
        },
        "inclusion_tag" => {
            let func_name = match &args[..] {
                [_template, callable] => callable_name(callable).or(keyword_func),
                [_] | [] => keyword_func,
                _ => None,
            }?;
            let name = name_override.unwrap_or_else(|| func_name.clone());
            Some((name, kind, Some(func_name)))
        }
        _ => None,
    }
}

/// Extract filter registration info from a call expression.
///
/// Returns `Some((name, func_name))` for patterns like:
/// - `register.filter("name", func)`
/// - `register.filter(func, name="alias")`
fn filter_registration_from_call(call: &ExprCall) -> Option<(String, Option<String>)> {
    let helper = direct_register_helper(&call.func)?;
    if !FILTER_DECORATORS.contains(&helper) {
        return None;
    }

    let name_override = kw_name_from(&call.arguments.keywords);
    let keyword_func = kw_callable_name(&call.arguments.keywords, &["filter_func", "func"]);
    match &call.arguments.args[..] {
        [name, callable] => {
            let name = name.string_literal()?.to_string();
            Some((name, callable_name(callable).or(keyword_func)))
        }
        [name] if name.string_literal().is_some() => {
            let name = name.string_literal()?.to_string();
            let func_name = keyword_func?;
            Some((name, Some(func_name)))
        }
        [callable] if name_override.is_none() => {
            let func_name = callable_name(callable)?;
            Some((func_name.clone(), Some(func_name)))
        }
        [] => {
            let func_name = keyword_func?;
            let name = name_override.unwrap_or_else(|| func_name.clone());
            Some((name, Some(func_name)))
        }
        _ => None,
    }
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
            return Some(s.to_string());
        }
    }
    None
}

/// Extract a callable name from keyword arguments by checking the given keyword names.
fn kw_callable_name(keywords: &[Keyword], kwarg_names: &[&str]) -> Option<String> {
    for kw in keywords {
        let Some(arg) = &kw.arg else { continue };
        if kwarg_names.contains(&arg.as_str())
            && let Some(name) = kw.value.name_target()
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Extract the first positional argument's string value.
fn first_string_arg(args: &[Expr]) -> Option<String> {
    args.first()
        .and_then(ExprExt::string_literal)
        .map(str::to_string)
}

/// Best-effort callable name extraction for debugging / registration mapping.
fn callable_name(expr: &Expr) -> Option<String> {
    if let Some(name) = expr.name_target() {
        return Some(name.to_string());
    }

    match expr {
        Expr::Attribute(ExprAttribute { value, attr, .. }) => {
            let base = callable_name(value)?;
            Some(format!("{base}.{}", attr.as_str()))
        }
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Compare(_)
        | Expr::Call(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Subscript(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryDefinitionState {
    Failed,
    ParsedNotLibrary {
        parse_quality: TemplateLibraryParseQuality,
    },
    Library {
        parse_quality: TemplateLibraryParseQuality,
        inventory: TemplateLibrarySymbolInventory,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateLibrarySymbolInventory {
    Observed,
    Open,
}

/// Equality-bearing registration facts for one Template Library source module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraryDefinitionFacts {
    state: TemplateLibraryDefinitionState,
    tags: BTreeMap<String, TemplateSymbol>,
    filters: BTreeMap<String, TemplateSymbol>,
}

impl TemplateLibraryDefinitionFacts {
    #[must_use]
    pub fn is_library(&self) -> bool {
        matches!(self.state, TemplateLibraryDefinitionState::Library { .. })
    }

    #[must_use]
    pub(crate) fn is_recovered(&self) -> bool {
        matches!(
            self.state,
            TemplateLibraryDefinitionState::ParsedNotLibrary {
                parse_quality: TemplateLibraryParseQuality::Recovered,
            } | TemplateLibraryDefinitionState::Library {
                parse_quality: TemplateLibraryParseQuality::Recovered,
                ..
            }
        )
    }

    #[must_use]
    pub(crate) fn source_failed(&self) -> bool {
        matches!(self.state, TemplateLibraryDefinitionState::Failed)
    }

    #[must_use]
    pub(crate) fn symbols_are_unobserved(&self) -> bool {
        matches!(
            self.state,
            TemplateLibraryDefinitionState::Library {
                inventory: TemplateLibrarySymbolInventory::Open,
                ..
            }
        )
    }

    pub(crate) fn symbols(&self) -> impl Iterator<Item = &TemplateSymbol> {
        self.tags.values().chain(self.filters.values())
    }

    #[must_use]
    pub fn symbol(&self, kind: TemplateSymbolKind, name: &str) -> Option<&TemplateSymbol> {
        match kind {
            TemplateSymbolKind::Tag => self.tags.get(name),
            TemplateSymbolKind::Filter => self.filters.get(name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateLibraryParseQuality {
    Exact,
    Recovered,
}

/// Canonical indexed analysis of one Template Library source module.
///
/// Registration discovery happens here once. Equality-bearing projections below keep changes in
/// Tag Definitions, Filter Definitions, Tag Rules, Block Specs, and Filter Arity independent.
#[derive(Clone, Debug, PartialEq)]
struct TemplateLibrarySourceAnalysis {
    definitions: TemplateLibraryDefinitionFacts,
    tag_rules: TagRuleMap,
    block_specs: BlockSpecs,
    filter_arities: FilterArityMap,
}

impl TemplateLibrarySourceAnalysis {
    fn failed() -> Self {
        Self {
            definitions: TemplateLibraryDefinitionFacts {
                state: TemplateLibraryDefinitionState::Failed,
                tags: BTreeMap::new(),
                filters: BTreeMap::new(),
            },
            tag_rules: TagRuleMap::default(),
            block_specs: BlockSpecs::default(),
            filter_arities: FilterArityMap::default(),
        }
    }
}

#[salsa::tracked(returns(ref))]
fn template_library_source_analysis(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibrarySourceAnalysis {
    let Some(file) = key.file(db) else {
        return TemplateLibrarySourceAnalysis::failed();
    };
    let Ok(Some(module)) = RecoveredPythonModule::from_file(db, file) else {
        return TemplateLibrarySourceAnalysis::failed();
    };
    let parse_quality = if module.has_ordinary_syntax_errors(db) {
        TemplateLibraryParseQuality::Recovered
    } else {
        TemplateLibraryParseQuality::Exact
    };

    let mut tags = BTreeMap::new();
    let mut filters = BTreeMap::new();
    let mut tag_rules = TagRuleMap::default();
    let mut block_specs = BlockSpecs::default();
    let mut filter_arities = FilterArityMap::default();
    let registration_module = key.module(db).as_str();
    let registration_analysis =
        analyze_registrations_from_body_in_module(module.body(db), registration_module);
    let mut symbols_unobserved = parse_quality == TemplateLibraryParseQuality::Recovered
        || registration_analysis.inventory_open;

    for_each_registration(
        &registration_analysis,
        module.body(db),
        registration_module,
        |registration, func, symbol_key| {
            if let Ok(name) = TemplateSymbolName::parse(&registration.name) {
                let kind = registration.kind.symbol_kind();
                let symbol = TemplateSymbol {
                    kind,
                    name,
                    definition: SymbolDefinition::Exact {
                        file: file.path(db).to_path_buf(),
                    },
                    doc: None,
                };
                match kind {
                    TemplateSymbolKind::Tag => {
                        tags.insert(symbol.name().to_string(), symbol);
                    }
                    TemplateSymbolKind::Filter => {
                        filters.insert(symbol.name().to_string(), symbol);
                    }
                }
            } else {
                symbols_unobserved = true;
            }

            let Some(func) = func else {
                return;
            };
            if let Some(rule) = registration.kind.extract_tag_rule(func) {
                tag_rules.insert(symbol_key.clone(), rule.into());
            }
            if let Some(block_spec) = registration.kind.extract_block_spec(func) {
                let end_tag = match block_spec.end_tag {
                    EndTagEvidence::Literal(end_tag) => Some(end_tag),
                    EndTagEvidence::SelfNamed => Some(format!("end{}", symbol_key.name)),
                    EndTagEvidence::Unknown => None,
                };
                block_specs.insert(
                    symbol_key.clone(),
                    BlockSpec {
                        end_tag,
                        intermediates: block_spec.intermediates,
                        opaque: block_spec.opaque,
                    },
                );
            }
            if let Some(arity) = registration.kind.extract_filter_arity(func) {
                filter_arities.insert(symbol_key, arity);
            }
        },
    );

    let state = if registration_analysis.defines_library || !tags.is_empty() || !filters.is_empty()
    {
        let inventory = if symbols_unobserved {
            TemplateLibrarySymbolInventory::Open
        } else {
            TemplateLibrarySymbolInventory::Observed
        };
        TemplateLibraryDefinitionState::Library {
            parse_quality,
            inventory,
        }
    } else {
        TemplateLibraryDefinitionState::ParsedNotLibrary { parse_quality }
    };
    TemplateLibrarySourceAnalysis {
        definitions: TemplateLibraryDefinitionFacts {
            state,
            tags,
            filters,
        },
        tag_rules,
        block_specs,
        filter_arities,
    }
}

/// Independently backdatable Tag analysis for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemplateLibraryTagFacts {
    tag_rules: TagRuleMap,
    block_specs: BlockSpecs,
}

impl TemplateLibraryTagFacts {
    #[must_use]
    pub fn tag_rules(&self) -> &TagRuleMap {
        &self.tag_rules
    }

    #[must_use]
    pub fn block_specs(&self) -> &BlockSpecs {
        &self.block_specs
    }
}

/// Independently backdatable Filter analysis for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemplateLibraryFilterFacts {
    filter_arities: FilterArityMap,
}

impl TemplateLibraryFilterFacts {
    #[must_use]
    pub fn filter_arities(&self) -> &FilterArityMap {
        &self.filter_arities
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_library_definition_facts(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibraryDefinitionFacts {
    template_library_source_analysis(db, key)
        .definitions
        .clone()
}

#[salsa::tracked(returns(ref))]
pub fn template_library_tag_facts(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibraryTagFacts {
    let analysis = template_library_source_analysis(db, key);
    TemplateLibraryTagFacts {
        tag_rules: analysis.tag_rules.clone(),
        block_specs: analysis.block_specs.clone(),
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_library_filter_facts(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibraryFilterFacts {
    TemplateLibraryFilterFacts {
        filter_arities: template_library_source_analysis(db, key)
            .filter_arities
            .clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::tags::testing::fixture_source;

    fn fixture(path: &str) -> &'static str {
        fixture_source(path).expect("requested corpus fixture should exist")
    }

    fn analyze_registrations(source: &str) -> RegistrationSourceAnalysis {
        let parsed = ruff_python_parser::parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        analyze_registrations_from_body(&module.body)
    }

    fn collect_registrations(source: &str) -> Vec<RegistrationInfo> {
        analyze_registrations(source).registrations
    }

    fn find_reg<'a>(regs: &'a [RegistrationInfo], name: &str) -> &'a RegistrationInfo {
        regs.iter()
            .find(|r| r.name == name)
            .expect("requested registration should have been collected")
    }

    // Corpus: `autoescape` in django/template/defaulttags.py uses `@register.tag` (bare)
    #[test]
    fn decorator_bare_tag() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "autoescape");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("autoescape"));
    }

    // Corpus: `querystring` in django/template/defaulttags.py uses
    // `@register.simple_tag(name="querystring", takes_context=True)`
    #[test]
    fn decorator_simple_tag_with_name_kwarg() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "querystring");
        assert_eq!(reg.kind, RegistrationKind::SimpleTag);
        assert_eq!(reg.func_name.as_deref(), Some("querystring"));
    }

    // Corpus: `inclusion_no_params` in tests/template_tests/templatetags/inclusion.py uses
    // `@register.inclusion_tag("inclusion.html")`
    #[test]
    fn decorator_inclusion_tag() {
        let source = fixture("tests/template_tests/templatetags/inclusion.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "inclusion_no_params");
        assert_eq!(reg.kind, RegistrationKind::InclusionTag);
    }

    // Corpus: `cut` in django/template/defaultfilters.py uses `@register.filter` (bare)
    #[test]
    fn decorator_filter_bare() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "cut");
        assert_eq!(reg.kind, RegistrationKind::Filter);
    }

    // Corpus: `escapejs` in django/template/defaultfilters.py uses
    // `@register.filter("escapejs")` — positional string name, func is `escapejs_filter`
    #[test]
    fn decorator_filter_with_positional_string_name() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "escapejs");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name.as_deref(), Some("escapejs_filter"));
    }

    // Corpus: `other_echo` in tests/template_tests/templatetags/testtags.py uses
    // `register.tag("other_echo", echo)` — call-style registration
    #[test]
    fn call_style_tag_registration() {
        let source = fixture("tests/template_tests/templatetags/testtags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "other_echo");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("echo"));
    }

    // Corpus: `intcomma` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
    // `register.filter("intcomma", intcomma)` — call-style filter registration
    #[test]
    fn call_style_filter_registration() {
        let source = fixture("wagtail/admin/templatetags/wagtailadmin_tags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "intcomma");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name.as_deref(), Some("intcomma"));
    }

    // Corpus: `for` in django/template/defaulttags.py uses `@register.tag("for")`
    // — positional string name overrides function name `do_for`
    #[test]
    fn tag_with_positional_string_name() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "for");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("do_for"));
    }

    // Corpus: `addslashes` in django/template/defaultfilters.py uses
    // `@register.filter(is_safe=True)` — name defaults to function name
    #[test]
    fn filter_with_is_safe_kwarg() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "addslashes");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name.as_deref(), Some("addslashes"));
    }

    // Corpus: `partialdef` in django/template/defaulttags.py uses
    // `@register.tag(name="partialdef")` — name kwarg overrides func name `partialdef_func`
    #[test]
    fn tag_with_name_kwarg() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "partialdef");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("partialdef_func"));
    }

    // Corpus: `dialog` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
    // `register.tag("dialog", DialogNode.handle)` — call-style with method callable
    #[test]
    fn call_style_tag_with_method_callable() {
        let source = fixture("wagtail/admin/templatetags/wagtailadmin_tags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "dialog");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("DialogNode.handle"));
    }

    // Corpus: `div` in tests/template_tests/templatetags/custom.py uses
    // `@register.simple_block_tag` (bare decorator)
    #[test]
    fn simple_block_tag_decorator() {
        let source = fixture("tests/template_tests/templatetags/custom.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "div");
        assert_eq!(reg.kind, RegistrationKind::SimpleBlockTag);
    }

    // Corpus: defaulttags.py has many registrations (tags + simple_tags)
    #[test]
    fn multiple_registrations() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
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
        let source = fixture("tests/template_tests/templatetags/testtags.py");
        let regs = collect_registrations(source);
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

    #[test]
    fn imported_register_preserves_known_symbols_but_opens_inventory() {
        let analysis = analyze_registrations(
            "from shared import register\n@register.simple_tag\ndef known(): pass\n@register.filter\ndef known_filter(value): return value\n",
        );
        assert!(analysis.defines_library);
        assert!(analysis.inventory_open);
        assert_eq!(analysis.registrations.len(), 2);
    }

    #[test]
    fn only_unshadowed_canonical_constructor_is_closed() {
        let canonical = analyze_registrations(
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known(): pass\n",
        );
        assert!(!canonical.inventory_open);
        let direct_import =
            analyze_registrations("from django.template import Library\nregister = Library()\n");
        assert!(!direct_import.inventory_open);

        for source in [
            "register = Library()\n",
            "from django import template as dt\nregister = dt.Library()\n",
            "import django.template as template\nregister = template.Library()\n",
            "from django import template\nalias = register = template.Library()\n",
            "from django import template\nimport other as template\nregister = template.Library()\n",
            "from django import template\nregister = template.Library()\nfrom shared import register\n",
        ] {
            assert!(
                analyze_registrations(source).inventory_open,
                "constructor should remain open: {source}"
            );
        }
    }

    #[test]
    fn uncertain_scopes_open_inventory_without_contributing_symbols() {
        for uncertain in [
            "if enabled:\n    @register.simple_tag\n    def conditional(): pass",
            "class Helpers:\n    register = template.Library()\n    @register.simple_tag\n    def class_local(): pass",
        ] {
            let source = format!(
                "from django import template\nregister = template.Library()\n{uncertain}\n"
            );
            let analysis = analyze_registrations(&source);
            assert!(analysis.inventory_open);
            assert!(analysis.registrations.is_empty());
        }
    }

    #[test]
    fn nested_helper_mutation_opens_inventory() {
        let analysis = analyze_registrations(
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known(): pass\ndef configure():\n    register.tags.update(dynamic_tags)\n",
        );
        assert!(analysis.inventory_open);
        assert_eq!(analysis.registrations.len(), 1);
        assert_eq!(analysis.registrations[0].name, "known");
    }

    #[test]
    fn mixed_positional_keyword_calls_are_known_and_closed() {
        let analysis = analyze_registrations(
            "from django import template\nregister = template.Library()\nregister.tag('known', compile_function=tag_func)\nregister.filter('known_filter', filter_func=filter_func)\n",
        );
        assert!(!analysis.inventory_open);
        assert!(
            analysis
                .registrations
                .iter()
                .any(|registration| registration.name == "known")
        );
        assert!(
            analysis
                .registrations
                .iter()
                .any(|registration| registration.name == "known_filter")
        );
    }

    #[test]
    fn only_register_root_contributes_symbols() {
        let analysis = analyze_registrations(
            "from django import template\nregister = template.Library()\n@other.tag\ndef invented(parser, token): pass\nother.filter('also_invented', func)\n",
        );
        assert!(analysis.registrations.is_empty());
        assert!(!analysis.inventory_open);
    }

    #[test]
    fn uncertain_operations_open_inventory_without_dropping_known_symbols() {
        for operation in [
            "@register.tag(name=dynamic_name)\ndef dynamic(parser, token): pass",
            "register.tags.update(dynamic_tags)",
            "register.tags['dynamic'] = func",
            "del register.filters['dynamic']",
            "register.tag()",
            "configure(register)",
        ] {
            let source = format!(
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known(): pass\n{operation}\n"
            );
            let analysis = analyze_registrations(&source);
            assert!(
                analysis.inventory_open,
                "operation should open inventory: {operation}"
            );
            assert!(
                analysis
                    .registrations
                    .iter()
                    .any(|registration| registration.name == "known")
            );
        }
    }
}
