use ruff_python_ast::Expr;
use ruff_python_ast::ExprNumberLiteral;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::Number;

pub(crate) trait ExprExt {
    /// Extract the full string value from a string literal expression.
    fn string_literal(&self) -> Option<String>;

    /// Extract the first whitespace-delimited word from a string literal.
    ///
    /// Django's `Parser.parse()` compares against
    /// `command = token.contents.split()[0]`, so only the first word of a
    /// stop-token string matters.
    fn string_literal_first_word(&self) -> Option<String>;

    /// Extract a non-negative integer literal as `usize`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn positive_integer(&self) -> Option<usize>;

    /// Check if the expression is a boolean `True` literal.
    fn is_true_literal(&self) -> bool;

    /// Map elements of a collection expression (tuple, list, or set) through
    /// a fallible function. Returns `None` if the expression is not a
    /// collection or if any element mapping fails.
    fn collection_map<T>(&self, f: impl Fn(&Expr) -> Option<T>) -> Option<Vec<T>>;
}

impl ExprExt for Expr {
    fn string_literal(&self) -> Option<String> {
        if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = self {
            return Some(value.to_str().to_string());
        }
        None
    }

    fn string_literal_first_word(&self) -> Option<String> {
        if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = self {
            let s = value.to_str();
            let cmd = s.split_whitespace().next().unwrap_or("");
            if cmd.is_empty() {
                return None;
            }
            return Some(cmd.to_string());
        }
        None
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn positive_integer(&self) -> Option<usize> {
        if let Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) = self
        {
            if let Some(n) = int_val.as_i64() {
                if n >= 0 {
                    return Some(n as usize);
                }
            }
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
