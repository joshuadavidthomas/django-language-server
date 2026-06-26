use std::marker::PhantomData;
use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprNumberLiteral;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::ExprUnaryOp;
use ruff_python_ast::Number;
use ruff_python_ast::Stmt;
use ruff_python_ast::UnaryOp;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::statement_visitor::walk_body;
use ruff_python_ast::statement_visitor::walk_stmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Recurse {
    /// Control-flow bodies only: If/For/While/Try/With/Match. Stops at nested fn/class scopes.
    ControlFlow,
    /// Everything except nested fn/class scopes.
    WithinScope,
    /// Only `ClassDef` bodies.
    IntoClasses,
    /// No recursion; scan the given body's direct statements only.
    Flat,
}

/// Run `matcher` on every statement in `body` pre-order, recursing per `policy`.
///
/// `matcher` returns `ControlFlow::Break(())` to stop the entire walk.
pub(crate) fn walk_stmts<'a>(
    body: &'a [Stmt],
    policy: Recurse,
    matcher: impl FnMut(&'a Stmt) -> ControlFlow<()>,
) {
    let mut walker = Walker {
        matcher,
        policy,
        stopped: false,
        _pd: PhantomData,
    };
    walker.visit_body(body);
}

struct Walker<'a, F> {
    matcher: F,
    policy: Recurse,
    stopped: bool,
    _pd: PhantomData<&'a ()>,
}

impl<'a, F> StatementVisitor<'a> for Walker<'a, F>
where
    F: FnMut(&'a Stmt) -> ControlFlow<()>,
{
    fn visit_body(&mut self, body: &'a [Stmt]) {
        if self.stopped {
            return;
        }
        walk_body(self, body);
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.stopped {
            return;
        }

        match (self.matcher)(stmt) {
            ControlFlow::Continue(()) => {}
            ControlFlow::Break(()) => {
                self.stopped = true;
                return;
            }
        }

        match self.policy {
            Recurse::ControlFlow => match stmt {
                Stmt::If(_)
                | Stmt::For(_)
                | Stmt::While(_)
                | Stmt::Try(_)
                | Stmt::With(_)
                | Stmt::Match(_) => walk_stmt(self, stmt),
                _ => {}
            },
            Recurse::WithinScope => match stmt {
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
                _ => walk_stmt(self, stmt),
            },
            Recurse::IntoClasses => {
                if let Stmt::ClassDef(_) = stmt {
                    walk_stmt(self, stmt);
                }
            }
            Recurse::Flat => {}
        }
    }
}

pub(crate) trait ExprExt {
    /// Extract the full string value from a string literal expression.
    fn string_literal(&self) -> Option<&str>;

    /// Extract the first whitespace-delimited word from a string literal.
    ///
    /// Django's `Parser.parse()` compares against
    /// `command = token.contents.split()[0]`, so only the first word of a
    /// stop-token string matters.
    fn string_literal_first_word(&self) -> Option<&str>;

    /// Extract the identifier from a name expression.
    fn name_target(&self) -> Option<&str>;

    /// Extract a boolean literal.
    fn bool_literal(&self) -> Option<bool>;

    /// Extract a non-negative integer literal as `usize`.
    fn non_negative_integer(&self) -> Option<usize>;

    /// Extract the magnitude from a negative integer literal.
    fn negative_integer(&self) -> Option<usize>;

    /// Map elements of a collection expression (tuple, list, or set) through
    /// a fallible function. Returns `None` if the expression is not a
    /// collection or if any element mapping fails.
    fn collection_map<T>(&self, f: impl Fn(&Expr) -> Option<T>) -> Option<Vec<T>>;
}

impl ExprExt for Expr {
    fn string_literal(&self) -> Option<&str> {
        if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = self {
            return Some(value.to_str());
        }
        None
    }

    fn string_literal_first_word(&self) -> Option<&str> {
        if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = self {
            let s = value.to_str();
            let cmd = s.split_whitespace().next().unwrap_or("");
            if cmd.is_empty() {
                return None;
            }
            return Some(cmd);
        }
        None
    }

    fn name_target(&self) -> Option<&str> {
        if let Expr::Name(name) = self {
            return Some(name.id.as_str());
        }
        None
    }

    fn bool_literal(&self) -> Option<bool> {
        match self {
            Expr::BooleanLiteral(literal) => Some(literal.value),
            _ => None,
        }
    }

    fn non_negative_integer(&self) -> Option<usize> {
        let Expr::NumberLiteral(ExprNumberLiteral { value, .. }) = self else {
            return None;
        };
        let Number::Int(value) = value else {
            return None;
        };
        usize::try_from(value.as_i64()?).ok()
    }

    fn negative_integer(&self) -> Option<usize> {
        let Expr::UnaryOp(ExprUnaryOp {
            op: UnaryOp::USub,
            operand,
            ..
        }) = self
        else {
            return None;
        };
        operand.non_negative_integer()
    }

    fn collection_map<T>(&self, f: impl Fn(&Expr) -> Option<T>) -> Option<Vec<T>> {
        let elements = match self {
            Expr::Tuple(t) => &t.elts,
            Expr::List(l) => &l.elts,
            Expr::Set(s) => &s.elts,
            _ => return None,
        };
        let mut values = Vec::new();
        for elt in elements {
            values.push(f(elt)?);
        }
        Some(values)
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;

    fn parse_expr(source: &str) -> Expr {
        let parsed = parse_module(source).expect("source should parse");
        let mut body = parsed.into_syntax().body;
        assert_eq!(body.len(), 1);
        let stmt = body.pop().expect("module should contain expression");
        let Stmt::Expr(stmt) = stmt else {
            panic!("source should parse as an expression statement");
        };
        *stmt.value
    }

    #[test]
    fn string_literal_extracts_normal_and_empty_strings() {
        assert_eq!(parse_expr("'django'").string_literal(), Some("django"));
        assert_eq!(parse_expr("''").string_literal(), Some(""));
    }

    #[test]
    fn string_literal_rejects_non_string() {
        assert_eq!(parse_expr("42").string_literal(), None);
    }

    #[test]
    fn string_literal_first_word_extracts_first_word() {
        assert_eq!(
            parse_expr("'end for'").string_literal_first_word(),
            Some("end")
        );
    }

    #[test]
    fn string_literal_first_word_skips_leading_whitespace() {
        assert_eq!(
            parse_expr("'  endfor'").string_literal_first_word(),
            Some("endfor")
        );
    }

    #[test]
    fn string_literal_first_word_rejects_empty_string() {
        assert_eq!(parse_expr("''").string_literal_first_word(), None);
    }

    #[test]
    fn extracts_name_target() {
        assert_eq!(parse_expr("name").name_target(), Some("name"));
    }

    #[test]
    fn rejects_name_target_for_non_name() {
        assert_eq!(parse_expr("object.attr").name_target(), None);
    }

    #[test]
    fn bool_literal_extracts_true_and_false() {
        assert_eq!(parse_expr("True").bool_literal(), Some(true));
        assert_eq!(parse_expr("False").bool_literal(), Some(false));
    }

    #[test]
    fn bool_literal_rejects_non_bool() {
        assert_eq!(parse_expr("1").bool_literal(), None);
    }

    #[test]
    fn non_negative_integer_extracts_zero_and_positive_ints() {
        assert_eq!(parse_expr("0").non_negative_integer(), Some(0));
        assert_eq!(parse_expr("42").non_negative_integer(), Some(42));
    }

    #[test]
    fn non_negative_integer_rejects_float_and_negative_ints() {
        assert_eq!(parse_expr("3.0").non_negative_integer(), None);
        assert_eq!(parse_expr("-3").non_negative_integer(), None);
    }

    #[test]
    fn negative_integer_extracts_magnitude() {
        assert_eq!(parse_expr("-3").negative_integer(), Some(3));
        assert_eq!(parse_expr("-0").negative_integer(), Some(0));
    }

    #[test]
    fn negative_integer_rejects_non_unary_expression() {
        assert_eq!(parse_expr("3").negative_integer(), None);
    }

    #[test]
    fn collection_map_maps_set_elements() {
        assert_eq!(
            parse_expr("{1, 2}").collection_map(super::ExprExt::non_negative_integer),
            Some(vec![1, 2])
        );
    }

    #[test]
    fn collection_map_accepts_empty_collections() {
        assert_eq!(
            parse_expr("()").collection_map(super::ExprExt::non_negative_integer),
            Some(Vec::new())
        );
    }

    #[test]
    fn collection_map_rejects_mixed_collections() {
        assert_eq!(
            parse_expr("(1, 'two')").collection_map(super::ExprExt::non_negative_integer),
            None
        );
    }
}
