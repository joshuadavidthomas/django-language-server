use ruff_python_ast as ast;

use crate::ast::ExprExt;

/// A statically known result of Python truth testing.
///
/// `None` represents an expression whose truthiness cannot be decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Truthiness {
    Falsy,
    Truthy,
}

impl Truthiness {
    pub(super) fn of_expr(
        expression: &ast::Expr,
        known_truthiness: &impl Fn(&str) -> Option<Self>,
    ) -> Option<Self> {
        if let Some(name) = expression.name_target() {
            return known_truthiness(name);
        }

        match expression {
            ast::Expr::BooleanLiteral(literal) => Some(Self::from_bool(literal.value)),
            ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => {
                Self::of_expr(&unary.operand, known_truthiness).map(Self::negate)
            }
            ast::Expr::BoolOp(boolean) => match boolean.op {
                ast::BoolOp::And => {
                    let mut result = Some(Self::Truthy);
                    for value in &boolean.values {
                        result = Self::and(result, Self::of_expr(value, known_truthiness));
                    }
                    result
                }
                ast::BoolOp::Or => {
                    let mut result = Some(Self::Falsy);
                    for value in &boolean.values {
                        result = Self::or(result, Self::of_expr(value, known_truthiness));
                    }
                    result
                }
            },
            ast::Expr::Named(_)
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
            | ast::Expr::IpyEscapeCommand(_) => None,
        }
    }

    pub(super) const fn from_bool(value: bool) -> Self {
        if value { Self::Truthy } else { Self::Falsy }
    }

    pub(super) const fn branch_arm(self) -> usize {
        match self {
            Self::Falsy => 0,
            Self::Truthy => 1,
        }
    }

    const fn negate(self) -> Self {
        match self {
            Self::Falsy => Self::Truthy,
            Self::Truthy => Self::Falsy,
        }
    }

    const fn and(left: Option<Self>, right: Option<Self>) -> Option<Self> {
        match (left, right) {
            (Some(Self::Falsy), _) | (_, Some(Self::Falsy)) => Some(Self::Falsy),
            (Some(Self::Truthy), Some(Self::Truthy)) => Some(Self::Truthy),
            (Some(Self::Truthy) | None, Some(Self::Truthy) | None) => None,
        }
    }

    const fn or(left: Option<Self>, right: Option<Self>) -> Option<Self> {
        match (left, right) {
            (Some(Self::Truthy), _) | (_, Some(Self::Truthy)) => Some(Self::Truthy),
            (Some(Self::Falsy), Some(Self::Falsy)) => Some(Self::Falsy),
            (Some(Self::Falsy) | None, Some(Self::Falsy) | None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast as ast;
    use ruff_python_parser::parse_module;

    use super::Truthiness;

    fn classify_with(
        expression: &str,
        known_truthiness: &impl Fn(&str) -> Option<Truthiness>,
    ) -> Option<Truthiness> {
        let source = format!("VALUE = {expression}\n");
        let module = parse_module(&source)
            .expect("expression should parse")
            .into_syntax();
        let [ast::Stmt::Assign(assignment)] = module.body.as_slice() else {
            panic!("expected one assignment");
        };
        Truthiness::of_expr(&assignment.value, known_truthiness)
    }

    fn classify(expression: &str) -> Option<Truthiness> {
        classify_with(expression, &|_| None)
    }

    #[test]
    fn classifies_boolean_literals_names_and_negation() {
        assert_eq!(classify("False"), Some(Truthiness::Falsy));
        assert_eq!(classify("True"), Some(Truthiness::Truthy));
        assert_eq!(classify("unknown"), None);
        assert_eq!(
            classify_with("known", &|name| {
                (name == "known").then_some(Truthiness::Truthy)
            }),
            Some(Truthiness::Truthy),
        );
        assert_eq!(classify("not False"), Some(Truthiness::Truthy));
        assert_eq!(classify("not unknown"), None);
    }

    #[test]
    fn boolean_operators_preserve_decisive_values_amid_ambiguity() {
        assert_eq!(classify("unknown and False"), Some(Truthiness::Falsy));
        assert_eq!(classify("False and unknown"), Some(Truthiness::Falsy));
        assert_eq!(classify("unknown and True"), None);
        assert_eq!(classify("unknown or True"), Some(Truthiness::Truthy));
        assert_eq!(classify("True or unknown"), Some(Truthiness::Truthy));
        assert_eq!(classify("unknown or False"), None);
    }

    #[test]
    fn exact_truthiness_maps_to_predicate_arms() {
        assert_eq!(Truthiness::Falsy.branch_arm(), 0);
        assert_eq!(Truthiness::Truthy.branch_arm(), 1);
    }
}
