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
            ast::Expr::BoolOp(_)
            | ast::Expr::Named(_)
            | ast::Expr::BinOp(_)
            | ast::Expr::UnaryOp(_)
            | ast::Expr::Lambda(_)
            | ast::Expr::If(_)
            | ast::Expr::Dict(_)
            | ast::Expr::Set(_)
            | ast::Expr::ListComp(_)
            | ast::Expr::SetComp(_)
            | ast::Expr::DictComp(_)
            | ast::Expr::Generator(_)
            | ast::Expr::Await(_)
            | ast::Expr::Yield(_)
            | ast::Expr::YieldFrom(_)
            | ast::Expr::Compare(_)
            | ast::Expr::Call(_)
            | ast::Expr::FString(_)
            | ast::Expr::TString(_)
            | ast::Expr::StringLiteral(_)
            | ast::Expr::BytesLiteral(_)
            | ast::Expr::NumberLiteral(_)
            | ast::Expr::NoneLiteral(_)
            | ast::Expr::EllipsisLiteral(_)
            | ast::Expr::Attribute(_)
            | ast::Expr::Subscript(_)
            | ast::Expr::Starred(_)
            | ast::Expr::Name(_)
            | ast::Expr::List(_)
            | ast::Expr::Tuple(_)
            | ast::Expr::Slice(_)
            | ast::Expr::IpyEscapeCommand(_) => Self::Ambiguous,
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
