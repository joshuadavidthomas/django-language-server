use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::specs::blocks::dynamic_end;
use crate::specs::blocks::is_token_contents_expr;
use crate::specs::types::BlockSpec;

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
pub(super) fn detect(body: &[Stmt], parser_var: &str, token_var: &str) -> Option<BlockSpec> {
    if !has_next_token_loop(body, parser_var) {
        return None;
    }

    let token_comparisons = collect_token_content_comparisons(body, token_var);
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
    let mut found = false;
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        if let Stmt::While(while_stmt) = stmt
            && is_parser_tokens_check(&while_stmt.test, parser_var)
            && body_has_next_token_call(&while_stmt.body, parser_var)
        {
            found = true;
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    found
}

/// Check if an expression is `parser.tokens` (the token list attribute).
fn is_parser_tokens_check(expr: &Expr, parser_var: &str) -> bool {
    if let Expr::Attribute(ExprAttribute { attr, value, .. }) = expr
        && attr.as_str() == "tokens"
    {
        return value.name_target() == Some(parser_var);
    }
    false
}

/// Check if a body contains a `parser.next_token()` call.
fn body_has_next_token_call(body: &[Stmt], parser_var: &str) -> bool {
    let mut found = false;
    walk_stmts(body, Recurse::Flat, |stmt| {
        let has_next_token_call = match stmt {
            Stmt::Assign(StmtAssign { value, .. }) => is_next_token_call(value, parser_var),
            Stmt::Expr(expr_stmt) => is_next_token_call(&expr_stmt.value, parser_var),
            _ => false,
        };
        if has_next_token_call {
            found = true;
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    found
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
    value.name_target() == Some(parser_var)
}

/// Collect string literals compared against `token.contents` in a body.
///
/// Looks for patterns like:
/// - `token.contents.strip() != "plural"`
/// - `token.contents == "endblocktrans"`
/// - `token.contents.strip() != end_tag_name` (skipped — dynamic)
fn collect_token_content_comparisons(body: &[Stmt], token_var: &str) -> Vec<String> {
    let mut comparisons = Vec::new();
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        match stmt {
            Stmt::If(if_stmt) => {
                for value in extract_comparisons_from_expr(&if_stmt.test, token_var) {
                    if !comparisons.contains(&value) {
                        comparisons.push(value);
                    }
                }
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        for value in extract_comparisons_from_expr(test, token_var) {
                            if !comparisons.contains(&value) {
                                comparisons.push(value);
                            }
                        }
                    }
                }
            }
            Stmt::While(while_stmt) => {
                for value in extract_comparisons_from_expr(&while_stmt.test, token_var) {
                    if !comparisons.contains(&value) {
                        comparisons.push(value);
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    });
    comparisons
}

/// Extract string comparisons against token.contents from a comparison expression.
fn extract_comparisons_from_expr(expr: &Expr, token_var: &str) -> Vec<String> {
    let mut comparisons = Vec::new();
    if let Expr::Compare(compare) = expr {
        let operands: Vec<&Expr> = std::iter::once(compare.left.as_ref())
            .chain(compare.comparators.iter())
            .collect();

        for window in operands.windows(2) {
            let left = window[0];
            let right = window[1];

            if is_token_contents_expr(left, Some(token_var))
                || is_token_contents_expr(right, Some(token_var))
            {
                if let Some(s) = left.string_literal()
                    && !comparisons.iter().any(|comparison| comparison == s)
                {
                    comparisons.push(s.to_string());
                }
                if let Some(s) = right.string_literal()
                    && !comparisons.iter().any(|comparison| comparison == s)
                {
                    comparisons.push(s.to_string());
                }
            }
        }
    }
    comparisons
}
