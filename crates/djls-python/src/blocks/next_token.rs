use ruff_python_ast::statement_visitor::walk_stmt;
use ruff_python_ast::statement_visitor::StatementVisitor;
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
    let mut visitor = NextTokenLoopFinder::new(parser_var);
    visitor.visit_body(body);
    visitor.found
}

struct NextTokenLoopFinder<'a> {
    parser_var: &'a str,
    found: bool,
}

impl<'a> NextTokenLoopFinder<'a> {
    fn new(parser_var: &'a str) -> Self {
        Self {
            parser_var,
            found: false,
        }
    }
}

impl StatementVisitor<'_> for NextTokenLoopFinder<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if self.found {
            return;
        }

        match stmt {
            Stmt::While(while_stmt) => {
                if is_parser_tokens_check(&while_stmt.test, self.parser_var)
                    && body_has_next_token_call(&while_stmt.body, self.parser_var)
                {
                    self.found = true;
                    return;
                }
                walk_stmt(self, stmt);
            }
            // Recurse into control flow to find all possible loop patterns.
            Stmt::If(_) | Stmt::For(_) | Stmt::Try(_) => walk_stmt(self, stmt),
            _ => {}
        }
    }
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
    let mut visitor = NextTokenCallFinder::new(parser_var);
    visitor.visit_body(body);
    visitor.found
}

struct NextTokenCallFinder<'a> {
    parser_var: &'a str,
    found: bool,
}

impl<'a> NextTokenCallFinder<'a> {
    fn new(parser_var: &'a str) -> Self {
        Self {
            parser_var,
            found: false,
        }
    }
}

impl StatementVisitor<'_> for NextTokenCallFinder<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if self.found {
            return;
        }

        match stmt {
            Stmt::Assign(StmtAssign { value, .. }) => {
                self.found = is_next_token_call(value, self.parser_var);
            }
            Stmt::Expr(expr_stmt) => {
                self.found = is_next_token_call(&expr_stmt.value, self.parser_var);
            }
            _ => {}
        }
    }
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
fn collect_token_content_comparisons(body: &[Stmt], token_var: &str) -> Vec<String> {
    let mut visitor = TokenComparisonVisitor::new(token_var);
    visitor.visit_body(body);
    visitor.comparisons
}

struct TokenComparisonVisitor<'a> {
    token_var: &'a str,
    comparisons: Vec<String>,
}

impl<'a> TokenComparisonVisitor<'a> {
    fn new(token_var: &'a str) -> Self {
        Self {
            token_var,
            comparisons: Vec::new(),
        }
    }

    fn add_all(&mut self, values: Vec<String>) {
        for value in values {
            if !self.comparisons.contains(&value) {
                self.comparisons.push(value);
            }
        }
    }
}

impl StatementVisitor<'_> for TokenComparisonVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::If(if_stmt) => {
                self.add_all(extract_comparisons_from_expr(&if_stmt.test, self.token_var));
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.add_all(extract_comparisons_from_expr(test, self.token_var));
                    }
                }
                walk_stmt(self, stmt);
            }
            Stmt::While(while_stmt) => {
                self.add_all(extract_comparisons_from_expr(&while_stmt.test, self.token_var));
                walk_stmt(self, stmt);
            }
            // Recurse into control flow to find all possible loop patterns.
            Stmt::For(_) | Stmt::Try(_) => walk_stmt(self, stmt),
            _ => {}
        }
    }
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
