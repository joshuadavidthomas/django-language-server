use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use super::is_parser_receiver;
use crate::ext::ExprExt;
use crate::types::BlockTagSpec;

/// Detect opaque block patterns: `parser.skip_past("endtag")`.
pub fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockTagSpec> {
    let skip_past_tokens = collect_skip_past_tokens(body, parser_var);
    if skip_past_tokens.is_empty() {
        return None;
    }
    let end_tag = if skip_past_tokens.len() == 1 {
        Some(skip_past_tokens[0].clone())
    } else {
        None
    };
    Some(BlockTagSpec {
        end_tag,
        intermediates: Vec::new(),
        opaque: true,
    })
}

/// Collect all `parser.skip_past("token")` calls in a statement body (recursively).
fn collect_skip_past_tokens(body: &[Stmt], parser_var: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for stmt in body {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if let Some(t) = extract_skip_past_token(&expr_stmt.value, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                if let Some(t) = extract_skip_past_token(value, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            Stmt::If(if_stmt) => {
                for t in collect_skip_past_tokens(&if_stmt.body, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
                for clause in &if_stmt.elif_else_clauses {
                    for t in collect_skip_past_tokens(&clause.body, parser_var) {
                        if !tokens.contains(&t) {
                            tokens.push(t);
                        }
                    }
                }
            }
            Stmt::For(for_stmt) => {
                for t in collect_skip_past_tokens(&for_stmt.body, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            Stmt::While(while_stmt) => {
                for t in collect_skip_past_tokens(&while_stmt.body, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            Stmt::Try(try_stmt) => {
                for t in collect_skip_past_tokens(&try_stmt.body, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            _ => {}
        }
    }
    tokens
}

/// Check if an expression is `parser.skip_past("token")` and extract the token.
fn extract_skip_past_token(expr: &Expr, parser_var: &str) -> Option<String> {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return None;
    };
    let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = func.as_ref()
    else {
        return None;
    };
    if attr.as_str() != "skip_past" {
        return None;
    }
    if !is_parser_receiver(obj, parser_var) {
        return None;
    }
    if arguments.args.is_empty() {
        return None;
    }
    arguments.args[0].string_literal()
}
