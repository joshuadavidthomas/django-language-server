use ruff_python_ast as ast;

use super::semantic_model::PythonSemanticEvaluator;
use super::semantic_model::PythonSemanticState;
use super::semantic_model::TouchedNames;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Truthiness {
    AlwaysTrue,
    AlwaysFalse,
    Ambiguous,
}

impl Truthiness {
    pub(crate) const fn from_bool(value: bool) -> Self {
        if value {
            Self::AlwaysTrue
        } else {
            Self::AlwaysFalse
        }
    }

    pub(crate) const fn negate(self) -> Self {
        match self {
            Self::AlwaysTrue => Self::AlwaysFalse,
            Self::AlwaysFalse => Self::AlwaysTrue,
            Self::Ambiguous => Self::Ambiguous,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum BranchPath<'a> {
    One(&'a [ast::Stmt]),
    Two(&'a [ast::Stmt], &'a [ast::Stmt]),
}

impl<'a> BranchPath<'a> {
    fn segments(self) -> impl Iterator<Item = &'a [ast::Stmt]> {
        match self {
            Self::One(body) => [Some(body), None],
            Self::Two(first, second) => [Some(first), Some(second)],
        }
        .into_iter()
        .flatten()
    }
}

pub(crate) fn walk_if(
    evaluator: &mut PythonSemanticEvaluator<'_>,
    state: PythonSemanticState,
    stmt_if: &ast::StmtIf,
) -> PythonSemanticState {
    match PythonSemanticEvaluator::evaluate_test(&state, &stmt_if.test) {
        Truthiness::AlwaysTrue => evaluator.walk_body(state, &stmt_if.body),
        Truthiness::AlwaysFalse => {
            walk_false_if_clauses(evaluator, state, &stmt_if.elif_else_clauses)
        }
        Truthiness::Ambiguous => {
            let mut arms = Vec::with_capacity(stmt_if.elif_else_clauses.len() + 2);
            arms.push(stmt_if.body.as_slice());
            let has_fallthrough =
                push_reachable_clause_arms(&state, &stmt_if.elif_else_clauses, &mut arms);
            if has_fallthrough {
                arms.push(&[]);
            }
            walk_ambiguous_arms(evaluator, state, &arms)
        }
    }
}

fn walk_false_if_clauses(
    evaluator: &mut PythonSemanticEvaluator<'_>,
    state: PythonSemanticState,
    clauses: &[ast::ElifElseClause],
) -> PythonSemanticState {
    for (index, clause) in clauses.iter().enumerate() {
        let Some(test) = &clause.test else {
            return evaluator.walk_body(state, &clause.body);
        };

        match PythonSemanticEvaluator::evaluate_test(&state, test) {
            Truthiness::AlwaysTrue => return evaluator.walk_body(state, &clause.body),
            Truthiness::AlwaysFalse => {}
            Truthiness::Ambiguous => {
                let ambiguous_clauses = &clauses[index..];
                let mut arms = Vec::with_capacity(ambiguous_clauses.len() + 1);
                let has_fallthrough =
                    push_reachable_clause_arms(&state, ambiguous_clauses, &mut arms);
                if has_fallthrough {
                    arms.push(&[]);
                }
                return walk_ambiguous_arms(evaluator, state, &arms);
            }
        }
    }
    state
}

fn push_reachable_clause_arms<'a>(
    state: &PythonSemanticState,
    clauses: &'a [ast::ElifElseClause],
    arms: &mut Vec<&'a [ast::Stmt]>,
) -> bool {
    for clause in clauses {
        let Some(test) = &clause.test else {
            arms.push(clause.body.as_slice());
            return false;
        };

        match PythonSemanticEvaluator::evaluate_test(state, test) {
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

pub(crate) fn walk_try(
    evaluator: &mut PythonSemanticEvaluator<'_>,
    state: PythonSemanticState,
    stmt_try: &ast::StmtTry,
) -> PythonSemanticState {
    if stmt_try.handlers.is_empty() {
        let state = evaluator.walk_body(state, &stmt_try.body);
        let state = evaluator.walk_body(state, &stmt_try.orelse);
        return evaluator.walk_body(state, &stmt_try.finalbody);
    }

    let mut paths = Vec::with_capacity(1 + stmt_try.handlers.len() * stmt_try.body.len().max(1));
    paths.push(BranchPath::Two(
        stmt_try.body.as_slice(),
        stmt_try.orelse.as_slice(),
    ));
    for handler in &stmt_try.handlers {
        let ast::ExceptHandler::ExceptHandler(handler) = handler;
        for prefix_len in 0..stmt_try.body.len().max(1) {
            paths.push(BranchPath::Two(
                &stmt_try.body[..prefix_len],
                handler.body.as_slice(),
            ));
        }
    }
    let state = walk_ambiguous_paths(evaluator, state, &paths);
    evaluator.walk_body(state, &stmt_try.finalbody)
}

pub(crate) fn walk_match(
    evaluator: &mut PythonSemanticEvaluator<'_>,
    mut state: PythonSemanticState,
    stmt_match: &ast::StmtMatch,
) -> PythonSemanticState {
    if stmt_match.cases.len() == 1 && is_irrefutable_match_case(&stmt_match.cases[0]) {
        bind_pattern_names(evaluator, &mut state, &stmt_match.cases[0].pattern);
        return evaluator.walk_body(state, &stmt_match.cases[0].body);
    }

    let mut writes = TouchedNames::default();
    for case in &stmt_match.cases {
        record_pattern_writes(&case.pattern, &mut writes);
        let body_writes = PythonSemanticEvaluator::collect_writes(&case.body);
        writes.merge(body_writes);
    }

    let base = state;
    let mut branches = Vec::with_capacity(stmt_match.cases.len() + 1);
    for case in &stmt_match.cases {
        let mut branch = base.clone();
        bind_pattern_names(evaluator, &mut branch, &case.pattern);
        branches.push(evaluator.walk_body(branch, &case.body));
    }
    if !stmt_match.cases.iter().any(is_irrefutable_match_case) {
        branches.push(base.clone());
    }
    PythonSemanticState::join_branches(base, &branches, &writes)
}

pub(crate) fn degrade_loop_bodies(
    evaluator: &mut PythonSemanticEvaluator<'_>,
    mut state: PythonSemanticState,
    bodies: &[&[ast::Stmt]],
) -> PythonSemanticState {
    let base = state.clone();
    let mut writes = TouchedNames::default();
    for body in bodies {
        let body_state = evaluator.walk_body(base.clone(), body);
        writes.merge(PythonSemanticState::changed_writes_from(&base, &body_state));
        state.merge_only_effects_from_state(body_state);
    }
    evaluator.degrade_writes(&mut state, writes);

    state
}

fn walk_ambiguous_arms(
    evaluator: &mut PythonSemanticEvaluator<'_>,
    state: PythonSemanticState,
    arms: &[&[ast::Stmt]],
) -> PythonSemanticState {
    let paths: Vec<BranchPath<'_>> = arms.iter().map(|arm| BranchPath::One(arm)).collect();
    walk_ambiguous_paths(evaluator, state, &paths)
}

fn walk_ambiguous_paths(
    evaluator: &mut PythonSemanticEvaluator<'_>,
    state: PythonSemanticState,
    paths: &[BranchPath<'_>],
) -> PythonSemanticState {
    let mut writes = TouchedNames::default();
    for path in paths {
        for segment in path.segments() {
            let segment_writes = PythonSemanticEvaluator::collect_writes(segment);
            writes.merge(segment_writes);
        }
    }

    let base = state;
    let mut branches = Vec::with_capacity(paths.len());
    for path in paths {
        let mut branch = base.clone();
        for segment in path.segments() {
            branch = evaluator.walk_body(branch, segment);
        }
        branches.push(branch);
    }
    PythonSemanticState::join_branches(base, &branches, &writes)
}

fn bind_pattern_names(
    evaluator: &PythonSemanticEvaluator<'_>,
    state: &mut PythonSemanticState,
    pattern: &ast::Pattern,
) {
    for name in pattern_bound_names(pattern) {
        evaluator.bind_pattern_name(state, name);
    }
}

fn record_pattern_writes(pattern: &ast::Pattern, writes: &mut TouchedNames) {
    for name in pattern_bound_names(pattern) {
        writes.record(name);
    }
}

pub(crate) fn pattern_bound_names(pattern: &ast::Pattern) -> Vec<&str> {
    let mut names = Vec::new();
    collect_pattern_bound_names(pattern, &mut names);
    names
}

fn collect_pattern_bound_names<'a>(pattern: &'a ast::Pattern, names: &mut Vec<&'a str>) {
    match pattern {
        ast::Pattern::MatchValue(_) | ast::Pattern::MatchSingleton(_) => {}
        ast::Pattern::MatchSequence(sequence) => {
            for pattern in &sequence.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
        ast::Pattern::MatchMapping(mapping) => {
            for pattern in &mapping.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(rest) = &mapping.rest {
                names.push(rest.as_str());
            }
        }
        ast::Pattern::MatchClass(class) => {
            for pattern in &class.arguments.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            for keyword in &class.arguments.keywords {
                collect_pattern_bound_names(&keyword.pattern, names);
            }
        }
        ast::Pattern::MatchStar(star) => {
            if let Some(name) = &star.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchAs(match_as) => {
            if let Some(pattern) = &match_as.pattern {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(name) = &match_as.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchOr(match_or) => {
            for pattern in &match_or.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
    }
}

fn is_irrefutable_match_case(case: &ast::MatchCase) -> bool {
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
