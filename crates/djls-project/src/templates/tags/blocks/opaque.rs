use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::blocks::EndTagEvidence;
use crate::templates::tags::blocks::ExtractedBlockSpec;
use crate::templates::tags::blocks::is_parser_receiver;

/// Detect opaque block patterns: `parser.skip_past("endtag")`.
pub(super) fn detect(body: &[Stmt], parser_var: &str) -> Option<ExtractedBlockSpec> {
    let skip_past_tokens = collect_skip_past_tokens(body, parser_var);
    if skip_past_tokens.is_empty() {
        return None;
    }
    let end_tag = if skip_past_tokens.len() == 1 {
        EndTagEvidence::Literal(skip_past_tokens[0].clone())
    } else {
        EndTagEvidence::Unknown
    };
    Some(ExtractedBlockSpec {
        end_tag,
        intermediates: Vec::new(),
        opaque: true,
    })
}

/// Collect all `parser.skip_past("token")` calls in a statement body.
///
/// Uses Ruff's statement visitor to avoid hand-written recursion across
/// statement variants.
fn collect_skip_past_tokens(body: &[Stmt], parser_var: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    walk_stmts(body, Recurse::WithinScope, |stmt| {
        let token = match stmt {
            Stmt::Expr(expr_stmt) => extract_skip_past_token(&expr_stmt.value, parser_var),
            Stmt::Assign(StmtAssign { value, .. }) => extract_skip_past_token(value, parser_var),
            _ => None,
        };
        if let Some(token) = token
            && !tokens.contains(&token)
        {
            tokens.push(token);
        }
        ControlFlow::Continue(())
    });
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
    arguments.args[0].string_literal().map(str::to_string)
}
