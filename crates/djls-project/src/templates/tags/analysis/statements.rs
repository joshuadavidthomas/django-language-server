use std::collections::BTreeSet;
use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::analysis::AnalysisResult;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::constraints::ExtractedTagConstraints;
use crate::templates::tags::analysis::exceptions::direct_raise_exception;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::analysis::expressions::eval_expr_with_ctx;
use crate::templates::tags::analysis::match_arms::extract_match_constraints;
use crate::templates::tags::analysis::mutations::try_extract_option_loop;
use crate::templates::tags::analysis::mutations::try_extract_pop_call;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::ExtractedDiagnosticConstraint;
use crate::templates::tags::types::ExtractedDiagnosticMessage;
use crate::templates::tags::types::ExtractedMessageTemplate;
use crate::templates::tags::types::SplitPosition;
use crate::templates::tags::types::TagArgumentSyntax;

const MAX_EXEC_STATES: usize = 64;

/// The control destination attached to one feasible execution state.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlOutcome {
    Next,
    Return(AbstractValue),
    Raise,
    Break,
    Continue,
}

/// One feasible path through a compile function. Facts stay attached to the
/// environment that established them until all accepting paths are projected.
#[derive(Debug, Clone, PartialEq)]
struct ExecutionState {
    env: Env,
    result: AnalysisResult,
    outcome: ControlOutcome,
}

impl ExecutionState {
    fn from_env(env: Env) -> Self {
        Self {
            env,
            result: AnalysisResult::default(),
            outcome: ControlOutcome::Next,
        }
    }

    fn attach_argument_syntax(&mut self, syntax: Option<&TagArgumentSyntax>) {
        if let Some(syntax) = syntax {
            self.result.extend(AnalysisResult {
                argument_syntax: Some(syntax.clone()),
                ..AnalysisResult::default()
            });
        }
    }
}

/// Feasible control destinations at one statement boundary.
#[derive(Debug)]
struct ExecutionStates(Vec<ExecutionState>);

impl ExecutionStates {
    fn from_env(env: Env) -> Self {
        Self(vec![ExecutionState::from_env(env)])
    }

    fn has_next(&self) -> bool {
        self.0
            .iter()
            .any(|state| matches!(state.outcome, ControlOutcome::Next))
    }

    fn next_mut(&mut self) -> impl Iterator<Item = &mut ExecutionState> {
        self.0
            .iter_mut()
            .filter(|state| matches!(state.outcome, ControlOutcome::Next))
    }

    fn take_next(&mut self) -> Vec<ExecutionState> {
        self.take_outcome(|outcome| matches!(outcome, ControlOutcome::Next))
    }

    fn take_outcome(&mut self, predicate: impl Fn(&ControlOutcome) -> bool) -> Vec<ExecutionState> {
        let (taken, retained) = std::mem::take(&mut self.0)
            .into_iter()
            .partition(|state| predicate(&state.outcome));
        self.0 = retained;
        taken
    }

    fn normalize(&mut self) {
        normalize_state_destinations(&mut [&mut self.0]);
    }
}

fn outcome_index(outcome: &ControlOutcome) -> usize {
    match outcome {
        ControlOutcome::Next => 0,
        ControlOutcome::Return(_) => 1,
        ControlOutcome::Raise => 2,
        ControlOutcome::Break => 3,
        ControlOutcome::Continue => 4,
    }
}

fn deduplicate_states(states: &mut Vec<ExecutionState>) {
    if states.len() < 2 {
        return;
    }
    let mut unique_len = 1;
    for candidate in 1..states.len() {
        if states[..unique_len].contains(&states[candidate]) {
            continue;
        }
        states.swap(unique_len, candidate);
        unique_len += 1;
    }
    states.truncate(unique_len);
}

fn normalize_state_destinations(destinations: &mut [&mut Vec<ExecutionState>]) {
    for destination in destinations.iter_mut() {
        deduplicate_states(destination);
    }
    if destinations
        .iter()
        .map(|states| states.len())
        .sum::<usize>()
        <= MAX_EXEC_STATES
    {
        return;
    }

    let mut groups = Vec::new();
    for (destination_index, destination) in destinations.iter_mut().enumerate() {
        let mut by_outcome = [const { Vec::new() }; 5];
        for state in destination.drain(..) {
            by_outcome[outcome_index(&state.outcome)].push(state);
        }
        groups.extend(
            by_outcome
                .into_iter()
                .filter(|group| !group.is_empty())
                .map(|group| (destination_index, group)),
        );
    }

    let lengths = groups
        .iter()
        .map(|(_, group)| group.len())
        .collect::<Vec<_>>();
    let mut budgets = vec![1; groups.len()];
    let mut remaining = MAX_EXEC_STATES - budgets.len();
    while remaining > 0 {
        let mut added = false;
        for (budget, length) in budgets.iter_mut().zip(&lengths) {
            if *budget < *length {
                *budget += 1;
                remaining -= 1;
                added = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !added {
            break;
        }
    }

    let mut normalized = (0..destinations.len())
        .map(|_| Vec::new())
        .collect::<Vec<_>>();
    for ((destination_index, mut group), budget) in groups.into_iter().zip(budgets) {
        if group.len() > budget {
            let summary = summarize_states(&group);
            group.truncate(budget - 1);
            group.push(summary);
        }
        normalized[destination_index].append(&mut group);
    }
    for (destination, mut states) in destinations.iter_mut().zip(normalized) {
        destination.append(&mut states);
    }
}

fn summarize_states(states: &[ExecutionState]) -> ExecutionState {
    let mut summary = states[0].clone();
    summary.env = Env::join_exact(states.iter().map(|state| &state.env));
    summary.result = project_results(&states.iter().collect::<Vec<_>>());
    if let ControlOutcome::Return(first) = &summary.outcome
        && !states
            .iter()
            .all(|state| matches!(&state.outcome, ControlOutcome::Return(value) if value == first))
    {
        summary.outcome = ControlOutcome::Return(AbstractValue::Unknown);
    }
    summary
}

/// Process a function body and retain facts entailed by every accepting path.
pub(crate) fn process_statements(
    stmts: &[Stmt],
    env: &mut Env,
    ctx: &mut CallContext<'_>,
) -> (AnalysisResult, AbstractValue) {
    let states =
        process_statement_states(stmts, ExecutionStates::from_env(std::mem::take(env)), ctx);
    let accepting = states
        .0
        .iter()
        .filter(|state| {
            matches!(
                state.outcome,
                ControlOutcome::Next | ControlOutcome::Return(_)
            )
        })
        .collect::<Vec<_>>();
    *env = Env::join_exact(accepting.iter().map(|state| &state.env));

    let values = accepting.iter().filter_map(|state| match &state.outcome {
        ControlOutcome::Return(value) => Some(value.clone()),
        ControlOutcome::Next => Some(AbstractValue::Unknown),
        ControlOutcome::Raise | ControlOutcome::Break | ControlOutcome::Continue => None,
    });
    (project_results(&accepting), join_return_values(values))
}

fn join_return_values(mut values: impl Iterator<Item = AbstractValue>) -> AbstractValue {
    let Some(first) = values.next() else {
        return AbstractValue::Unknown;
    };
    if values.all(|value| value == first) {
        first
    } else {
        AbstractValue::Unknown
    }
}

fn process_statement_states(
    stmts: &[Stmt],
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    for stmt in stmts {
        if !states.has_next() {
            break;
        }

        match stmt {
            Stmt::If(stmt_if) => states = branch_if(stmt_if, states, ctx),
            Stmt::Try(stmt_try) => states = branch_try(stmt_try, states, ctx),
            Stmt::For(stmt_for) => states = branch_for(stmt_for, states, ctx),
            Stmt::While(stmt_while) => states = branch_while(stmt_while, states, ctx),
            Stmt::With(stmt_with) => states = branch_with(stmt_with, states, ctx),
            Stmt::Match(stmt_match) => states = branch_match(stmt_match, states, ctx),
            Stmt::Return(returned) => {
                for state in states.next_mut() {
                    let value = returned
                        .value
                        .as_deref()
                        .map_or(AbstractValue::Unknown, |expr| {
                            eval_expr_with_ctx(expr, &mut state.env, Some(ctx))
                        });
                    state.outcome = ControlOutcome::Return(value);
                }
            }
            Stmt::Raise(_) => {
                for state in states.next_mut() {
                    state.outcome = ControlOutcome::Raise;
                }
            }
            Stmt::Break(_) => {
                for state in states.next_mut() {
                    state.outcome = ControlOutcome::Break;
                }
            }
            Stmt::Continue(_) => {
                for state in states.next_mut() {
                    state.outcome = ControlOutcome::Continue;
                }
            }
            Stmt::FunctionDef(_)
            | Stmt::ClassDef(_)
            | Stmt::Assign(_)
            | Stmt::Delete(_)
            | Stmt::TypeAlias(_)
            | Stmt::AugAssign(_)
            | Stmt::AnnAssign(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::IpyEscapeCommand(_)
            | Stmt::Expr(_) => {
                for state in states.next_mut() {
                    state
                        .result
                        .extend(process_statement(stmt, &mut state.env, ctx));
                }
            }
        }
        states.normalize();
    }

    states
}

fn branch_if(
    stmt_if: &ruff_python_ast::StmtIf,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    let mut alternatives = states;

    for mut state in incoming {
        // Keep the original fixed-width syntax prepass until path-derived forms
        // replace it in the later extraction change.
        let argument_syntax = crate::templates::tags::analysis::forms::extract_if_argument_syntax(
            stmt_if, &state.env, ctx,
        );
        state.attach_argument_syntax(argument_syntax.as_ref());

        let mut unmatched = Some(state);
        let mut clauses = Vec::with_capacity(stmt_if.elif_else_clauses.len() + 1);
        clauses.push((Some(stmt_if.test.as_ref()), stmt_if.body.as_slice()));
        clauses.extend(
            stmt_if
                .elif_else_clauses
                .iter()
                .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
        );

        for (test, body) in clauses {
            let Some(state) = unmatched.take() else {
                break;
            };
            let truth = test.and_then(static_truthiness);
            if truth == Some(false) {
                unmatched = Some(state);
                continue;
            }

            let has_direct_raise = body.iter().any(|stmt| matches!(stmt, Stmt::Raise(_)));
            let direct_guard =
                test.filter(|_| direct_raise_exception(body).is_some())
                    .map(|test| {
                        crate::templates::tags::analysis::guards::extract_direct_guard(
                            test, body, &state.env,
                        )
                    });

            let mut taken = state.clone();
            if let Some(test) = test.filter(|_| !has_direct_raise) {
                taken.result.constraints.extend(
                    crate::templates::tags::analysis::guards::extract_true_condition_constraints(
                        test, &taken.env,
                    ),
                );
            }
            let mut branch = process_statement_states(body, ExecutionStates(vec![taken]), ctx);
            alternatives.0.append(&mut branch.0);

            if truth != Some(true) && test.is_some() {
                let mut fallthrough = state;
                if let Some(guard) = direct_guard {
                    fallthrough.result.extend(guard.into());
                } else if let Some(test) = test.filter(|_| !has_direct_raise) {
                    fallthrough.result.constraints.extend(
                        crate::templates::tags::analysis::guards::extract_false_condition_constraints(
                            test, &fallthrough.env,
                        ),
                    );
                }
                unmatched = Some(fallthrough);
            }
        }

        if let Some(state) = unmatched {
            alternatives.0.push(state);
        }
    }

    alternatives
}

fn branch_for(
    stmt_for: &ruff_python_ast::StmtFor,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    for state in incoming {
        let iterator = eval_expr(&stmt_for.iter, &mut state.env.clone());
        if let AbstractValue::Tuple(values) = iterator {
            let mut active = vec![state];
            for value in values {
                let mut next_iteration = Vec::new();
                for mut active_state in active {
                    process_assignment_target(&stmt_for.target, &value, &mut active_state.env);
                    let body = process_statement_states(
                        &stmt_for.body,
                        ExecutionStates(vec![active_state]),
                        ctx,
                    );
                    collect_loop_body(body, &mut states.0, &mut next_iteration);
                }
                normalize_state_destinations(&mut [&mut states.0, &mut next_iteration]);
                active = next_iteration;
                if active.is_empty() {
                    break;
                }
            }
            let mut exhausted =
                process_statement_states(&stmt_for.orelse, ExecutionStates(active), ctx);
            states.0.append(&mut exhausted.0);
            continue;
        }

        let mut body_entry = state;
        let mut target_changes = PotentialEnvChanges::default();
        target_changes.record_target(&stmt_for.target);
        body_entry.env = target_changes.apply(&body_entry.env);
        let body = process_statement_states(&stmt_for.body, ExecutionStates(vec![body_entry]), ctx);
        let mut exhausted = Vec::new();
        collect_loop_body(body, &mut states.0, &mut exhausted);
        let mut after_loop =
            process_statement_states(&stmt_for.orelse, ExecutionStates(exhausted), ctx);
        states.0.append(&mut after_loop.0);
    }
    states
}

fn branch_while(
    stmt_while: &ruff_python_ast::StmtWhile,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    for mut state in incoming {
        if let Some(options) = try_extract_option_loop(stmt_while, &state.env) {
            state.result.known_options = Some(options);
            states.0.push(state);
            continue;
        }

        // The original evaluator executes one representative loop iteration.
        // Keep that bound while carrying exits as explicit outcomes.
        let body = process_statement_states(&stmt_while.body, ExecutionStates(vec![state]), ctx);
        let mut exhausted = Vec::new();
        collect_loop_body(body, &mut states.0, &mut exhausted);
        let mut after_loop =
            process_statement_states(&stmt_while.orelse, ExecutionStates(exhausted), ctx);
        states.0.append(&mut after_loop.0);
    }
    states
}

fn collect_loop_body(
    body: ExecutionStates,
    completed: &mut Vec<ExecutionState>,
    repeatable: &mut Vec<ExecutionState>,
) {
    for mut state in body.0 {
        match state.outcome {
            ControlOutcome::Next | ControlOutcome::Continue => {
                state.outcome = ControlOutcome::Next;
                repeatable.push(state);
            }
            ControlOutcome::Break => {
                state.outcome = ControlOutcome::Next;
                completed.push(state);
            }
            ControlOutcome::Return(_) | ControlOutcome::Raise => completed.push(state),
        }
    }
}

fn branch_with(
    stmt_with: &ruff_python_ast::StmtWith,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    for mut state in incoming {
        let mut changes = PotentialEnvChanges::default();
        for item in &stmt_with.items {
            if let Some(target) = &item.optional_vars {
                changes.record_target(target);
            }
        }
        state.env = changes.apply(&state.env);
        let mut body = process_statement_states(&stmt_with.body, ExecutionStates(vec![state]), ctx);
        states.0.append(&mut body.0);
    }
    states
}

fn branch_match(
    stmt_match: &ruff_python_ast::StmtMatch,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    for mut state in incoming {
        if let Some(constraints) = extract_match_constraints(stmt_match, &mut state.env) {
            state.result.constraints.extend(constraints);
        }
        // Match exhaustiveness and pattern binding are handled by the later
        // path-form change. Keep each original arm feasible here.
        states.0.push(state.clone());
        for case in &stmt_match.cases {
            let mut branch =
                process_statement_states(&case.body, ExecutionStates(vec![state.clone()]), ctx);
            states.0.append(&mut branch.0);
        }
    }
    states
}

#[derive(Default)]
struct PotentialEnvChanges {
    assigned_names: BTreeSet<String>,
    mutates_split_result: bool,
    forget_all: bool,
}

impl PotentialEnvChanges {
    fn collect(stmts: &[Stmt]) -> Self {
        let mut changes = Self::default();
        walk_stmts(stmts, Recurse::ControlFlow, |stmt| {
            match stmt {
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        changes.record_target(target);
                    }
                    changes.record_mutating_call(&assign.value);
                }
                Stmt::AnnAssign(assign) => changes.record_target(&assign.target),
                Stmt::AugAssign(assign) => changes.record_target(&assign.target),
                Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        changes.record_target(target);
                    }
                }
                Stmt::For(stmt_for) => changes.record_target(&stmt_for.target),
                Stmt::With(stmt_with) => {
                    for item in &stmt_with.items {
                        if let Some(target) = &item.optional_vars {
                            changes.record_target(target);
                        }
                    }
                }
                Stmt::Expr(stmt_expr) => changes.record_mutating_call(&stmt_expr.value),
                // These statements bind names, but the tag evaluator does not
                // retain enough target detail to widen only those bindings.
                Stmt::Import(_)
                | Stmt::ImportFrom(_)
                | Stmt::FunctionDef(_)
                | Stmt::ClassDef(_)
                | Stmt::TypeAlias(_)
                | Stmt::Match(_) => changes.forget_all = true,
                Stmt::If(_)
                | Stmt::While(_)
                | Stmt::Try(_)
                | Stmt::Return(_)
                | Stmt::Raise(_)
                | Stmt::Assert(_)
                | Stmt::Global(_)
                | Stmt::Nonlocal(_)
                | Stmt::Pass(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::IpyEscapeCommand(_) => {}
            }
            ControlFlow::Continue(())
        });
        changes
    }

    fn extend(&mut self, stmts: &[Stmt]) {
        let other = Self::collect(stmts);
        self.assigned_names.extend(other.assigned_names);
        self.mutates_split_result |= other.mutates_split_result;
        self.forget_all |= other.forget_all;
    }

    fn record_target(&mut self, target: &Expr) {
        if let Some(name) = target.name_target() {
            self.assigned_names.insert(name.to_string());
            return;
        }
        match target {
            Expr::Tuple(tuple) => {
                for element in &tuple.elts {
                    self.record_target(element);
                }
            }
            Expr::List(list) => {
                for element in &list.elts {
                    self.record_target(element);
                }
            }
            Expr::Starred(starred) => self.record_target(&starred.value),
            Expr::BoolOp(_)
            | Expr::Named(_)
            | Expr::BinOp(_)
            | Expr::Compare(_)
            | Expr::UnaryOp(_)
            | Expr::Lambda(_)
            | Expr::If(_)
            | Expr::Dict(_)
            | Expr::Set(_)
            | Expr::ListComp(_)
            | Expr::SetComp(_)
            | Expr::DictComp(_)
            | Expr::Generator(_)
            | Expr::Await(_)
            | Expr::Yield(_)
            | Expr::YieldFrom(_)
            | Expr::Call(_)
            | Expr::FString(_)
            | Expr::TString(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::Attribute(_)
            | Expr::Subscript(_)
            | Expr::Name(_)
            | Expr::Slice(_)
            | Expr::IpyEscapeCommand(_) => self.forget_all = true,
        }
    }

    fn record_mutating_call(&mut self, expr: &Expr) {
        if try_extract_pop_call(expr).is_some() {
            // Separate names may refer to the same Python list. The abstract
            // values do not carry allocation identity, so invalidate every
            // token-derived sequence when one of them may be mutated.
            self.mutates_split_result = true;
        }
        if let Some(name) = try_extract_token_kwargs_call(expr) {
            self.assigned_names.insert(name);
            self.mutates_split_result = true;
        }
    }

    fn apply(&self, env: &Env) -> Env {
        if self.forget_all {
            return Env::default();
        }
        let mut widened = env.clone();
        for name in &self.assigned_names {
            widened.forget(name);
        }
        if self.mutates_split_result {
            widened.forget_split_results();
        }
        widened
    }
}

fn branch_try(
    stmt_try: &ruff_python_ast::StmtTry,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    // Outcomes completed before this try never enter its finalizer.
    let mut alternatives = ExecutionStates(Vec::new());

    for state in incoming {
        let body_changes = PotentialEnvChanges::collect(&stmt_try.body);
        let mut body =
            process_statement_states(&stmt_try.body, ExecutionStates(vec![state.clone()]), ctx);

        let raised = body.take_outcome(|outcome| matches!(outcome, ControlOutcome::Raise));
        let normal = ExecutionStates(body.take_next());
        let mut success = process_statement_states(&stmt_try.orelse, normal, ctx);
        alternatives.0.append(&mut body.0);
        alternatives.0.append(&mut success.0);
        // An explicit raise may escape the try or enter a matching handler.
        alternatives.0.extend(raised.iter().cloned());

        for exception in &stmt_try.handlers {
            let ruff_python_ast::ExceptHandler::ExceptHandler(clause) = exception;
            for mut raised_state in raised.iter().cloned() {
                raised_state.outcome = ControlOutcome::Next;
                let mut handled = process_statement_states(
                    &clause.body,
                    ExecutionStates(vec![raised_state]),
                    ctx,
                );
                alternatives.0.append(&mut handled.0);
            }

            // Any runtime operation may fail between changes. AST writes and
            // mutations cover intermediate states that terminal environments
            // cannot reveal when a later assignment restores the entry value.
            let mut unknown_exception = state.clone();
            unknown_exception.env = body_changes.apply(&state.env);
            let mut handled = process_statement_states(
                &clause.body,
                ExecutionStates(vec![unknown_exception]),
                ctx,
            );
            for handled_state in &mut handled.0 {
                handled_state.result = state.result.clone();
            }
            alternatives.0.append(&mut handled.0);
        }

        if !stmt_try.finalbody.is_empty() {
            // An implicit exception also enters `finally` when no handler is
            // present, or when it bypasses or arises within a handler.
            let mut finalizer_changes = body_changes;
            finalizer_changes.extend(&stmt_try.orelse);
            for handler in &stmt_try.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
                finalizer_changes.extend(&handler.body);
            }
            let mut implicit_raise = state.clone();
            implicit_raise.env = finalizer_changes.apply(&state.env);
            implicit_raise.outcome = ControlOutcome::Raise;
            alternatives.0.push(implicit_raise);
        }
    }

    let mut result = if stmt_try.finalbody.is_empty() {
        alternatives
    } else {
        run_finally(&stmt_try.finalbody, alternatives, ctx)
    };
    result.0.append(&mut states.0);
    result.normalize();
    result
}

fn run_finally(
    finalbody: &[Stmt],
    alternatives: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let mut after_finally = ExecutionStates(Vec::new());
    for mut state in alternatives.0 {
        let mut saved = std::mem::replace(&mut state.outcome, ControlOutcome::Next);
        if let ControlOutcome::Return(value) = &mut saved {
            // Scalars are snapshots, but a returned list still shares its Python
            // object with the finalizer. The evaluator cannot preserve that alias.
            value.forget_mutable();
        }
        let finalized = process_statement_states(finalbody, ExecutionStates(vec![state]), ctx);
        after_finally
            .0
            .extend(finalized.0.into_iter().map(|mut finalized_state| {
                if matches!(finalized_state.outcome, ControlOutcome::Next) {
                    finalized_state.outcome = saved.clone();
                }
                finalized_state
            }));
    }
    after_finally.normalize();
    after_finally
}

fn project_results(paths: &[&ExecutionState]) -> AnalysisResult {
    let Some(first) = paths.first() else {
        return AnalysisResult::default();
    };

    let mut constraints = project_constraints(paths);
    let diagnostic_messages = project_diagnostics(paths, &constraints);
    let known_options = first.result.known_options.clone().filter(|options| {
        paths
            .iter()
            .all(|path| path.result.known_options.as_ref() == Some(options))
    });
    let argument_syntax = if paths
        .iter()
        .all(|path| path.result.argument_syntax == first.result.argument_syntax)
    {
        first.result.argument_syntax.clone()
    } else if paths
        .iter()
        .any(|path| path.result.argument_syntax.is_some())
    {
        Some(TagArgumentSyntax::Unknown)
    } else {
        None
    };
    if matches!(argument_syntax, Some(TagArgumentSyntax::Forms { .. })) {
        // Forms retain count/keyword correlation. Reapplying their branch-local
        // facts as flat constraints both loses that correlation and duplicates
        // diagnostics.
        constraints = ExtractedTagConstraints::default();
    }

    AnalysisResult {
        constraints,
        diagnostic_messages,
        known_options,
        argument_syntax,
    }
}

fn project_constraints(paths: &[&ExecutionState]) -> ExtractedTagConstraints {
    if paths.is_empty() {
        return ExtractedTagConstraints::default();
    }

    let keyword_candidates = paths
        .iter()
        .flat_map(|path| path.result.constraints.required_keywords.iter().cloned())
        .fold(Vec::new(), |mut candidates, keyword| {
            if !candidates.contains(&keyword) {
                candidates.push(keyword);
            }
            candidates
        });
    let required_keywords = keyword_candidates
        .into_iter()
        .filter(|keyword| {
            paths.iter().all(|path| {
                path.result.constraints.required_keywords.contains(keyword)
                    || position_is_absent(
                        keyword.position,
                        &path.result.constraints.arg_constraints,
                    )
            })
        })
        .collect();

    let choice_candidates = paths
        .iter()
        .flat_map(|path| {
            path.result
                .constraints
                .choice_at_constraints
                .iter()
                .cloned()
        })
        .fold(Vec::new(), |mut candidates, choice| {
            if !candidates.contains(&choice) {
                candidates.push(choice);
            }
            candidates
        });
    let choice_at_constraints = choice_candidates
        .into_iter()
        .filter(|choice| {
            paths.iter().all(|path| {
                path.result
                    .constraints
                    .choice_at_constraints
                    .contains(choice)
                    || position_is_absent(choice.position, &path.result.constraints.arg_constraints)
            })
        })
        .collect();

    ExtractedTagConstraints {
        arg_constraints: project_count_constraints(paths),
        required_keywords,
        choice_at_constraints,
    }
}

fn project_count_constraints(paths: &[&ExecutionState]) -> Vec<ArgumentCountConstraint> {
    let finite = paths
        .iter()
        .map(|path| finite_counts(&path.result.constraints.arg_constraints))
        .collect::<Vec<_>>();
    if finite.iter().all(Option::is_some) {
        let mut counts = finite.into_iter().flatten().flatten().collect::<Vec<_>>();
        counts.sort_unstable();
        counts.dedup();
        return match counts.as_slice() {
            [] => Vec::new(),
            [count] => vec![ArgumentCountConstraint::Exact(*count)],
            _ => vec![ArgumentCountConstraint::OneOf(counts)],
        };
    }

    let lower_bounds = paths
        .iter()
        .map(|path| lower_bound(&path.result.constraints.arg_constraints))
        .collect::<Option<Vec<_>>>();
    let upper_bounds = paths
        .iter()
        .map(|path| upper_bound(&path.result.constraints.arg_constraints))
        .collect::<Option<Vec<_>>>();
    let mut projected = Vec::new();
    if let Some(bound) = lower_bounds.and_then(|bounds| bounds.into_iter().min()) {
        projected.push(ArgumentCountConstraint::Min(bound));
    }
    if let Some(bound) = upper_bounds.and_then(|bounds| bounds.into_iter().max()) {
        projected.push(ArgumentCountConstraint::Max(bound));
    }
    projected
}

fn finite_counts(constraints: &[ArgumentCountConstraint]) -> Option<Vec<usize>> {
    let mut finite: Option<Vec<usize>> = None;
    for constraint in constraints {
        let values = match constraint {
            ArgumentCountConstraint::Exact(count) => Some(vec![*count]),
            ArgumentCountConstraint::OneOf(counts) => Some(counts.clone()),
            ArgumentCountConstraint::Min(_) | ArgumentCountConstraint::Max(_) => None,
        };
        if let Some(values) = values {
            finite = Some(match finite {
                Some(current) => current
                    .into_iter()
                    .filter(|count| values.contains(count))
                    .collect(),
                None => values,
            });
        }
    }
    let mut finite = finite?;
    finite.retain(|count| count_satisfies(*count, constraints));
    Some(finite)
}

fn count_satisfies(count: usize, constraints: &[ArgumentCountConstraint]) -> bool {
    constraints.iter().all(|constraint| match constraint {
        ArgumentCountConstraint::Exact(expected) => count == *expected,
        ArgumentCountConstraint::Min(minimum) => count >= *minimum,
        ArgumentCountConstraint::Max(maximum) => count <= *maximum,
        ArgumentCountConstraint::OneOf(expected) => expected.contains(&count),
    })
}

fn lower_bound(constraints: &[ArgumentCountConstraint]) -> Option<usize> {
    constraints
        .iter()
        .filter_map(|constraint| match constraint {
            ArgumentCountConstraint::Exact(count) | ArgumentCountConstraint::Min(count) => {
                Some(*count)
            }
            ArgumentCountConstraint::OneOf(counts) => counts.iter().min().copied(),
            ArgumentCountConstraint::Max(_) => None,
        })
        .max()
}

fn upper_bound(constraints: &[ArgumentCountConstraint]) -> Option<usize> {
    constraints
        .iter()
        .filter_map(|constraint| match constraint {
            ArgumentCountConstraint::Exact(count) | ArgumentCountConstraint::Max(count) => {
                Some(*count)
            }
            ArgumentCountConstraint::OneOf(counts) => counts.iter().max().copied(),
            ArgumentCountConstraint::Min(_) => None,
        })
        .min()
}

fn position_is_absent(position: SplitPosition, constraints: &[ArgumentCountConstraint]) -> bool {
    let Some(maximum_count) = upper_bound(constraints) else {
        return false;
    };
    match position {
        SplitPosition::Forward(position) => maximum_count <= position,
        SplitPosition::Backward(_) => maximum_count <= 1,
    }
}

fn project_diagnostics(
    paths: &[&ExecutionState],
    constraints: &ExtractedTagConstraints,
) -> Vec<ExtractedDiagnosticMessage> {
    if paths.is_empty() {
        return Vec::new();
    }
    let candidates = paths
        .iter()
        .flat_map(|path| path.result.diagnostic_messages.iter().cloned())
        .fold(Vec::new(), |mut candidates, message| {
            if !candidates.contains(&message) {
                candidates.push(message);
            }
            candidates
        });
    let mut messages = candidates
        .into_iter()
        .filter(|message| {
            paths
                .iter()
                .all(|path| path.result.diagnostic_messages.contains(message))
                || (diagnostic_constraint_is_projected(&message.constraint, constraints)
                    && paths
                        .iter()
                        .all(|path| diagnostic_is_proven_on_path(message, path)))
        })
        .collect::<Vec<_>>();

    if let [projected_count] = constraints.arg_constraints.as_slice()
        && let Some(message) = common_count_message(paths)
    {
        let projected = ExtractedDiagnosticMessage {
            constraint: ExtractedDiagnosticConstraint::ArgumentCount(projected_count.clone()),
            message,
        };
        if !messages.contains(&projected) {
            messages.push(projected);
        }
    }
    messages
}

fn diagnostic_is_proven_on_path(
    diagnostic: &ExtractedDiagnosticMessage,
    path: &ExecutionState,
) -> bool {
    if path.result.diagnostic_messages.contains(diagnostic) {
        return true;
    }
    match &diagnostic.constraint {
        ExtractedDiagnosticConstraint::RequiredKeyword { position, .. }
        | ExtractedDiagnosticConstraint::ChoiceAt { position, .. } => {
            position_is_absent(*position, &path.result.constraints.arg_constraints)
        }
        ExtractedDiagnosticConstraint::ArgumentCount(constraint) => {
            count_constraints_imply(&path.result.constraints.arg_constraints, constraint)
        }
    }
}

fn count_constraints_imply(
    path: &[ArgumentCountConstraint],
    projected: &ArgumentCountConstraint,
) -> bool {
    if let Some(counts) = finite_counts(path) {
        return !counts.is_empty()
            && counts
                .into_iter()
                .all(|count| count_satisfies(count, std::slice::from_ref(projected)));
    }
    match projected {
        ArgumentCountConstraint::Min(minimum) => {
            lower_bound(path).is_some_and(|bound| bound >= *minimum)
        }
        ArgumentCountConstraint::Max(maximum) => {
            upper_bound(path).is_some_and(|bound| bound <= *maximum)
        }
        ArgumentCountConstraint::Exact(_) | ArgumentCountConstraint::OneOf(_) => false,
    }
}

fn common_count_message(paths: &[&ExecutionState]) -> Option<ExtractedMessageTemplate> {
    let per_path = paths
        .iter()
        .map(|path| {
            let mut messages = path
                .result
                .diagnostic_messages
                .iter()
                .filter_map(|diagnostic| match &diagnostic.constraint {
                    ExtractedDiagnosticConstraint::ArgumentCount(_) => Some(&diagnostic.message),
                    ExtractedDiagnosticConstraint::RequiredKeyword { .. }
                    | ExtractedDiagnosticConstraint::ChoiceAt { .. } => None,
                });
            let first = messages.next()?;
            messages
                .all(|message| message == first)
                .then(|| first.clone())
        })
        .collect::<Option<Vec<_>>>()?;
    let first = per_path.first()?;
    per_path
        .iter()
        .all(|message| message == first)
        .then(|| first.clone())
}

fn diagnostic_constraint_is_projected(
    diagnostic: &ExtractedDiagnosticConstraint,
    constraints: &ExtractedTagConstraints,
) -> bool {
    match diagnostic {
        ExtractedDiagnosticConstraint::ArgumentCount(count) => {
            constraints.arg_constraints.contains(count)
        }
        ExtractedDiagnosticConstraint::RequiredKeyword { position, value } => constraints
            .required_keywords
            .iter()
            .any(|keyword| keyword.position == *position && keyword.value == *value),
        ExtractedDiagnosticConstraint::ChoiceAt { position, values } => constraints
            .choice_at_constraints
            .iter()
            .any(|choice| choice.position == *position && choice.values == *values),
    }
}

fn static_truthiness(expr: &Expr) -> Option<bool> {
    if let Some(value) = expr.bool_literal() {
        return Some(value);
    }
    let Expr::UnaryOp(unary) = expr else {
        return None;
    };
    if matches!(unary.op, ruff_python_ast::UnaryOp::Not) {
        static_truthiness(&unary.operand).map(|value| !value)
    } else {
        None
    }
}

fn process_statement(stmt: &Stmt, env: &mut Env, ctx: &mut CallContext<'_>) -> AnalysisResult {
    match stmt {
        Stmt::Assign(StmtAssign { targets, value, .. }) => {
            let rhs = eval_expr_with_ctx(value, env, Some(ctx));
            if let [target] = targets.as_slice() {
                process_assignment_target(target, &rhs, env);
            }
        }
        Stmt::Expr(stmt_expr) => {
            // Expression evaluation owns mutation effects such as `pop()`.
            eval_expr_with_ctx(&stmt_expr.value, env, Some(ctx));
        }
        Stmt::FunctionDef(_)
        | Stmt::ClassDef(_)
        | Stmt::If(_)
        | Stmt::With(_)
        | Stmt::For(_)
        | Stmt::While(_)
        | Stmt::Match(_)
        | Stmt::Try(_)
        | Stmt::Return(_)
        | Stmt::Delete(_)
        | Stmt::TypeAlias(_)
        | Stmt::AugAssign(_)
        | Stmt::AnnAssign(_)
        | Stmt::Raise(_)
        | Stmt::Assert(_)
        | Stmt::Import(_)
        | Stmt::ImportFrom(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Pass(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::IpyEscapeCommand(_) => {}
    }

    AnalysisResult::default()
}

/// Try to detect `token_kwargs(bits, parser)` calls and return the first
/// argument name so we can mark it as Unknown (`token_kwargs` mutates bits).
fn try_extract_token_kwargs_call(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.func.name_target() != Some("token_kwargs") {
        return None;
    }
    // First argument is the bits variable that gets mutated
    call.arguments
        .args
        .first()
        .and_then(ExprExt::name_target)
        .map(str::to_string)
}

/// Process an assignment target with the evaluated RHS value.
fn process_assignment_target(target: &Expr, value: &AbstractValue, env: &mut Env) {
    if let Some(name) = target.name_target() {
        env.set(name.to_string(), value.clone());
        return;
    }

    if let Expr::Tuple(ExprTuple { elts, .. }) = target {
        process_tuple_unpack(elts, value, env);
    }
}

/// Handle tuple unpacking assignment.
fn process_tuple_unpack(targets: &[Expr], value: &AbstractValue, env: &mut Env) {
    match value {
        AbstractValue::Tuple(elements) => {
            for (i, target) in targets.iter().enumerate() {
                let elem = elements.get(i).cloned().unwrap_or(AbstractValue::Unknown);
                if let Some(name) = target.name_target() {
                    env.set(name.to_string(), elem);
                }
            }
        }

        AbstractValue::SplitResult(split) => {
            let split = *split;

            // Find starred target index
            let star_index = targets.iter().position(|t| matches!(t, Expr::Starred(_)));

            if let Some(si) = star_index {
                // Elements before the star
                for (i, target) in targets[..si].iter().enumerate() {
                    if let Some(name) = target.name_target() {
                        env.set(
                            name.to_string(),
                            AbstractValue::SplitElement {
                                index: split.resolve_index(i),
                            },
                        );
                    }
                }

                // Elements after the star (indexed from end)
                let after_star = targets.len() - si - 1;

                // The star target captures everything between pre-star and post-star elements.
                // Its back_offset must include the trailing targets it doesn't contain.
                if let Expr::Starred(starred) = &targets[si]
                    && let Some(name) = starred.value.name_target()
                {
                    // Start from the current split sliced past the pre-star targets,
                    // which preserves the original back_offset.
                    let mut star_split = split.after_slice_from(si);
                    // Add trailing targets as additional back pops
                    for _ in 0..after_star {
                        star_split = star_split.after_pop_back();
                    }
                    env.set(name.to_string(), AbstractValue::SplitResult(star_split));
                }
                for (j, target) in targets[si + 1..].iter().enumerate() {
                    if let Some(name) = target.name_target() {
                        env.set(
                            name.to_string(),
                            AbstractValue::SplitElement {
                                index: SplitPosition::Backward(after_star - j),
                            },
                        );
                    }
                }
            } else {
                // No star: each target gets a SplitElement at its position
                for (i, target) in targets.iter().enumerate() {
                    if let Some(name) = target.name_target() {
                        env.set(
                            name.to_string(),
                            AbstractValue::SplitElement {
                                index: split.resolve_index(i),
                            },
                        );
                    }
                }
            }
        }

        AbstractValue::Unknown
        | AbstractValue::Token
        | AbstractValue::Parser
        | AbstractValue::SplitElement { .. }
        | AbstractValue::SplitLength(_)
        | AbstractValue::Int(_)
        | AbstractValue::Str(_) => {
            for target in targets {
                if let Some(name) = target.name_target() {
                    env.set(name.to_string(), AbstractValue::Unknown);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_ast::StmtFunctionDef;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::templates::tags::analysis::state::AbstractValue;
    use crate::templates::tags::analysis::state::Env;
    use crate::templates::tags::analysis::state::TokenSplit;
    use crate::templates::tags::testing::django_function;
    use crate::templates::tags::types::OptionRejection;
    use crate::templates::tags::types::SplitPosition;

    fn parse_function(source: &str) -> StmtFunctionDef {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        for stmt in module.body {
            if let Stmt::FunctionDef(func_def) = stmt {
                return func_def;
            }
        }
        panic!("no function definition found in source");
    }

    fn eval_body(source: &str) -> Env {
        let func = parse_function(source);
        let parser_param = func
            .parameters
            .args
            .first()
            .map_or("parser", |p| p.parameter.name.as_str());
        let token_param = func
            .parameters
            .args
            .get(1)
            .map_or("token", |p| p.parameter.name.as_str());
        let mut env = Env::for_compile_function(parser_param, token_param);
        let mut ctx = CallContext {
            db: None,
            file: None,
        };
        process_statements(&func.body, &mut env, &mut ctx);
        env
    }

    #[test]
    fn deduplicate_states_preserves_first_occurrence_order() {
        let path = |value| {
            let mut env = Env::default();
            env.set("value".to_string(), AbstractValue::Int(value));
            ExecutionState::from_env(env)
        };
        let first = path(1);
        let second = path(2);
        let third = path(3);
        let mut paths = vec![
            first.clone(),
            second.clone(),
            first.clone(),
            third.clone(),
            second.clone(),
        ];

        deduplicate_states(&mut paths);

        assert_eq!(paths, vec![first, second, third]);
    }

    #[test]
    fn normalization_caps_multiple_destinations_without_losing_an_outcome() {
        let outcomes = [
            ControlOutcome::Next,
            ControlOutcome::Return(AbstractValue::Int(1)),
            ControlOutcome::Raise,
            ControlOutcome::Break,
            ControlOutcome::Continue,
        ];
        let mut completed = Vec::new();
        let mut active = Vec::new();
        for (index, outcome) in (0..100).zip(outcomes.iter().cycle()) {
            let mut env = Env::default();
            env.set("path".to_string(), AbstractValue::Int(index));
            let state = ExecutionState {
                env,
                result: AnalysisResult::default(),
                outcome: outcome.clone(),
            };
            if index % 2 == 0 {
                completed.push(state);
            } else {
                active.push(state);
            }
        }

        normalize_state_destinations(&mut [&mut completed, &mut active]);

        assert_eq!(completed.len() + active.len(), MAX_EXEC_STATES);
        for expected in outcomes {
            assert!(
                completed
                    .iter()
                    .chain(&active)
                    .any(|state| outcome_index(&state.outcome) == outcome_index(&expected))
            );
        }
    }

    #[test]
    fn return_expression_applies_pop_once() {
        let function = parse_function(
            "def compile(parser, token):\n    bits = token.split_contents()\n    return bits.pop(0)\n",
        );
        let mut env = Env::for_compile_function("parser", "token");
        let mut ctx = CallContext {
            db: None,
            file: None,
        };
        let (_, value) = process_statements(&function.body, &mut env, &mut ctx);
        assert_eq!(
            value,
            AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_front())
        );
    }

    #[test]
    fn env_initialization() {
        let source = "def do_tag(parser, token): pass";
        let func = parse_function(source);
        let env = Env::for_compile_function(
            func.parameters.args[0].parameter.name.as_str(),
            func.parameters.args[1].parameter.name.as_str(),
        );
        assert_eq!(env.get("parser"), &AbstractValue::Parser);
        assert_eq!(env.get("token"), &AbstractValue::Token);
        assert_eq!(env.get("nonexistent"), &AbstractValue::Unknown);
    }

    #[test]
    fn split_contents_binding() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn contents_split_binding() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    args = token.contents.split()
",
        );
        assert_eq!(
            env.get("args"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    // Fabricated: tests `parser.token.split_contents()` pattern (classytags-
    // style). Real Django compile functions use `token.split_contents()` but
    // third-party libraries access token via parser. Keep as unit test. (b)
    #[test]
    fn parser_token_split_contents() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = parser.token.split_contents()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn subscript_forward() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    tag_name = bits[0]
    item = bits[2]
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(2)
            }
        );
    }

    #[test]
    fn subscript_negative() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    last = bits[-1]
",
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Backward(1)
            }
        );
    }

    #[test]
    fn slice_from_start() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
    }

    #[test]
    fn slice_with_existing_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[2:]
    more = rest[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(2))
        );
        assert_eq!(
            env.get("more"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(3))
        );
    }

    #[test]
    fn len_of_split_result() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength(TokenSplit::fresh())
        );
    }

    #[test]
    fn list_wrapping() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits = list(bits)
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn star_unpack() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, *rest = token.split_contents()
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
    }

    #[test]
    fn tuple_unpack() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    a, b, c = (1, 'x', None)
",
        );
        assert_eq!(env.get("a"), &AbstractValue::Int(1));
        assert_eq!(env.get("b"), &AbstractValue::Str("x".to_string()));
        assert_eq!(env.get("c"), &AbstractValue::Unknown);
    }

    #[test]
    fn contents_split_none_1() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, rest = token.contents.split(None, 1)
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(env.get("rest"), &AbstractValue::Unknown);
    }

    #[test]
    fn unknown_variable() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    x = some_function()
",
        );
        assert_eq!(env.get("x"), &AbstractValue::Unknown);
    }

    #[test]
    fn split_result_tuple_unpack_no_star() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, item, connector, varname = token.split_contents()
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(1)
            }
        );
        assert_eq!(
            env.get("connector"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(2)
            }
        );
        assert_eq!(
            env.get("varname"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(3)
            }
        );
    }

    #[test]
    fn subscript_with_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[1:]
    second = rest[0]
",
        );
        assert_eq!(
            env.get("second"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(1)
            }
        );
    }

    #[test]
    fn if_branch_updates_env() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    if True:
        rest = bits[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
    }

    #[test]
    fn integer_literal() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    n = 42
",
        );
        assert_eq!(env.get("n"), &AbstractValue::Int(42));
    }

    #[test]
    fn string_literal() {
        let env = eval_body(
            r#"
def do_tag(parser, token):
    s = "hello"
"#,
        );
        assert_eq!(env.get("s"), &AbstractValue::Str("hello".to_string()));
    }

    #[test]
    fn slice_truncation_preserves_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits = bits[1:]
    truncated = bits[:3]
",
        );
        assert_eq!(
            env.get("truncated"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
    }

    #[test]
    fn star_unpack_with_trailing() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    first, *middle, last = token.split_contents()
",
        );
        assert_eq!(
            env.get("first"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        // middle = original[1:-1], so base_offset=1 and pops_from_end=1
        // (the trailing `last` element is accounted for in pops_from_end)
        assert_eq!(
            env.get("middle"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1).after_pop_back())
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Backward(1)
            }
        );
    }

    #[test]
    fn pop_0_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
    }

    #[test]
    fn pop_zero_with_assignment() {
        for index in ["0", "-0"] {
            let env = eval_body(&format!(
                "def do_tag(parser, token):\n    bits = token.split_contents()\n    tag_name = bits.pop({index})\n"
            ));
            assert_eq!(
                env.get("tag_name"),
                &AbstractValue::SplitElement {
                    index: SplitPosition::Forward(0)
                }
            );
            assert_eq!(
                env.get("bits"),
                &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
            );
        }
    }

    #[test]
    fn pop_from_end() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_back())
        );
    }

    #[test]
    fn pop_from_end_with_assignment() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    last = bits.pop()
",
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Backward(1)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_back())
        );
    }

    #[test]
    fn explicit_pop_minus_one_tracks_return_and_mutation() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
    last = bits.pop(-1)
",
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Backward(2)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_back().after_pop_back())
        );
    }

    #[test]
    fn untracked_pop_discards_return_and_remaining_positions() {
        for call in [
            "bits.pop(2)",
            "bits.pop(-2)",
            "bits.pop(index)",
            "bits.pop(*indices)",
            "bits.pop(0, 1)",
            "bits.pop(index=0)",
        ] {
            let env = eval_body(&format!(
                "def do_tag(parser, token):\n    bits = token.split_contents()\n    popped = {call}\n    following = bits[1]\n"
            ));
            assert_eq!(env.get("popped"), &AbstractValue::Unknown, "{call}");
            assert_eq!(env.get("bits"), &AbstractValue::Unknown, "{call}");
            assert_eq!(env.get("following"), &AbstractValue::Unknown, "{call}");
        }
    }

    #[test]
    fn untracked_pop_statement_discards_remaining_positions() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(2)
    following = bits[1]
",
        );
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
        assert_eq!(env.get("following"), &AbstractValue::Unknown);
    }

    #[test]
    fn multiple_pops() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    bits.pop()
    bits.pop()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(
                TokenSplit::fresh()
                    .after_pop_front()
                    .after_pop_back()
                    .after_pop_back()
            )
        );
    }

    #[test]
    fn len_after_pop() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength(TokenSplit::fresh().after_pop_front())
        );
    }

    #[test]
    fn len_after_end_pop() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
    bits.pop()
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength(TokenSplit::fresh().after_pop_back().after_pop_back())
        );
    }

    fn analyze(source: &str) -> crate::templates::tags::types::TagRule {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        let func = module
            .body
            .into_iter()
            .find_map(|s| {
                if let Stmt::FunctionDef(f) = s {
                    Some(f)
                } else {
                    None
                }
            })
            .expect("no function found");
        crate::templates::tags::analysis::analyze_compile_function(&func)
    }

    fn analyze_func(func: &StmtFunctionDef) -> crate::templates::tags::types::TagRule {
        crate::templates::tags::analysis::analyze_compile_function(func)
    }

    // Fabricated: simple option loop without duplicate checking. No corpus
    // function covers this simpler extraction path. (b)
    #[test]
    fn option_loop_basic() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining_bits = bits[2:]
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option == "with":
            pass
        elif option == "only":
            pass
        else:
            raise TemplateSyntaxError("unknown option")
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert_eq!(opts.duplicate_rejection, OptionRejection::NotDetected);
        assert_eq!(opts.unknown_rejection, OptionRejection::Detected);
    }

    // Corpus: do_translate in i18n.py — option loop with `seen = set()`
    // duplicate check. Options: "noop", "context", "as".
    #[test]
    fn option_loop_with_duplicate_check() {
        let func = django_function("django/templatetags/i18n.py", "do_translate")
            .expect("expected Django fixture function should exist");
        let rule = analyze_func(&func);
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(
            opts.values,
            vec!["noop".to_string(), "context".to_string(), "as".to_string()]
        );
        assert_eq!(opts.duplicate_rejection, OptionRejection::Detected);
        assert_eq!(opts.unknown_rejection, OptionRejection::Detected);
    }

    // Corpus: do_include in loader_tags.py — option loop with dict-based
    // duplicate check (`if option in options:`). Options: "with", "only".
    #[test]
    fn option_loop_include_pattern() {
        let func = django_function("django/template/loader_tags.py", "do_include")
            .expect("expected Django fixture function should exist");
        let rule = analyze_func(&func);
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert_eq!(opts.duplicate_rejection, OptionRejection::Detected);
        assert_eq!(opts.unknown_rejection, OptionRejection::Detected);
    }

    #[test]
    fn option_loop_without_rejection_guards() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while remaining:
        option = remaining.pop(0)
        if option == "noescape":
            pass
        elif option == "trimmed":
            pass
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["noescape", "trimmed"]);
        assert_eq!(opts.duplicate_rejection, OptionRejection::NotDetected);
        assert_eq!(opts.unknown_rejection, OptionRejection::NotDetected);
    }

    #[test]
    fn option_loop_membership_without_raise_is_not_duplicate_rejection() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    remaining = token.split_contents()[1:]
    seen = set()
    while remaining:
        option = remaining.pop(0)
        if option in seen:
            continue
        if option == "only":
            seen.add(option)
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["only"]);
        assert_eq!(opts.duplicate_rejection, OptionRejection::NotDetected);
        assert_eq!(opts.unknown_rejection, OptionRejection::NotDetected);
    }

    #[test]
    fn option_loop_duplicate_rejection_does_not_imply_unknown_rejection() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    remaining = token.split_contents()[1:]
    seen = set()
    while remaining:
        option = remaining.pop(0)
        if option in seen:
            raise TemplateSyntaxError("duplicate option")
        if option == "only":
            seen.add(option)
        else:
            continue
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["only"]);
        assert_eq!(opts.duplicate_rejection, OptionRejection::Detected);
        assert_eq!(opts.unknown_rejection, OptionRejection::NotDetected);
    }

    #[test]
    fn no_option_loop_returns_none() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError("err")
"#,
        );
        assert!(rule.known_options.is_none());
    }

    // Corpus: partialdef_func in defaulttags.py — match statement with
    // multiple case arms of different lengths (2 and 3 elements), producing
    // OneOf([2, 3]) constraint. Django 6.0+ match-based tag parsing.
    #[test]
    fn match_partialdef_pattern() {
        let func = django_function("django/template/defaulttags.py", "partialdef_func")
            .expect("expected Django fixture function should exist");
        let rule = analyze_func(&func);
        assert!(
            rule.arg_constraints.contains(
                &crate::templates::tags::types::ArgumentCountConstraint::OneOf(vec![2, 3])
            ),
            "expected OneOf([2, 3]), got {:?}",
            rule.arg_constraints
        );
    }

    // Corpus: partial_func in defaulttags.py — match statement with a
    // single fixed-length case (2 elements) + wildcard error, producing
    // Exact(2) constraint.
    #[test]
    fn match_partial_exact() {
        let func = django_function("django/template/defaulttags.py", "partial_func")
            .expect("expected Django fixture function should exist");
        let rule = analyze_func(&func);
        assert!(
            rule.arg_constraints
                .contains(&crate::templates::tags::types::ArgumentCountConstraint::Exact(2)),
            "expected Exact(2), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_non_split_result_no_constraints() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    x = something()
    match x:
        case "a":
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints.is_empty(),
            "non-SplitResult match should produce no constraints"
        );
    }

    // Fabricated: match with star pattern (`case "tag", *rest:`). No corpus
    // function uses star patterns in match arms currently. Keep as unit test
    // for variable-length match handling. (b)
    #[test]
    fn match_star_pattern_variable_length() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", *rest:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&crate::templates::tags::types::ArgumentCountConstraint::Min(1)),
            "expected Min(1), got {:?}",
            rule.arg_constraints
        );
    }

    // Fabricated: match with multiple fixed-length non-error arms of
    // different sizes (2 and 4 elements). Tests OneOf constraint from
    // match. No corpus function has this exact pattern. (b)
    #[test]
    fn match_multiple_valid_lengths() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a:
            pass
        case "tag", a, b, c:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints.contains(
                &crate::templates::tags::types::ArgumentCountConstraint::OneOf(vec![2, 4])
            ),
            "expected OneOf([2, 4]), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_all_error_cases_no_constraints() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag":
            raise TemplateSyntaxError("bad")
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints.is_empty(),
            "all-error match should produce no constraints, got {:?}",
            rule.arg_constraints
        );
    }

    // Fabricated: wildcard match arm overrides variable-length minimum.
    // Tests that `case _: pass` (non-error) removes Min constraint. (b)
    #[test]
    fn match_wildcard_overrides_variable_min_to_zero() {
        // When a Variable arm (min_len=2) appears before a non-error Wildcard,
        // the wildcard should unconditionally set the minimum to 0 since it
        // matches anything including zero-length inputs.
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a, *rest:
            pass
        case _:
            pass
"#,
        );
        // Wildcard `case _:` is a valid (non-error) arm that matches anything,
        // so there should be no Min constraint at all (min is effectively 0).
        assert!(
            !rule.arg_constraints.iter().any(|c| matches!(
                c,
                crate::templates::tags::types::ArgumentCountConstraint::Min(_)
            )),
            "wildcard should override variable min to 0 (no Min constraint), got {:?}",
            rule.arg_constraints
        );
    }

    // Fabricated: non-error wildcard after fixed-length arm prevents Min
    // constraint. Tests wildcard catch-all semantics in match. (b)
    #[test]
    fn match_wildcard_after_fixed_produces_no_min() {
        // A non-error wildcard means any length is valid, so even fixed-length
        // arms shouldn't produce exact/range constraints when a wildcard is present.
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a, b:
            pass
        case _:
            pass
"#,
        );
        // The wildcard is non-error, so it acts as a variable-length catch-all.
        // With min=0, no Min constraint should be emitted.
        assert!(
            !rule.arg_constraints.iter().any(
                |c| matches!(c, crate::templates::tags::types::ArgumentCountConstraint::Min(m) if *m > 0)
            ),
            "non-error wildcard should prevent Min constraint > 0, got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_env_updates_propagate() {
        let env = eval_body(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", name:
            result = name
"#,
        );
        // The match body should have processed assignments
        assert_eq!(env.get("result"), &AbstractValue::Unknown);
    }

    #[test]
    fn while_body_assignments_propagate() {
        // Non-option while loop: body should be processed for env updates
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while some_condition:
        val = remaining.pop(0)
",
        );
        // The pop(0) assignment inside the while body should be processed
        assert_eq!(
            env.get("val"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(1)
            }
        );
        // The pop(0) side effect should also mutate `remaining`
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(2))
        );
    }

    #[test]
    fn while_body_pop_side_effects() {
        // Non-option while loop: pop side effects should be tracked
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[2:]
    while some_condition:
        remaining.pop(0)
",
        );
        // The pop(0) inside the while body should mutate `remaining`
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(3))
        );
    }

    #[test]
    fn contents_split_none_2_is_not_tuple() {
        // split(None, 2) should NOT be modeled as a 2-tuple;
        // only split(None, 1) has the special 2-tuple treatment.
        let env = eval_body(
            r"
def do_tag(parser, token):
    result = token.contents.split(None, 2)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn contents_split_none_0_is_not_tuple() {
        // split(None, 0) should NOT be modeled as a 2-tuple.
        let env = eval_body(
            r"
def do_tag(parser, token):
    result = token.contents.split(None, 0)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn contents_split_none_variable_is_not_tuple() {
        // split(None, some_var) should NOT be modeled as a 2-tuple.
        let env = eval_body(
            r"
def do_tag(parser, token):
    n = 1
    result = token.contents.split(None, n)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn while_option_loop_skips_body_processing() {
        // Option loop pattern: body should NOT be processed to avoid
        // the loop variable appearing as a false positional argument
        let env = eval_body(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[2:]
    while remaining:
        option = remaining.pop(0)
        if option == "with":
            pass
        elif option == "only":
            pass
        else:
            raise TemplateSyntaxError("unknown")
"#,
        );
        // `option` should NOT have a SplitElement value since the
        // option loop body is not processed (to avoid false positives)
        assert_eq!(env.get("option"), &AbstractValue::Unknown);
        // `remaining` should keep its pre-loop value
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(2))
        );
    }
}
