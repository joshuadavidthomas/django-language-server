use std::collections::BTreeSet;
use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprList;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::analysis::AnalysisResult;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::constraints::ExtractedTagConstraints;
use crate::templates::tags::analysis::exceptions::direct_raise_exception;
use crate::templates::tags::analysis::exceptions::extract_exception_message;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::analysis::expressions::eval_expr_with_ctx;
use crate::templates::tags::analysis::match_arms::extract_match_constraints;
use crate::templates::tags::analysis::mutations::PopInfo;
use crate::templates::tags::analysis::mutations::try_extract_option_loop;
use crate::templates::tags::analysis::mutations::try_extract_pop_call;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::analysis::state::SplitPredicate;
use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::ExtractedDiagnosticConstraint;
use crate::templates::tags::types::ExtractedDiagnosticMessage;
use crate::templates::tags::types::ExtractedMessageTemplate;
use crate::templates::tags::types::SplitPosition;
use crate::templates::tags::types::TagArgumentSyntax;

const MAX_EXEC_STATES: usize = 64;

/// One feasible path through a compile function. Facts stay attached to the
/// environment that established them until all accepting paths are projected.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum PathAssumption {
    LengthNotEquals(usize),
    ElementNotEquals {
        position: SplitPosition,
        value: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltinException {
    AssertionError,
    AttributeError,
    BaseException,
    Exception,
    ImportError,
    IndexError,
    KeyError,
    NameError,
    OSError,
    RuntimeError,
    StopIteration,
    SyntaxError,
    TypeError,
    ValueError,
}

impl BuiltinException {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "AssertionError" => Some(Self::AssertionError),
            "AttributeError" => Some(Self::AttributeError),
            "BaseException" => Some(Self::BaseException),
            "Exception" => Some(Self::Exception),
            "ImportError" => Some(Self::ImportError),
            "IndexError" => Some(Self::IndexError),
            "KeyError" => Some(Self::KeyError),
            "NameError" => Some(Self::NameError),
            "OSError" => Some(Self::OSError),
            "RuntimeError" => Some(Self::RuntimeError),
            "StopIteration" => Some(Self::StopIteration),
            "SyntaxError" => Some(Self::SyntaxError),
            "TypeError" => Some(Self::TypeError),
            "ValueError" => Some(Self::ValueError),
            _ => None,
        }
    }

    fn catches(self, pending: Self) -> bool {
        matches!(self, Self::BaseException | Self::Exception) || self == pending
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PendingException {
    Builtin(BuiltinException),
    Unpack(ArgumentCountConstraint),
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
struct RaisedException {
    kind: PendingException,
    message: Option<ExtractedMessageTemplate>,
}

impl RaisedException {
    fn implicit(kind: PendingException) -> Self {
        Self {
            kind,
            message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ControlOutcome {
    Next,
    Return(AbstractValue),
    Raise(RaisedException),
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq)]
struct ExecutionState {
    outcome: ControlOutcome,
    env: Env,
    result: AnalysisResult,
    exclusions: Vec<PathAssumption>,
}

impl ExecutionState {
    fn from_env(env: Env) -> Self {
        Self {
            env,
            result: AnalysisResult::default(),
            exclusions: Vec::new(),
            outcome: ControlOutcome::Next,
        }
    }

    fn assume(mut self, predicate: &SplitPredicate, truth: bool) -> Option<Self> {
        match (predicate, truth) {
            (SplitPredicate::LengthEquals(length), true) => {
                self.result
                    .constraints
                    .extend(ExtractedTagConstraints::single_length(
                        ArgumentCountConstraint::Exact(*length),
                    ));
            }
            (SplitPredicate::LengthEquals(length), false) => {
                self.exclusions
                    .push(PathAssumption::LengthNotEquals(*length));
            }
            (SplitPredicate::LengthAtLeast(length), true) if *length > 1 => self
                .result
                .constraints
                .extend(ExtractedTagConstraints::single_length(
                    ArgumentCountConstraint::Min(*length),
                )),
            (SplitPredicate::LengthAtLeast(length), false) if *length > 0 => self
                .result
                .constraints
                .extend(ExtractedTagConstraints::single_length(
                    ArgumentCountConstraint::Max(length - 1),
                )),
            (SplitPredicate::ElementEquals { position, value }, true) => {
                let keyword = crate::templates::tags::types::RequiredKeyword {
                    position: *position,
                    value: value.clone(),
                };
                if !self.result.constraints.required_keywords.contains(&keyword) {
                    self.result.constraints.required_keywords.push(keyword);
                }
            }
            (SplitPredicate::ElementEquals { position, value }, false) => {
                self.exclusions.push(PathAssumption::ElementNotEquals {
                    position: *position,
                    value: value.clone(),
                });
            }
            (SplitPredicate::LengthAtLeast(0), false) => return None,
            (SplitPredicate::LengthAtLeast(_), _) => {}
        }

        if !path_facts_are_feasible(&self.result.constraints, &self.exclusions) {
            return None;
        }
        self.exclusions.dedup();
        Some(self)
    }
}

fn path_facts_are_feasible(
    constraints: &ExtractedTagConstraints,
    exclusions: &[PathAssumption],
) -> bool {
    if finite_counts(&constraints.arg_constraints).is_some_and(|counts| counts.is_empty()) {
        return false;
    }
    let lower = lower_bound(&constraints.arg_constraints).unwrap_or(1);
    let upper = upper_bound(&constraints.arg_constraints);
    if upper.is_some_and(|upper| lower > upper) {
        return false;
    }
    if upper == Some(lower)
        && exclusions.iter().any(
            |assumption| matches!(assumption, PathAssumption::LengthNotEquals(length) if *length == lower),
        )
    {
        return false;
    }

    let exact_length = finite_counts(&constraints.arg_constraints).and_then(|counts| {
        let [length] = counts.as_slice() else {
            return None;
        };
        Some(*length)
    });
    let exact_arguments_len = exact_length.and_then(|length| length.checked_sub(1));
    if exact_length == Some(0)
        || exact_arguments_len.is_some_and(|arguments_len| {
            constraints
                .required_keywords
                .iter()
                .map(|keyword| keyword.position)
                .chain(exclusions.iter().filter_map(|assumption| match assumption {
                    PathAssumption::ElementNotEquals { position, .. } => Some(*position),
                    PathAssumption::LengthNotEquals(_) => None,
                }))
                .any(|position| position.to_bits_index(arguments_len).is_none())
        })
    {
        return false;
    }

    for (index, left) in constraints.required_keywords.iter().enumerate() {
        if constraints.required_keywords[index + 1..]
            .iter()
            .any(|right| {
                positions_alias(left.position, right.position, exact_length)
                    && left.value != right.value
            })
            || exclusions.iter().any(|assumption| match assumption {
                PathAssumption::ElementNotEquals { position, value } => {
                    positions_alias(left.position, *position, exact_length) && left.value == *value
                }
                PathAssumption::LengthNotEquals(_) => false,
            })
        {
            return false;
        }
    }
    true
}

fn positions_alias(left: SplitPosition, right: SplitPosition, exact_length: Option<usize>) -> bool {
    left == right
        || exact_length.is_some_and(|length| {
            let arguments_len = length.saturating_sub(1);
            left.to_bits_index(arguments_len).is_some()
                && left.to_bits_index(arguments_len) == right.to_bits_index(arguments_len)
        })
}

/// Feasible paths at one statement boundary. Each state owns one control
/// destination, so environments and evidence cannot cross between exits.
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

fn normalize_state_destinations(destinations: &mut [&mut Vec<ExecutionState>]) {
    for states in &mut *destinations {
        deduplicate_states(states);
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
    for (destination, states) in destinations.iter_mut().enumerate() {
        let mut by_outcome = [const { Vec::new() }; 5];
        for state in states.drain(..) {
            by_outcome[outcome_index(&state.outcome)].push(state);
        }
        groups.extend(
            by_outcome
                .into_iter()
                .filter(|group| !group.is_empty())
                .map(|group| (destination, group)),
        );
    }
    let mut budgets = vec![1; groups.len()];
    let mut remaining = MAX_EXEC_STATES - groups.len();
    while remaining > 0 {
        let mut added = false;
        for (budget, (_, group)) in budgets.iter_mut().zip(&groups) {
            if *budget < group.len() {
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

    for ((destination, mut group), budget) in groups.into_iter().zip(budgets) {
        if group.len() > budget
            && let Some(summary) = summarize_states(&group)
        {
            group.truncate(budget - 1);
            group.push(summary);
        }
        destinations[destination].append(&mut group);
    }
}

fn outcome_index(outcome: &ControlOutcome) -> usize {
    match outcome {
        ControlOutcome::Next => 0,
        ControlOutcome::Return(_) => 1,
        ControlOutcome::Raise(_) => 2,
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

fn summarize_states(states: &[ExecutionState]) -> Option<ExecutionState> {
    let mut summary = states.first()?.clone();
    summary.env = Env::join_exact(states.iter().map(|state| &state.env));
    summary.result = project_common_results(&states.iter().collect::<Vec<_>>());
    summary.exclusions = summarize_exclusions(states);
    summary.outcome = match &summary.outcome {
        ControlOutcome::Return(first)
            if states.iter().all(
                |state| matches!(&state.outcome, ControlOutcome::Return(value) if value == first),
            ) =>
        {
            ControlOutcome::Return(first.clone())
        }
        ControlOutcome::Return(_) => ControlOutcome::Return(AbstractValue::Unknown),
        ControlOutcome::Raise(first)
            if states.iter().all(
                |state| matches!(&state.outcome, ControlOutcome::Raise(value) if value == first),
            ) =>
        {
            ControlOutcome::Raise(first.clone())
        }
        ControlOutcome::Raise(_) => {
            ControlOutcome::Raise(RaisedException::implicit(PendingException::Unknown))
        }
        ControlOutcome::Next => ControlOutcome::Next,
        ControlOutcome::Break => ControlOutcome::Break,
        ControlOutcome::Continue => ControlOutcome::Continue,
    };
    Some(summary)
}

fn summarize_exclusions(states: &[ExecutionState]) -> Vec<PathAssumption> {
    let Some(first) = states.first() else {
        return Vec::new();
    };
    first
        .exclusions
        .iter()
        .filter(|exclusion| {
            states
                .iter()
                .all(|state| state.exclusions.contains(exclusion))
        })
        .cloned()
        .collect()
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
        ControlOutcome::Raise(_) | ControlOutcome::Break | ControlOutcome::Continue => None,
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
            Stmt::Raise(raised) => {
                for state in states.next_mut() {
                    state.outcome = ControlOutcome::Raise(RaisedException {
                        kind: raised_exception_kind(raised, &state.env),
                        message: raised
                            .exc
                            .as_deref()
                            .and_then(|exception| extract_exception_message(exception, &state.env)),
                    });
                }
            }
            Stmt::Assign(assign) => {
                let incoming = states.take_next();
                for state in incoming {
                    if matches!(assign.value.as_ref(), Expr::If(_))
                        && let Some(conditional) =
                            analyze_conditional_assignment(assign, &state.env)
                    {
                        states
                            .0
                            .extend(branch_conditional_assignment(conditional, state));
                    } else {
                        states.0.extend(execute_assignment(assign, state, ctx));
                    }
                }
            }
            Stmt::For(stmt_for) => states = branch_for(stmt_for, states, ctx),
            Stmt::With(stmt_with) => states = branch_with(stmt_with, states, ctx),
            Stmt::Expr(stmt_expr) => {
                for state in states.next_mut() {
                    process_expression_statement(stmt_expr, &mut state.env);
                }
            }
            Stmt::While(stmt_while) => states = branch_while(stmt_while, states, ctx),
            Stmt::Match(stmt_match) => states = branch_match(stmt_match, states, ctx),
            Stmt::Pass(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Assert(_)
            | Stmt::IpyEscapeCommand(_) => {}
            Stmt::AnnAssign(_)
            | Stmt::FunctionDef(_)
            | Stmt::ClassDef(_)
            | Stmt::Delete(_)
            | Stmt::TypeAlias(_)
            | Stmt::AugAssign(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_) => {
                let changes = PotentialEnvChanges::collect(std::slice::from_ref(stmt));
                for state in states.next_mut() {
                    state.env = changes.apply(&state.env);
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
        }
        states.normalize();
    }

    states
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
                let mut iteration = ExecutionStates(Vec::new());
                for active_state in active {
                    let assigned = execute_assignment_targets(
                        std::slice::from_ref(stmt_for.target.as_ref()),
                        &value,
                        active_state,
                    );
                    let mut branch =
                        process_statement_states(&stmt_for.body, ExecutionStates(assigned), ctx);
                    iteration.0.append(&mut branch.0);
                }
                active = Vec::new();
                for mut state in iteration.0 {
                    match state.outcome {
                        ControlOutcome::Next | ControlOutcome::Continue => {
                            state.outcome = ControlOutcome::Next;
                            active.push(state);
                        }
                        ControlOutcome::Break => {
                            state.outcome = ControlOutcome::Next;
                            states.0.push(state);
                        }
                        ControlOutcome::Return(_) | ControlOutcome::Raise(_) => {
                            states.0.push(state);
                        }
                    }
                }
                normalize_state_destinations(&mut [&mut states.0, &mut active]);
                if active.is_empty() {
                    break;
                }
            }
            let mut after_loop =
                process_statement_states(&stmt_for.orelse, ExecutionStates(active), ctx);
            states.0.append(&mut after_loop.0);
            continue;
        }

        let body_changes = PotentialEnvChanges::collect(&stmt_for.body);
        let mut exhausted = vec![state.clone()];
        let mut target_changes = PotentialEnvChanges::default();
        target_changes.record_target(&stmt_for.target);
        let mut body_entry = state;
        body_entry.env = target_changes.apply(&body_entry.env);
        body_entry.outcome = ControlOutcome::Next;
        let body = process_statement_states(&stmt_for.body, ExecutionStates(vec![body_entry]), ctx);
        let mut repeatable = Vec::new();
        for mut state in body.0 {
            match state.outcome {
                ControlOutcome::Next | ControlOutcome::Continue => {
                    state.outcome = ControlOutcome::Next;
                    repeatable.push(state);
                }
                ControlOutcome::Break => {
                    state.outcome = ControlOutcome::Next;
                    states.0.push(state);
                }
                ControlOutcome::Return(_) | ControlOutcome::Raise(_) => states.0.push(state),
            }
        }
        for repeat_state in &repeatable {
            let mut widened = repeat_state.clone();
            widened.env = body_changes.apply(&widened.env);
            exhausted.push(widened);
        }
        exhausted.append(&mut repeatable);
        let mut after_loop =
            process_statement_states(&stmt_for.orelse, ExecutionStates(exhausted), ctx);
        states.0.append(&mut after_loop.0);
    }
    states
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
        state.outcome = ControlOutcome::Next;
        let mut body = process_statement_states(&stmt_with.body, ExecutionStates(vec![state]), ctx);
        states.0.append(&mut body.0);
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

        let body_changes = PotentialEnvChanges::collect(&stmt_while.body);
        let can_exhaust = static_truthiness(&stmt_while.test) != Some(true);
        let mut exhausted = if can_exhaust {
            vec![state.clone()]
        } else {
            Vec::new()
        };
        state.outcome = ControlOutcome::Next;
        let body = process_statement_states(&stmt_while.body, ExecutionStates(vec![state]), ctx);
        let mut repeatable = Vec::new();
        for mut state in body.0 {
            match state.outcome {
                ControlOutcome::Next | ControlOutcome::Continue => {
                    state.outcome = ControlOutcome::Next;
                    repeatable.push(state);
                }
                ControlOutcome::Break => {
                    state.outcome = ControlOutcome::Next;
                    states.0.push(state);
                }
                ControlOutcome::Return(_) | ControlOutcome::Raise(_) => states.0.push(state),
            }
        }

        // Execute one widened repeat. Its entry state summarizes all body
        // changes, so another unroll cannot establish a sound hard fact.
        let mut later = ExecutionStates(Vec::new());
        for repeat_state in &repeatable {
            let mut widened = repeat_state.clone();
            widened.env = body_changes.apply(&widened.env);
            if can_exhaust {
                exhausted.push(widened.clone());
            }
            widened.outcome = ControlOutcome::Next;
            let mut branch =
                process_statement_states(&stmt_while.body, ExecutionStates(vec![widened]), ctx);
            later.0.append(&mut branch.0);
        }

        for mut state in later.0 {
            match state.outcome {
                ControlOutcome::Next | ControlOutcome::Continue => {
                    state.outcome = ControlOutcome::Next;
                    if can_exhaust {
                        exhausted.push(state);
                    }
                }
                ControlOutcome::Break => {
                    state.outcome = ControlOutcome::Next;
                    states.0.push(state);
                }
                ControlOutcome::Return(_) | ControlOutcome::Raise(_) => states.0.push(state),
            }
        }
        if can_exhaust {
            exhausted.append(&mut repeatable);
        }
        let mut after_loop =
            process_statement_states(&stmt_while.orelse, ExecutionStates(exhausted), ctx);
        states.0.append(&mut after_loop.0);
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

        if stmt_match.cases.is_empty() {
            states.0.push(state);
            continue;
        }

        for case in &stmt_match.cases {
            let mut branch =
                process_statement_states(&case.body, ExecutionStates(vec![state.clone()]), ctx);
            states.0.append(&mut branch.0);
        }
    }
    states
}

struct ConditionalAssignment<'a> {
    target: &'a str,
    predicate: SplitPredicate,
    condition_truth: bool,
    true_value: AbstractValue,
    false_value: AbstractValue,
}

fn analyze_conditional_assignment<'a>(
    assign: &'a StmtAssign,
    env: &Env,
) -> Option<ConditionalAssignment<'a>> {
    let [target] = assign.targets.as_slice() else {
        return None;
    };
    let Expr::If(conditional) = assign.value.as_ref() else {
        return None;
    };
    let (predicate, condition_truth) =
        crate::templates::tags::analysis::guards::split_predicate_condition(
            &conditional.test,
            env,
        )?;
    let true_value = eval_expr(&conditional.body, &mut env.clone());
    let false_value = eval_expr(&conditional.orelse, &mut env.clone());
    if !matches!(&true_value, AbstractValue::Int(_))
        || !matches!(&false_value, AbstractValue::Int(_))
    {
        return None;
    }
    Some(ConditionalAssignment {
        target: target.name_target()?,
        predicate,
        condition_truth,
        true_value,
        false_value,
    })
}

fn branch_conditional_assignment(
    conditional: ConditionalAssignment<'_>,
    path: ExecutionState,
) -> Vec<ExecutionState> {
    let ConditionalAssignment {
        target,
        predicate,
        condition_truth,
        true_value,
        false_value,
    } = conditional;
    let mut alternatives = Vec::with_capacity(2);
    if let Some(mut taken) = path.clone().assume(&predicate, condition_truth) {
        taken.env.set(target.to_string(), true_value);
        alternatives.push(taken);
    }
    if let Some(mut path) = path.assume(&predicate, !condition_truth) {
        path.env.set(target.to_string(), false_value);
        alternatives.push(path);
    }
    alternatives
}

#[allow(clippy::too_many_lines)]
fn branch_if(
    stmt_if: &ruff_python_ast::StmtIf,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    let mut alternatives = states;

    for mut path in incoming {
        if let Some(argument_syntax) =
            crate::templates::tags::analysis::forms::extract_if_argument_syntax(
                stmt_if, &path.env, ctx,
            )
        {
            path.result.extend(AnalysisResult {
                argument_syntax: Some(argument_syntax),
                ..AnalysisResult::default()
            });
        }

        let mut unmatched = Some(path);
        let mut clauses = Vec::with_capacity(stmt_if.elif_else_clauses.len() + 1);
        clauses.push((Some(stmt_if.test.as_ref()), stmt_if.body.as_slice()));
        clauses.extend(
            stmt_if
                .elif_else_clauses
                .iter()
                .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
        );

        for (test, body) in clauses {
            let Some(path) = unmatched.take() else {
                break;
            };
            let truth = test.and_then(static_truthiness);
            if truth == Some(false) {
                unmatched = Some(path);
                continue;
            }

            let predicate = test.and_then(|test| {
                crate::templates::tags::analysis::guards::split_predicate_condition(test, &path.env)
            });
            let has_direct_raise = body.iter().any(|stmt| matches!(stmt, Stmt::Raise(_)));
            let direct_guard =
                test.filter(|_| direct_raise_exception(body).is_some())
                    .map(|test| {
                        crate::templates::tags::analysis::guards::extract_direct_guard(
                            test, body, &path.env,
                        )
                    });
            let taken = path.clone();
            let taken = match predicate.as_ref() {
                Some((predicate, condition_truth)) => taken.assume(predicate, *condition_truth),
                None => Some(taken),
            };
            if let Some(mut taken) = taken {
                if let Some(test) = test.filter(|_| !has_direct_raise) {
                    taken.result.constraints.extend(
                        crate::templates::tags::analysis::guards::extract_true_condition_constraints(
                            test, &taken.env,
                        ),
                    );
                }
                taken.outcome = ControlOutcome::Next;
                let mut branch = process_statement_states(body, ExecutionStates(vec![taken]), ctx);
                alternatives.0.append(&mut branch.0);
            }

            if truth != Some(true) && test.is_some() {
                let fallthrough = path;
                let fallthrough = match predicate.as_ref() {
                    Some((predicate, condition_truth)) => {
                        fallthrough.assume(predicate, !condition_truth)
                    }
                    None => Some(fallthrough),
                };
                if let Some(mut fallthrough) = fallthrough {
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
        }

        if let Some(path) = unmatched {
            alternatives.0.push(path);
        }
    }

    alternatives
}

#[derive(Clone, Default)]
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
            | Expr::Name(_)
            | Expr::Slice(_)
            | Expr::IpyEscapeCommand(_) => self.forget_all = true,
            Expr::Attribute(attribute) => {
                if let Some(name) = assignment_base_name(&attribute.value) {
                    self.assigned_names.insert(name.to_string());
                } else {
                    self.forget_all = true;
                }
            }
            Expr::Subscript(subscript) => {
                if let Some(name) = assignment_base_name(&subscript.value) {
                    self.assigned_names.insert(name.to_string());
                } else {
                    self.forget_all = true;
                }
            }
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

fn assignment_base_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str()),
        Expr::Attribute(attribute) => assignment_base_name(&attribute.value),
        Expr::Subscript(subscript) => assignment_base_name(&subscript.value),
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
        | Expr::Starred(_)
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => None,
    }
}

fn raised_exception_kind(raised: &ruff_python_ast::StmtRaise, env: &Env) -> PendingException {
    let Some(exception) = raised.exc.as_deref() else {
        return PendingException::Unknown;
    };
    let exception_type = if let Expr::Call(call) = exception {
        call.func.as_ref()
    } else {
        exception
    };
    let Some(name) = exception_type.name_target() else {
        return PendingException::Unknown;
    };
    if !env.builtin_name_visible(name) {
        return PendingException::Unknown;
    }
    BuiltinException::from_name(name).map_or(PendingException::Unknown, PendingException::Builtin)
}

#[derive(Clone, Copy)]
enum HandlerMatch {
    Always,
    Maybe,
    Never,
}

fn exception_handler_match(
    pending: &PendingException,
    handler_type: Option<&Expr>,
    env: &Env,
) -> HandlerMatch {
    let Some(handler_type) = handler_type else {
        return HandlerMatch::Always;
    };
    let pending = match pending {
        PendingException::Builtin(kind) => kind,
        PendingException::Unpack(_) => &BuiltinException::ValueError,
        PendingException::Unknown => return HandlerMatch::Maybe,
    };

    match handler_type {
        Expr::Name(name) => {
            let name = name.id.as_str();
            if !env.builtin_name_visible(name) {
                return HandlerMatch::Maybe;
            }
            BuiltinException::from_name(name).map_or(HandlerMatch::Maybe, |handler| {
                if handler.catches(*pending) {
                    HandlerMatch::Always
                } else {
                    HandlerMatch::Never
                }
            })
        }
        Expr::Tuple(tuple) => {
            let mut maybe = false;
            for item in &tuple.elts {
                match exception_handler_match(&PendingException::Builtin(*pending), Some(item), env)
                {
                    HandlerMatch::Always => return HandlerMatch::Always,
                    HandlerMatch::Maybe => maybe = true,
                    HandlerMatch::Never => {}
                }
            }
            if maybe {
                HandlerMatch::Maybe
            } else {
                HandlerMatch::Never
            }
        }
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
        | Expr::Starred(_)
        | Expr::List(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => HandlerMatch::Maybe,
    }
}

#[allow(clippy::too_many_lines)]
fn branch_try(
    stmt_try: &ruff_python_ast::StmtTry,
    mut states: ExecutionStates,
    ctx: &mut CallContext<'_>,
) -> ExecutionStates {
    let incoming = states.take_next();
    // Outcomes reached before this try do not enter its finalizer.
    let mut alternatives = ExecutionStates(Vec::new());

    for state in incoming {
        let body_changes = PotentialEnvChanges::collect(&stmt_try.body);
        let body_may_raise_implicitly =
            suite_may_raise_implicitly(&stmt_try.body, &state.env, &body_changes);
        let mut body_entry = state.clone();
        body_entry.outcome = ControlOutcome::Next;
        let mut body =
            process_statement_states(&stmt_try.body, ExecutionStates(vec![body_entry]), ctx);
        let mut pending_exceptions =
            body.take_outcome(|outcome| matches!(outcome, ControlOutcome::Raise(_)));
        let normal = ExecutionStates(body.take_next());
        let mut completed = ExecutionStates(body.0);
        let mut success = process_statement_states(&stmt_try.orelse, normal, ctx);
        completed.0.append(&mut success.0);

        if body_may_raise_implicitly {
            let mut implicit_exception = state.clone();
            implicit_exception.env = body_changes.apply(&state.env);
            implicit_exception.outcome =
                ControlOutcome::Raise(RaisedException::implicit(PendingException::Unknown));
            pending_exceptions.push(implicit_exception);
        }
        for exception in &stmt_try.handlers {
            let ruff_python_ast::ExceptHandler::ExceptHandler(clause) = exception;
            let mut residual = Vec::new();
            for mut raised in pending_exceptions {
                let pending = match &raised.outcome {
                    ControlOutcome::Raise(pending) => pending.clone(),
                    ControlOutcome::Next
                    | ControlOutcome::Return(_)
                    | ControlOutcome::Break
                    | ControlOutcome::Continue => {
                        alternatives.0.push(raised);
                        continue;
                    }
                };
                let handler_match =
                    exception_handler_match(&pending.kind, clause.type_.as_deref(), &raised.env);
                if !matches!(handler_match, HandlerMatch::Always) {
                    residual.push(raised.clone());
                }
                if !matches!(handler_match, HandlerMatch::Never) {
                    let records_unpack_message = matches!(handler_match, HandlerMatch::Always)
                        && handler_names_builtin_value_error(clause.type_.as_deref(), &raised.env);
                    raised.outcome = ControlOutcome::Next;
                    let mut handled =
                        process_statement_states(&clause.body, ExecutionStates(vec![raised]), ctx);
                    if records_unpack_message
                        && let PendingException::Unpack(constraint) = pending.kind
                        && let Some(message) = common_raised_message(&handled.0)
                    {
                        let diagnostic = ExtractedDiagnosticMessage {
                            constraint: ExtractedDiagnosticConstraint::ArgumentCount(constraint),
                            message,
                        };
                        for completed in &mut completed.0 {
                            if !completed.result.diagnostic_messages.contains(&diagnostic) {
                                completed
                                    .result
                                    .diagnostic_messages
                                    .push(diagnostic.clone());
                            }
                        }
                    }
                    alternatives.0.append(&mut handled.0);
                }
            }
            pending_exceptions = residual;
            if pending_exceptions.is_empty() {
                break;
            }
        }
        alternatives.0.append(&mut completed.0);
        alternatives.0.append(&mut pending_exceptions);

        if !stmt_try.finalbody.is_empty() {
            let mut finalizer_changes = body_changes;
            finalizer_changes.extend(&stmt_try.orelse);
            for handler in &stmt_try.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
                finalizer_changes.extend(&handler.body);
                if let Some(name) = &handler.name {
                    finalizer_changes.assigned_names.insert(name.to_string());
                }
            }
            let preceding_suite_may_raise = body_may_raise_implicitly
                || suite_may_raise_implicitly(&stmt_try.orelse, &state.env, &finalizer_changes)
                || stmt_try.handlers.iter().any(|handler| {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
                    suite_may_raise_implicitly(&handler.body, &state.env, &finalizer_changes)
                });
            if preceding_suite_may_raise {
                let mut implicit_raise = state.clone();
                implicit_raise.env = finalizer_changes.apply(&state.env);
                implicit_raise.outcome =
                    ControlOutcome::Raise(RaisedException::implicit(PendingException::Unknown));
                alternatives.0.push(implicit_raise);
            }
        }
    }

    alternatives.normalize();
    let mut result = if stmt_try.finalbody.is_empty() {
        alternatives
    } else {
        run_finally(&stmt_try.finalbody, alternatives, ctx)
    };
    result.0.append(&mut states.0);
    result
}

fn suite_may_raise_implicitly(stmts: &[Stmt], env: &Env, changes: &PotentialEnvChanges) -> bool {
    struct MayRaise<'env> {
        env: &'env Env,
        found: bool,
    }

    impl<'a> Visitor<'a> for MayRaise<'_> {
        fn visit_stmt(&mut self, statement: &'a Stmt) {
            match statement {
                Stmt::Raise(_) => return,
                Stmt::Import(_)
                | Stmt::ImportFrom(_)
                | Stmt::For(_)
                | Stmt::While(_)
                | Stmt::With(_)
                | Stmt::Match(_)
                | Stmt::Assert(_)
                | Stmt::AugAssign(_)
                | Stmt::Delete(_)
                | Stmt::ClassDef(_) => {
                    self.found = true;
                    return;
                }
                Stmt::FunctionDef(_)
                | Stmt::TypeAlias(_)
                | Stmt::If(_)
                | Stmt::Try(_)
                | Stmt::Return(_)
                | Stmt::Assign(_)
                | Stmt::AnnAssign(_)
                | Stmt::Expr(_)
                | Stmt::Global(_)
                | Stmt::Nonlocal(_)
                | Stmt::Pass(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::IpyEscapeCommand(_) => {}
            }
            visitor::walk_stmt(self, statement);
        }

        fn visit_expr(&mut self, expression: &'a Expr) {
            if let Expr::Call(call) = expression
                && call.arguments.args.is_empty()
                && call.arguments.keywords.is_empty()
                && let Expr::Attribute(attribute) = call.func.as_ref()
                && attribute.attr.as_str() == "split_contents"
                && attribute
                    .value
                    .name_target()
                    .is_some_and(|name| matches!(self.env.get(name), AbstractValue::Token))
            {
                // The normal evaluator models this intrinsic, including the
                // unpack failure it can produce. Do not add a second unknown
                // exception edge for its call or callee attribute.
                return;
            }
            if matches!(
                expression,
                Expr::Call(_)
                    | Expr::Attribute(_)
                    | Expr::Subscript(_)
                    | Expr::Named(_)
                    | Expr::Await(_)
                    | Expr::Yield(_)
                    | Expr::YieldFrom(_)
            ) {
                self.found = true;
                return;
            }
            visitor::walk_expr(self, expression);
        }
    }

    let widened_env = changes.apply(env);
    let mut may_raise = MayRaise {
        env: &widened_env,
        found: false,
    };
    for statement in stmts {
        may_raise.visit_stmt(statement);
        if may_raise.found {
            return true;
        }
    }
    false
}

fn handler_names_builtin_value_error(handler_type: Option<&Expr>, env: &Env) -> bool {
    let Some(handler_type) = handler_type else {
        return false;
    };
    match handler_type {
        Expr::Name(name) => {
            name.id.as_str() == "ValueError" && env.builtin_name_visible("ValueError")
        }
        Expr::Tuple(tuple) => {
            tuple.elts.iter().all(|item| {
                item.name_target().is_some_and(|name| {
                    env.builtin_name_visible(name) && BuiltinException::from_name(name).is_some()
                })
            }) && tuple
                .elts
                .iter()
                .any(|item| item.name_target() == Some("ValueError"))
        }
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
        | Expr::Starred(_)
        | Expr::List(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => false,
    }
}

fn common_raised_message(states: &[ExecutionState]) -> Option<ExtractedMessageTemplate> {
    let mut messages = states.iter().map(|state| match &state.outcome {
        ControlOutcome::Raise(raised) => raised.message.as_ref(),
        ControlOutcome::Next
        | ControlOutcome::Return(_)
        | ControlOutcome::Break
        | ControlOutcome::Continue => None,
    });
    let first = messages.next()??;
    messages
        .all(|message| message == Some(first))
        .then(|| first.clone())
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
            // object with the finalizer. We cannot preserve that alias here.
            value.forget_mutable();
        }
        let finalized = process_statement_states(finalbody, ExecutionStates(vec![state]), ctx);
        after_finally
            .0
            .extend(finalized.0.into_iter().map(|mut state| {
                if matches!(state.outcome, ControlOutcome::Next) {
                    state.outcome = saved.clone();
                }
                state
            }));
    }
    after_finally.normalize();
    after_finally
}

fn project_results(paths: &[&ExecutionState]) -> AnalysisResult {
    let mut result = project_common_results(paths);
    if matches!(
        result.argument_syntax,
        Some(TagArgumentSyntax::Forms {
            coverage: crate::templates::tags::types::ArgumentFormCoverage::Complete,
            ..
        })
    ) {
        result.constraints = ExtractedTagConstraints::default();
    }
    result
}

fn project_common_results(paths: &[&ExecutionState]) -> AnalysisResult {
    let Some(first) = paths.first() else {
        return AnalysisResult::default();
    };

    let constraints = project_constraints(paths);
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
        SplitPosition::Forward(position) | SplitPosition::Backward(position) => {
            maximum_count <= position
        }
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

fn execute_assignment(
    assign: &StmtAssign,
    mut state: ExecutionState,
    ctx: &mut CallContext<'_>,
) -> Vec<ExecutionState> {
    let StmtAssign { targets, value, .. } = assign;
    let pop_info = try_extract_pop_call(value);
    let invalidates_split = expression_may_mutate_split(value, &state.env)
        || unsupported_container_captures_split(value, &state.env)
        || pop_info.as_ref().is_some_and(PopInfo::is_untracked);
    let rhs = eval_expr_with_ctx(value, &mut state.env, Some(ctx));

    if invalidates_split {
        state.env.forget_split_results();
    }
    execute_assignment_targets(targets, &rhs, state)
}

fn execute_assignment_targets(
    targets: &[Expr],
    rhs: &AbstractValue,
    state: ExecutionState,
) -> Vec<ExecutionState> {
    let mut active = vec![state];
    let mut completed = Vec::new();
    for target in targets {
        let mut next = Vec::new();
        for mut state in active {
            let unpack_failure = if let Some(constraint) = split_unpack_constraint(target, rhs) {
                let predicate = match &constraint {
                    ArgumentCountConstraint::Exact(length) => SplitPredicate::LengthEquals(*length),
                    ArgumentCountConstraint::Min(length) => SplitPredicate::LengthAtLeast(*length),
                    ArgumentCountConstraint::Max(_) | ArgumentCountConstraint::OneOf(_) => {
                        next.push(state);
                        continue;
                    }
                };

                let success = state.clone().assume(&predicate, true);
                let failure = state.assume(&predicate, false).map(|mut state| {
                    state.outcome = ControlOutcome::Raise(RaisedException {
                        kind: PendingException::Unpack(constraint),
                        message: None,
                    });
                    state
                });
                let Some(success) = success else {
                    if let Some(failure) = failure {
                        completed.push(failure);
                    }
                    continue;
                };
                state = success;
                failure
            } else {
                None
            };

            let assignment = process_assignment_target(target, rhs, &mut state.env);
            match assignment.failure {
                AssignmentFailure::Never => next.push(state),
                AssignmentFailure::Maybe => {
                    let mut raised = state.clone();
                    let mut changes = PotentialEnvChanges::default();
                    changes.record_target(target);
                    raised.env = changes.apply(&raised.env);
                    raised.outcome =
                        ControlOutcome::Raise(RaisedException::implicit(PendingException::Unknown));
                    completed.push(raised);
                    next.push(state);
                }
                AssignmentFailure::Definite => {
                    state.outcome = ControlOutcome::Raise(RaisedException::implicit(
                        PendingException::Builtin(BuiltinException::ValueError),
                    ));
                    completed.push(state);
                }
            }
            if let Some(failure) = unpack_failure {
                completed.push(failure);
            }
        }
        normalize_state_destinations(&mut [&mut completed, &mut next]);
        active = next;
        if active.is_empty() {
            break;
        }
    }
    completed.extend(active);
    completed
}

fn assignment_sequence_elements(target: &Expr) -> Option<&[Expr]> {
    match target {
        Expr::Tuple(tuple) => Some(&tuple.elts),
        Expr::List(list) => Some(&list.elts),
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
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => None,
    }
}

fn split_unpack_constraint(
    target: &Expr,
    value: &AbstractValue,
) -> Option<ArgumentCountConstraint> {
    let AbstractValue::SplitResult(split) = value else {
        return None;
    };
    let targets = assignment_sequence_elements(target)?;
    if targets.is_empty() {
        return None;
    }
    let fixed = targets
        .iter()
        .filter(|target| !matches!(target, Expr::Starred(_)))
        .count();
    Some(if fixed == targets.len() {
        ArgumentCountConstraint::Exact(split.resolve_length(fixed))
    } else {
        ArgumentCountConstraint::Min(split.resolve_length(fixed))
    })
}

fn process_expression_statement(stmt_expr: &ruff_python_ast::StmtExpr, env: &mut Env) {
    let pop_info = try_extract_pop_call(&stmt_expr.value);
    let invalidates_split = expression_may_mutate_split(&stmt_expr.value, env)
        || pop_info.as_ref().is_some_and(PopInfo::is_untracked);
    eval_expr(&stmt_expr.value, env);
    if invalidates_split {
        env.forget_split_results();
    }
}

fn unsupported_container_captures_split(expr: &Expr, env: &Env) -> bool {
    matches!(expr, Expr::List(_) | Expr::Set(_) | Expr::Dict(_))
        && expression_contains_split(expr, env)
}

fn expression_may_mutate_split(expr: &Expr, env: &Env) -> bool {
    struct EscapingSplit<'a> {
        env: &'a Env,
        found: bool,
    }

    impl<'a> Visitor<'a> for EscapingSplit<'_> {
        fn visit_expr(&mut self, expression: &'a Expr) {
            let Expr::Call(call) = expression else {
                visitor::walk_expr(self, expression);
                return;
            };

            if try_extract_pop_call(expression).is_none() {
                let safe_builtin = call.func.name_target().is_some_and(|name| {
                    matches!(name, "len" | "list") && self.env.builtin_name_visible(name)
                });
                let (split_receiver, read_only_method) =
                    if let Expr::Attribute(attribute) = call.func.as_ref() {
                        let receiver = eval_expr(&attribute.value, &mut self.env.clone());
                        (
                            matches!(receiver, AbstractValue::SplitResult(_)),
                            matches!(receiver, AbstractValue::Str(_))
                                && attribute.attr.as_str() == "join",
                        )
                    } else {
                        (false, false)
                    };
                let split_argument =
                    !safe_builtin
                        && !read_only_method
                        && (call
                            .arguments
                            .args
                            .iter()
                            .any(|argument| expression_contains_split(argument, self.env))
                            || call.arguments.keywords.iter().any(|keyword| {
                                expression_contains_split(&keyword.value, self.env)
                            }));
                if split_receiver || split_argument {
                    self.found = true;
                    return;
                }
            }
            visitor::walk_expr(self, expression);
        }
    }

    let mut escaping = EscapingSplit { env, found: false };
    escaping.visit_expr(expr);
    escaping.found
}

fn expression_contains_split(expr: &Expr, env: &Env) -> bool {
    fn value_contains_split(value: &AbstractValue) -> bool {
        match value {
            AbstractValue::SplitResult(_) => true,
            AbstractValue::Tuple(values) => values.iter().any(value_contains_split),
            AbstractValue::Unknown
            | AbstractValue::Token
            | AbstractValue::Parser
            | AbstractValue::SplitElement { .. }
            | AbstractValue::SplitLength(_)
            | AbstractValue::SplitPredicate(_)
            | AbstractValue::Int(_)
            | AbstractValue::Str(_) => false,
        }
    }

    if value_contains_split(&eval_expr(expr, &mut env.clone())) {
        return true;
    }
    match expr {
        Expr::Tuple(tuple) => tuple
            .elts
            .iter()
            .any(|element| expression_contains_split(element, env)),
        Expr::List(list) => list
            .elts
            .iter()
            .any(|element| expression_contains_split(element, env)),
        Expr::Set(set) => set
            .elts
            .iter()
            .any(|element| expression_contains_split(element, env)),
        Expr::Starred(starred) => expression_contains_split(&starred.value, env),
        Expr::Dict(dict) => dict.items.iter().any(|item| {
            item.key
                .as_ref()
                .is_some_and(|key| expression_contains_split(key, env))
                || expression_contains_split(&item.value, env)
        }),
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::Compare(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
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
        | Expr::IpyEscapeCommand(_) => false,
    }
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssignmentFailure {
    Never,
    Maybe,
    Definite,
}

#[derive(Clone, Copy)]
struct AssignmentTargetResult {
    failure: AssignmentFailure,
}

impl AssignmentTargetResult {
    fn applied() -> Self {
        Self {
            failure: AssignmentFailure::Never,
        }
    }

    fn include(&mut self, result: Self) {
        self.failure = match (self.failure, result.failure) {
            (AssignmentFailure::Definite, _) | (_, AssignmentFailure::Definite) => {
                AssignmentFailure::Definite
            }
            (AssignmentFailure::Maybe, _) | (_, AssignmentFailure::Maybe) => {
                AssignmentFailure::Maybe
            }
            (AssignmentFailure::Never, AssignmentFailure::Never) => AssignmentFailure::Never,
        };
    }
}

/// Process an assignment target with the evaluated RHS value.
fn process_assignment_target(
    target: &Expr,
    value: &AbstractValue,
    env: &mut Env,
) -> AssignmentTargetResult {
    if let Some(name) = target.name_target() {
        env.set(name.to_string(), value.clone());
        return AssignmentTargetResult::applied();
    }

    match target {
        Expr::Tuple(ExprTuple { elts, .. }) | Expr::List(ExprList { elts, .. }) => {
            process_tuple_unpack(elts, value, env)
        }
        Expr::Starred(starred) => process_assignment_target(&starred.value, value, env),
        Expr::Attribute(attribute) => {
            let mut target_env = env.clone();
            match eval_expr(&attribute.value, &mut target_env) {
                AbstractValue::SplitResult(_) => {
                    env.forget_split_results();
                    AssignmentTargetResult::applied()
                }
                AbstractValue::Token => {
                    env.forget_tokens();
                    AssignmentTargetResult::applied()
                }
                AbstractValue::Unknown
                | AbstractValue::Parser
                | AbstractValue::SplitElement { .. }
                | AbstractValue::SplitLength(_)
                | AbstractValue::Int(_)
                | AbstractValue::Str(_)
                | AbstractValue::SplitPredicate(_)
                | AbstractValue::Tuple(_) => AssignmentTargetResult::applied(),
            }
        }
        Expr::Subscript(subscript) => {
            let mut target_env = env.clone();
            let invalidates_split = matches!(
                eval_expr(&subscript.value, &mut target_env),
                AbstractValue::SplitResult(_)
            );
            if invalidates_split {
                env.forget_split_results();
            }
            AssignmentTargetResult::applied()
        }
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
        | Expr::Name(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => AssignmentTargetResult::applied(),
    }
}

/// Handle tuple unpacking assignment in Python's target order.
fn process_tuple_unpack(
    targets: &[Expr],
    value: &AbstractValue,
    env: &mut Env,
) -> AssignmentTargetResult {
    let star_index = targets
        .iter()
        .position(|target| matches!(target, Expr::Starred(_)));
    let fixed = targets.len() - usize::from(star_index.is_some());
    let mut result = AssignmentTargetResult::applied();

    match value {
        AbstractValue::Tuple(elements) => {
            if star_index.map_or(elements.len() != fixed, |_| elements.len() < fixed) {
                result.failure = AssignmentFailure::Definite;
                return result;
            }
            let after_star = star_index.map_or(0, |star| targets.len() - star - 1);
            for (index, target) in targets.iter().enumerate() {
                let element = match star_index {
                    Some(star) if index == star => {
                        AbstractValue::Tuple(elements[star..elements.len() - after_star].to_vec())
                    }
                    Some(star) if index > star => {
                        elements[elements.len() - (targets.len() - index)].clone()
                    }
                    Some(_) | None => elements[index].clone(),
                };
                let assignment = process_assignment_target(target, &element, env);
                let definite = assignment.failure == AssignmentFailure::Definite;
                result.include(assignment);
                if definite {
                    break;
                }
            }
        }
        AbstractValue::SplitResult(split) => {
            let split = *split;
            let after_star = star_index.map_or(0, |star| targets.len() - star - 1);
            for (index, target) in targets.iter().enumerate() {
                let element = match star_index {
                    Some(star) if index == star => {
                        let mut middle = split.after_slice_from(star);
                        for _ in 0..after_star {
                            middle = middle.after_pop_back();
                        }
                        AbstractValue::SplitResult(middle)
                    }
                    Some(star) if index > star => AbstractValue::SplitElement {
                        index: SplitPosition::Backward(targets.len() - index),
                    },
                    Some(_) | None => AbstractValue::SplitElement {
                        index: split.resolve_index(index),
                    },
                };
                result.include(process_assignment_target(target, &element, env));
            }
        }
        AbstractValue::Unknown
        | AbstractValue::Token
        | AbstractValue::Parser
        | AbstractValue::SplitElement { .. }
        | AbstractValue::SplitLength(_)
        | AbstractValue::SplitPredicate(_)
        | AbstractValue::Int(_)
        | AbstractValue::Str(_) => {
            for target in targets {
                result.include(process_assignment_target(
                    target,
                    &AbstractValue::Unknown,
                    env,
                ));
            }
            result.failure = AssignmentFailure::Maybe;
        }
    }
    result
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
        env.set_builtin_name_scope(std::collections::HashSet::new());
        let mut ctx = CallContext {
            db: None,
            file: None,
        };
        process_statements(&func.body, &mut env, &mut ctx);
        env
    }

    #[test]
    fn normalization_caps_multiple_destinations_without_losing_an_outcome() {
        let path = |marker, outcome| {
            let mut env = Env::default();
            env.set("marker".to_string(), AbstractValue::Int(marker));
            let mut state = ExecutionState::from_env(env);
            state.outcome = outcome;
            state
        };
        let mut completed = (0_i64..16)
            .flat_map(|marker| {
                [
                    ControlOutcome::Next,
                    ControlOutcome::Return(AbstractValue::Int(marker)),
                    ControlOutcome::Raise(RaisedException::implicit(PendingException::Unknown)),
                    ControlOutcome::Break,
                    ControlOutcome::Continue,
                ]
                .map(|outcome| path(marker, outcome))
            })
            .collect::<Vec<_>>();
        let mut active = (80_i64..120)
            .map(|marker| path(marker, ControlOutcome::Next))
            .collect::<Vec<_>>();

        normalize_state_destinations(&mut [&mut completed, &mut active]);

        assert_eq!(completed.len() + active.len(), MAX_EXEC_STATES);
        assert!(
            completed
                .iter()
                .any(|state| matches!(state.outcome, ControlOutcome::Next))
        );
        assert!(!active.is_empty());
        assert!(
            completed
                .iter()
                .any(|state| matches!(state.outcome, ControlOutcome::Return(_)))
        );
        assert!(
            completed
                .iter()
                .any(|state| matches!(state.outcome, ControlOutcome::Raise(_)))
        );
        assert!(
            completed
                .iter()
                .any(|state| matches!(state.outcome, ControlOutcome::Break))
        );
        assert!(
            completed
                .iter()
                .any(|state| matches!(state.outcome, ControlOutcome::Continue))
        );
    }

    #[test]
    fn deduplicate_paths_preserves_first_occurrence_order() {
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
    fn unknown_while_body_assignments_are_widened() {
        // A non-option loop may execute zero or many times.
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while some_condition:
        val = remaining.pop(0)
",
        );
        assert_eq!(env.get("val"), &AbstractValue::Unknown);
        assert_eq!(env.get("remaining"), &AbstractValue::Unknown);
    }

    #[test]
    fn unknown_while_body_pop_side_effects_are_widened() {
        // The loop count is unknown, so its final split offset is unknown.
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[2:]
    while some_condition:
        remaining.pop(0)
",
        );
        assert_eq!(env.get("remaining"), &AbstractValue::Unknown);
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
    fn caught_flat_tuple_unpack_proves_exact_arity() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    try:
        tag_name, name = token.split_contents()
    except ValueError:
        message = "requires exactly one argument"
        raise TemplateSyntaxError(message)
"#,
        );
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
        assert!(rule.diagnostic_messages.is_none());
    }

    #[test]
    fn recovering_value_error_handler_does_not_prove_unpack_arity() {
        let rule = analyze(
            r"
def do_tag(parser, token):
    try:
        tag_name, name = token.split_contents()
    except ValueError:
        name = None
",
        );
        assert!(rule.arg_constraints.is_empty());
    }

    #[test]
    fn runtime_operation_before_unpack_keeps_successful_arity_proof() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    try:
        runtime_call()
        tag_name, name = token.split_contents()
    except ValueError:
        raise TemplateSyntaxError("bad")
"#,
        );
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
    }

    #[test]
    fn starred_and_nested_unpack_keep_their_outer_arity_proof() {
        for (target, constraint) in [
            ("tag_name, *names", vec![]),
            (
                "tag_name, first, *names",
                vec![ArgumentCountConstraint::Min(2)],
            ),
            (
                "tag_name, (first, second)",
                vec![ArgumentCountConstraint::Exact(2)],
            ),
        ] {
            let rule = analyze(&format!(
                "def do_tag(parser, token):\n    try:\n        {target} = token.split_contents()\n    except ValueError:\n        raise TemplateSyntaxError('bad')\n"
            ));
            assert_eq!(rule.arg_constraints, constraint, "target: {target}");
        }
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
