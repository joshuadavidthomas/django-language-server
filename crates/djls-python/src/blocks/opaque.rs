use ruff_python_ast::statement_visitor::walk_stmt;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use super::is_parser_receiver;
use crate::ext::ExprExt;
use crate::types::BlockSpec;

/// Detect opaque block patterns: `parser.skip_past("endtag")`.
pub(super) fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockSpec> {
    let skip_past_tokens = collect_skip_past_tokens(body, parser_var);
    if skip_past_tokens.is_empty() {
        return None;
    }
    let end_tag = if skip_past_tokens.len() == 1 {
        Some(skip_past_tokens[0].clone())
    } else {
        None
    };
    Some(BlockSpec {
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
    let mut visitor = SkipPastVisitor::new(parser_var);
    visitor.visit_body(body);
    visitor.tokens
}

struct SkipPastVisitor<'a> {
    parser_var: &'a str,
    tokens: Vec<String>,
}

impl<'a> SkipPastVisitor<'a> {
    fn new(parser_var: &'a str) -> Self {
        Self {
            parser_var,
            tokens: Vec::new(),
        }
    }

    fn insert_token(&mut self, token: String) {
        if !self.tokens.contains(&token) {
            self.tokens.push(token);
        }
    }
}

impl StatementVisitor<'_> for SkipPastVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if let Some(token) = extract_skip_past_token(&expr_stmt.value, self.parser_var) {
                    self.insert_token(token);
                }
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                if let Some(token) = extract_skip_past_token(value, self.parser_var) {
                    self.insert_token(token);
                }
            }
            // Stay within the current function scope.
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }
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
