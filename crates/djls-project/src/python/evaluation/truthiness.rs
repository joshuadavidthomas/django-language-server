use ruff_python_ast as ast;

use crate::ast::ExprExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Truthiness {
    AlwaysTrue,
    AlwaysFalse,
    Ambiguous,
}

impl Truthiness {
    pub(super) fn of_expr(
        expression: &ast::Expr,
        known_bool: &impl Fn(&str) -> Option<bool>,
    ) -> Self {
        if let Some(name) = expression.name_target() {
            return known_bool(name).map_or(Self::Ambiguous, Self::from_bool);
        }

        match expression {
            ast::Expr::BooleanLiteral(literal) => Self::from_bool(literal.value),
            ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => {
                Self::of_expr(&unary.operand, known_bool).negate()
            }
            _ => Self::Ambiguous,
        }
    }

    const fn from_bool(value: bool) -> Self {
        if value {
            Self::AlwaysTrue
        } else {
            Self::AlwaysFalse
        }
    }

    const fn negate(self) -> Self {
        match self {
            Self::AlwaysTrue => Self::AlwaysFalse,
            Self::AlwaysFalse => Self::AlwaysTrue,
            Self::Ambiguous => Self::Ambiguous,
        }
    }
}
