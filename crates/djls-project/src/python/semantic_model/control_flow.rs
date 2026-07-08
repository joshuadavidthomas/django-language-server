use ruff_python_ast as ast;

use crate::ast::ExprExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Truthiness {
    AlwaysTrue,
    AlwaysFalse,
    Ambiguous,
}

impl Truthiness {
    pub(super) const fn from_bool(value: bool) -> Self {
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

pub(super) fn evaluate_test_with(
    expr: &ast::Expr,
    bool_value: impl Fn(&str) -> Option<bool>,
) -> Truthiness {
    if let Some(name) = expr.name_target() {
        return bool_value(name).map_or(Truthiness::Ambiguous, Truthiness::from_bool);
    }

    match expr {
        ast::Expr::BooleanLiteral(literal) => Truthiness::from_bool(literal.value),
        ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => {
            evaluate_test_with(&unary.operand, bool_value).negate()
        }
        _ => Truthiness::Ambiguous,
    }
}

#[derive(Debug, Clone)]
pub(super) enum IfBranches<'a> {
    Deterministic(Option<&'a [ast::Stmt]>),
    Ambiguous(Vec<&'a [ast::Stmt]>),
}

pub(super) fn if_branches<'a, S>(
    state: &S,
    test: &ast::Expr,
    body: &'a [ast::Stmt],
    clauses: &'a [ast::ElifElseClause],
    truthiness: impl Fn(&S, &ast::Expr) -> Truthiness,
) -> IfBranches<'a> {
    match truthiness(state, test) {
        Truthiness::AlwaysTrue => IfBranches::Deterministic(Some(body)),
        Truthiness::AlwaysFalse => false_if_branches(state, clauses, truthiness),
        Truthiness::Ambiguous => {
            let mut arms = Vec::with_capacity(clauses.len() + 2);
            arms.push(body);
            let has_fallthrough = push_reachable_clause_arms(state, clauses, &mut arms, truthiness);
            if has_fallthrough {
                arms.push(&[]);
            }
            IfBranches::Ambiguous(arms)
        }
    }
}

fn false_if_branches<'a, S>(
    state: &S,
    clauses: &'a [ast::ElifElseClause],
    truthiness: impl Fn(&S, &ast::Expr) -> Truthiness,
) -> IfBranches<'a> {
    for (index, clause) in clauses.iter().enumerate() {
        let Some(test) = &clause.test else {
            return IfBranches::Deterministic(Some(&clause.body));
        };

        match truthiness(state, test) {
            Truthiness::AlwaysTrue => return IfBranches::Deterministic(Some(&clause.body)),
            Truthiness::AlwaysFalse => {}
            Truthiness::Ambiguous => {
                let ambiguous_clauses = &clauses[index..];
                let mut arms = Vec::with_capacity(ambiguous_clauses.len() + 1);
                let has_fallthrough =
                    push_reachable_clause_arms(state, ambiguous_clauses, &mut arms, truthiness);
                if has_fallthrough {
                    arms.push(&[]);
                }
                return IfBranches::Ambiguous(arms);
            }
        }
    }
    IfBranches::Deterministic(None)
}

fn push_reachable_clause_arms<'a, S>(
    state: &S,
    clauses: &'a [ast::ElifElseClause],
    arms: &mut Vec<&'a [ast::Stmt]>,
    truthiness: impl Fn(&S, &ast::Expr) -> Truthiness,
) -> bool {
    for clause in clauses {
        let Some(test) = &clause.test else {
            arms.push(clause.body.as_slice());
            return false;
        };

        match truthiness(state, test) {
            Truthiness::AlwaysTrue => {
                arms.push(clause.body.as_slice());
                return false;
            }
            Truthiness::AlwaysFalse => {}
            Truthiness::Ambiguous => arms.push(clause.body.as_slice()),
        }
    }

    true
}

#[derive(Debug, Clone, Copy)]
pub(super) enum BranchPath<'a> {
    Single(&'a [ast::Stmt]),
    Sequential(&'a [ast::Stmt], &'a [ast::Stmt]),
}

impl<'a> BranchPath<'a> {
    pub(super) fn segments(self) -> impl Iterator<Item = &'a [ast::Stmt]> {
        match self {
            Self::Single(body) => [Some(body), None],
            Self::Sequential(first, second) => [Some(first), Some(second)],
        }
        .into_iter()
        .flatten()
    }
}

pub(super) fn try_paths(stmt_try: &ast::StmtTry) -> Vec<BranchPath<'_>> {
    let mut paths = Vec::with_capacity(1 + stmt_try.handlers.len() * stmt_try.body.len().max(1));
    paths.push(BranchPath::Sequential(
        stmt_try.body.as_slice(),
        stmt_try.orelse.as_slice(),
    ));
    for handler in &stmt_try.handlers {
        let ast::ExceptHandler::ExceptHandler(handler) = handler;
        for prefix_len in 0..stmt_try.body.len().max(1) {
            paths.push(BranchPath::Sequential(
                &stmt_try.body[..prefix_len],
                handler.body.as_slice(),
            ));
        }
    }
    paths
}

pub(super) fn is_irrefutable_match_case(case: &ast::MatchCase) -> bool {
    case.guard.is_none() && is_irrefutable_pattern(&case.pattern)
}

fn is_irrefutable_pattern(pattern: &ast::Pattern) -> bool {
    match pattern {
        ast::Pattern::MatchValue(_)
        | ast::Pattern::MatchSingleton(_)
        | ast::Pattern::MatchSequence(_)
        | ast::Pattern::MatchMapping(_)
        | ast::Pattern::MatchClass(_)
        | ast::Pattern::MatchStar(_) => false,
        ast::Pattern::MatchAs(match_as) => match_as
            .pattern
            .as_deref()
            .is_none_or(is_irrefutable_pattern),
        ast::Pattern::MatchOr(match_or) => match_or.patterns.iter().any(is_irrefutable_pattern),
    }
}
