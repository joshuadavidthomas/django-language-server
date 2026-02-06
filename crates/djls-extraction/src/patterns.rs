//! AST pattern matching utilities.
//!
//! This module provides helper functions for matching common Python AST patterns
//! during the extraction process.

use ruff_python_ast::Expr;
use ruff_python_ast::Number;

/// Check if an expression is `len(<target_name>)`.
///
/// Returns `Some(())` if the pattern matches, `None` otherwise.
pub fn is_len_call(expr: &Expr, target_name: &str) -> Option<()> {
    let Expr::Call(call) = expr else { return None };
    let Expr::Name(func_name) = call.func.as_ref() else { return None };
    if func_name.id.as_str() != "len" {
        return None;
    }

    let Some(Expr::Name(arg_name)) = call.arguments.args.first() else {
        return None;
    };
    if arg_name.id.as_str() != target_name {
        return None;
    }

    Some(())
}

/// Check if an expression is a simple name matching the target.
pub fn is_name(expr: &Expr, target: &str) -> bool {
    matches!(expr, Expr::Name(n) if n.id.as_str() == target)
}

/// Extract an integer literal from an expression.
pub fn extract_int_literal(expr: &Expr) -> Option<i64> {
    let Expr::NumberLiteral(num) = expr else { return None };
    match &num.value {
        Number::Int(i) => i.as_i64(),
        _ => None,
    }
}

/// Extract a string literal from an expression.
pub fn extract_string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(s) => Some(s.value.to_string()),
        _ => None,
    }
}

/// Extract a subscript index from an expression like `<name>[N]`.
///
/// Returns `(index, variable_name)` if the expression matches.
pub fn extract_subscript_index(expr: &Expr) -> Option<(usize, String)> {
    let Expr::Subscript(sub) = expr else { return None };
    let Expr::Name(name) = sub.value.as_ref() else { return None };

    let idx = extract_int_literal(&sub.slice)?;
    if idx < 0 {
        return None;
    }

    Some((idx as usize, name.id.to_string()))
}

/// Extract a tuple of string literals from an expression like `("opt1", "opt2")`.
pub fn extract_string_tuple(expr: &Expr) -> Option<Vec<String>> {
    let Expr::Tuple(tuple) = expr else { return None };

    let mut strings = Vec::new();
    for elt in &tuple.elts {
        strings.push(extract_string_literal(elt)?);
    }

    Some(strings)
}
