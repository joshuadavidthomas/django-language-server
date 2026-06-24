use ruff_python_ast::Expr;
use ruff_python_ast::ExprNumberLiteral;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::Number;

pub trait ExprExt {
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

    /// Extract a non-negative integer literal as `usize`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn non_negative_integer(&self) -> Option<usize>;

    /// Check if the expression is a boolean `True` literal.
    fn is_true_literal(&self) -> bool;

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

    fn non_negative_integer(&self) -> Option<usize> {
        if let Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) = self
            && let Some(n) = int_val.as_i64()
            && n >= 0
        {
            return usize::try_from(n).ok();
        }
        None
    }

    fn is_true_literal(&self) -> bool {
        matches!(self, Expr::BooleanLiteral(lit) if lit.value)
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
}
