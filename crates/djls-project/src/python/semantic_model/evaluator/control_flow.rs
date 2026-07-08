use ruff_python_ast as ast;

use super::PythonSemanticEvaluator;
use super::Truthiness;
use crate::python::semantic_model::state::PythonSemanticState;
use crate::python::semantic_model::touched_names::TouchedNames;
use crate::python::semantic_model::touched_names::pattern_bound_names;

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

impl PythonSemanticEvaluator<'_> {
    pub(super) fn walk_if(
        &mut self,
        state: PythonSemanticState,
        stmt_if: &ast::StmtIf,
    ) -> PythonSemanticState {
        match Self::evaluate_test(&state, &stmt_if.test) {
            Truthiness::AlwaysTrue => self.walk_body(state, &stmt_if.body),
            Truthiness::AlwaysFalse => {
                self.walk_false_if_clauses(state, &stmt_if.elif_else_clauses)
            }
            Truthiness::Ambiguous => {
                let mut arms = Vec::with_capacity(stmt_if.elif_else_clauses.len() + 2);
                arms.push(stmt_if.body.as_slice());
                let has_fallthrough =
                    Self::push_reachable_clause_arms(&state, &stmt_if.elif_else_clauses, &mut arms);
                if has_fallthrough {
                    arms.push(&[]);
                }
                self.walk_ambiguous_arms(state, &arms)
            }
        }
    }

    fn walk_false_if_clauses(
        &mut self,
        state: PythonSemanticState,
        clauses: &[ast::ElifElseClause],
    ) -> PythonSemanticState {
        for (index, clause) in clauses.iter().enumerate() {
            let Some(test) = &clause.test else {
                return self.walk_body(state, &clause.body);
            };

            match Self::evaluate_test(&state, test) {
                Truthiness::AlwaysTrue => return self.walk_body(state, &clause.body),
                Truthiness::AlwaysFalse => {}
                Truthiness::Ambiguous => {
                    let ambiguous_clauses = &clauses[index..];
                    let mut arms = Vec::with_capacity(ambiguous_clauses.len() + 1);
                    let has_fallthrough =
                        Self::push_reachable_clause_arms(&state, ambiguous_clauses, &mut arms);
                    if has_fallthrough {
                        arms.push(&[]);
                    }
                    return self.walk_ambiguous_arms(state, &arms);
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

            match Self::evaluate_test(state, test) {
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

    pub(super) fn walk_try(
        &mut self,
        state: PythonSemanticState,
        stmt_try: &ast::StmtTry,
    ) -> PythonSemanticState {
        if stmt_try.handlers.is_empty() {
            let state = self.walk_body(state, &stmt_try.body);
            let state = self.walk_body(state, &stmt_try.orelse);
            return self.walk_body(state, &stmt_try.finalbody);
        }

        let mut paths =
            Vec::with_capacity(1 + stmt_try.handlers.len() * stmt_try.body.len().max(1));
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
        let state = self.walk_ambiguous_paths(state, &paths);
        self.walk_body(state, &stmt_try.finalbody)
    }

    pub(super) fn walk_match(
        &mut self,
        mut state: PythonSemanticState,
        stmt_match: &ast::StmtMatch,
    ) -> PythonSemanticState {
        if stmt_match.cases.len() == 1 && Self::is_irrefutable_match_case(&stmt_match.cases[0]) {
            self.bind_pattern_names(&mut state, &stmt_match.cases[0].pattern);
            return self.walk_body(state, &stmt_match.cases[0].body);
        }

        let mut writes = TouchedNames::default();
        for case in &stmt_match.cases {
            Self::record_pattern_writes(&case.pattern, &mut writes);
            let body_writes = Self::collect_writes(&case.body);
            writes.merge(body_writes);
        }

        let base = state;
        let mut branches = Vec::with_capacity(stmt_match.cases.len() + 1);
        for case in &stmt_match.cases {
            let mut branch = base.clone();
            self.bind_pattern_names(&mut branch, &case.pattern);
            branches.push(self.walk_body(branch, &case.body));
        }
        if !stmt_match.cases.iter().any(Self::is_irrefutable_match_case) {
            branches.push(base.clone());
        }
        PythonSemanticState::join_branches(base, &branches, &writes)
    }

    pub(super) fn degrade_loop_bodies(
        &mut self,
        mut state: PythonSemanticState,
        bodies: &[&[ast::Stmt]],
    ) -> PythonSemanticState {
        let base = state.clone();
        let mut writes = TouchedNames::default();
        for body in bodies {
            let body_state = self.walk_body(base.clone(), body);
            writes.merge(PythonSemanticState::changed_writes_from(&base, &body_state));
            state.merge_only_effects_from_state(body_state);
        }
        self.degrade_writes(&mut state, writes);

        state
    }

    fn walk_ambiguous_arms(
        &mut self,
        state: PythonSemanticState,
        arms: &[&[ast::Stmt]],
    ) -> PythonSemanticState {
        let paths: Vec<BranchPath<'_>> = arms.iter().map(|arm| BranchPath::One(arm)).collect();
        self.walk_ambiguous_paths(state, &paths)
    }

    fn walk_ambiguous_paths(
        &mut self,
        state: PythonSemanticState,
        paths: &[BranchPath<'_>],
    ) -> PythonSemanticState {
        let mut writes = TouchedNames::default();
        for path in paths {
            for segment in path.segments() {
                let segment_writes = Self::collect_writes(segment);
                writes.merge(segment_writes);
            }
        }

        let base = state;
        let mut branches = Vec::with_capacity(paths.len());
        for path in paths {
            let mut branch = base.clone();
            for segment in path.segments() {
                branch = self.walk_body(branch, segment);
            }
            branches.push(branch);
        }
        PythonSemanticState::join_branches(base, &branches, &writes)
    }

    fn bind_pattern_names(&self, state: &mut PythonSemanticState, pattern: &ast::Pattern) {
        for name in pattern_bound_names(pattern) {
            self.bind_pattern_name(state, name);
        }
    }

    fn record_pattern_writes(pattern: &ast::Pattern, writes: &mut TouchedNames) {
        for name in pattern_bound_names(pattern) {
            writes.record(name);
        }
    }

    fn is_irrefutable_match_case(case: &ast::MatchCase) -> bool {
        case.guard.is_none() && Self::is_irrefutable_pattern(&case.pattern)
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
                .is_none_or(Self::is_irrefutable_pattern),
            ast::Pattern::MatchOr(match_or) => {
                match_or.patterns.iter().any(Self::is_irrefutable_pattern)
            }
        }
    }
}
