use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use super::dynamic_end;
use super::is_token_contents_expr;
use crate::ext::ExprExt;
use crate::types::BlockSpec;

/// Detect block structure from `parser.next_token()` loop patterns.
///
/// Handles tags like `blocktrans`/`blocktranslate` that manually iterate
/// tokens instead of using `parser.parse((...))`. The pattern is:
///
/// ```python
/// while parser.tokens:
///     token = parser.next_token()
///     if token.token_type in (...):
///         singular.append(token)
///     else:
///         break
/// # check for intermediate: token.contents.strip() != "plural"
/// # ...
/// end_tag_name = "end%s" % bits[0]
/// if token.contents.strip() != end_tag_name:
///     raise TemplateSyntaxError(...)
/// ```
pub fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockSpec> {
    if !has_next_token_loop(body, parser_var) {
        return None;
    }

    let token_comparisons = collect_token_content_comparisons(body);
    let has_dynamic_end = dynamic_end::has_dynamic_end_tag_format(body);

    if token_comparisons.is_empty() && !has_dynamic_end {
        return None;
    }

    let mut intermediates = Vec::new();
    let mut end_tag = None;

    for token in &token_comparisons {
        if token.starts_with("end") {
            end_tag = Some(token.clone());
        } else {
            intermediates.push(token.clone());
        }
    }

    if end_tag.is_none() && !has_dynamic_end && intermediates.is_empty() {
        return None;
    }

    intermediates.sort();

    Some(BlockSpec {
        end_tag,
        intermediates,
        opaque: false,
    })
}

/// Check if a body contains `while parser.tokens:` with `parser.next_token()`.
fn has_next_token_loop(body: &[Stmt], parser_var: &str) -> bool {
    for stmt in body {
        match stmt {
            Stmt::While(while_stmt) => {
                if is_parser_tokens_check(&while_stmt.test, parser_var)
                    && body_has_next_token_call(&while_stmt.body, parser_var)
                {
                    return true;
                }
                if has_next_token_loop(&while_stmt.body, parser_var)
                    || has_next_token_loop(&while_stmt.orelse, parser_var)
                {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if has_next_token_loop(&if_stmt.body, parser_var) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if has_next_token_loop(&clause.body, parser_var) {
                        return true;
                    }
                }
            }
            Stmt::For(for_stmt) => {
                if has_next_token_loop(&for_stmt.body, parser_var)
                    || has_next_token_loop(&for_stmt.orelse, parser_var)
                {
                    return true;
                }
            }
            Stmt::Try(try_stmt) => {
                if has_next_token_loop(&try_stmt.body, parser_var) {
                    return true;
                }
                for handler in &try_stmt.handlers {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                    if has_next_token_loop(&h.body, parser_var) {
                        return true;
                    }
                }
                if has_next_token_loop(&try_stmt.orelse, parser_var)
                    || has_next_token_loop(&try_stmt.finalbody, parser_var)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Check if an expression is `parser.tokens` (the token list attribute).
fn is_parser_tokens_check(expr: &Expr, parser_var: &str) -> bool {
    if let Expr::Attribute(ExprAttribute { attr, value, .. }) = expr {
        if attr.as_str() == "tokens" {
            if let Expr::Name(ExprName { id, .. }) = value.as_ref() {
                return id.as_str() == parser_var;
            }
        }
    }
    false
}

/// Check if a body contains a `parser.next_token()` call.
fn body_has_next_token_call(body: &[Stmt], parser_var: &str) -> bool {
    for stmt in body {
        match stmt {
            Stmt::Assign(StmtAssign { value, .. }) => {
                if is_next_token_call(value, parser_var) {
                    return true;
                }
            }
            Stmt::Expr(expr_stmt) => {
                if is_next_token_call(&expr_stmt.value, parser_var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Check if an expression is `parser.next_token()`.
fn is_next_token_call(expr: &Expr, parser_var: &str) -> bool {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return false;
    };
    if !arguments.args.is_empty() {
        return false;
    }
    let Expr::Attribute(ExprAttribute { attr, value, .. }) = func.as_ref() else {
        return false;
    };
    if attr.as_str() != "next_token" {
        return false;
    }
    if let Expr::Name(ExprName { id, .. }) = value.as_ref() {
        return id.as_str() == parser_var;
    }
    false
}

/// Collect string literals compared against `token.contents` in a body.
///
/// Looks for patterns like:
/// - `token.contents.strip() != "plural"`
/// - `token.contents == "endblocktrans"`
/// - `token.contents.strip() != end_tag_name` (skipped — dynamic)
fn collect_token_content_comparisons(body: &[Stmt]) -> Vec<String> {
    let mut comparisons = Vec::new();
    for stmt in body {
        match stmt {
            Stmt::If(if_stmt) => {
                for s in extract_comparisons_from_expr(&if_stmt.test) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                for s in collect_token_content_comparisons(&if_stmt.body) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        for s in extract_comparisons_from_expr(test) {
                            if !comparisons.contains(&s) {
                                comparisons.push(s);
                            }
                        }
                    }
                    for s in collect_token_content_comparisons(&clause.body) {
                        if !comparisons.contains(&s) {
                            comparisons.push(s);
                        }
                    }
                }
            }
            Stmt::While(while_stmt) => {
                for s in extract_comparisons_from_expr(&while_stmt.test) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                for s in collect_token_content_comparisons(&while_stmt.body) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                for s in collect_token_content_comparisons(&while_stmt.orelse) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
            }
            Stmt::For(for_stmt) => {
                for s in collect_token_content_comparisons(&for_stmt.body) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                for s in collect_token_content_comparisons(&for_stmt.orelse) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
            }
            Stmt::Try(try_stmt) => {
                for s in collect_token_content_comparisons(&try_stmt.body) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                for handler in &try_stmt.handlers {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                    for s in collect_token_content_comparisons(&h.body) {
                        if !comparisons.contains(&s) {
                            comparisons.push(s);
                        }
                    }
                }
                for s in collect_token_content_comparisons(&try_stmt.orelse) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                for s in collect_token_content_comparisons(&try_stmt.finalbody) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
            }
            _ => {}
        }
    }
    comparisons
}

/// Extract string comparisons against token.contents from a comparison expression.
fn extract_comparisons_from_expr(expr: &Expr) -> Vec<String> {
    let mut comparisons = Vec::new();
    if let Expr::Compare(compare) = expr {
        let operands: Vec<&Expr> = std::iter::once(compare.left.as_ref())
            .chain(compare.comparators.iter())
            .collect();

        for window in operands.windows(2) {
            let left = window[0];
            let right = window[1];

            if is_token_contents_expr(left) || is_token_contents_expr(right) {
                if let Some(s) = left.string_literal() {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                if let Some(s) = right.string_literal() {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
            }
        }
    }
    comparisons
}
