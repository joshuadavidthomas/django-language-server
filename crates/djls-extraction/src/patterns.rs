use ruff_python_ast::Expr;
use ruff_python_ast::Number;

/// Check if `expr` is `len(<target_name>)`.
pub fn is_len_call(expr: &Expr, target_name: &str) -> Option<()> {
    let Expr::Call(call) = expr else { return None };
    let Expr::Name(func_name) = call.func.as_ref() else {
        return None;
    };
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

/// Check if `expr` is a `Name` node matching `target`.
pub fn is_name(expr: &Expr, target: &str) -> bool {
    matches!(expr, Expr::Name(n) if n.id.as_str() == target)
}

/// Extract an integer literal value from an expression.
pub fn extract_int_literal(expr: &Expr) -> Option<i64> {
    let Expr::NumberLiteral(num) = expr else {
        return None;
    };
    match &num.value {
        Number::Int(i) => i.as_i64(),
        _ => None,
    }
}

/// Extract a string literal value from an expression.
pub fn extract_string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(s) => Some(s.value.to_string()),
        _ => None,
    }
}

/// Extract subscript index and variable name from `<name>[N]`.
///
/// Returns `(index, variable_name)` if the expression is a subscript
/// with a non-negative integer index.
pub fn extract_subscript_index(expr: &Expr) -> Option<(usize, String)> {
    let Expr::Subscript(sub) = expr else {
        return None;
    };
    let Expr::Name(name) = sub.value.as_ref() else {
        return None;
    };

    let idx = extract_int_literal(&sub.slice)?;
    if idx < 0 {
        return None;
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((idx as usize, name.id.to_string()))
}

/// Extract a tuple of string literals from an expression.
///
/// Returns `None` if the expression is not a tuple or contains non-string elements.
pub fn extract_string_tuple(expr: &Expr) -> Option<Vec<String>> {
    let Expr::Tuple(tuple) = expr else {
        return None;
    };

    let mut strings = Vec::new();
    for elt in &tuple.elts {
        strings.push(extract_string_literal(elt)?);
    }

    Some(strings)
}
