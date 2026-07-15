use djls_source::Span;
use ruff_python_ast as ast;

use super::super::PythonBinding;
use super::super::PythonMutationOperation;
use super::super::PythonUnknownCause;
use super::super::control_flow::BranchPath;
use super::super::control_flow::IfBranches;
use super::super::control_flow::Truthiness;
use super::super::control_flow::evaluate_test_with;
use super::super::control_flow::if_branches;
use super::super::control_flow::is_irrefutable_match_case;
use super::super::control_flow::try_paths;
use super::super::extend_ordered_unique;
use super::super::mutation;
use super::super::mutation::MutationTarget;
use super::super::touched_names::TouchedNames;
use super::super::touched_names::collect_touched_names;
use super::super::touched_names::first_import_segment;
use super::super::touched_names::pattern_bound_names;
use super::super::touched_names::target_write_names;
use super::EvaluationContext;
use super::EvaluationState;
use super::expression;
use super::imports;
use crate::ast::ExprExt;
use crate::ast::RangedExt;

struct SemanticEvaluator<'db> {
    context: EvaluationContext<'db>,
}

pub(super) fn evaluate_body(
    context: EvaluationContext<'_>,
    state: EvaluationState,
    body: &[ast::Stmt],
) -> EvaluationState {
    walk_body(&mut SemanticEvaluator { context }, state, body)
}

fn walk_body(
    evaluator: &mut SemanticEvaluator<'_>,
    mut state: EvaluationState,
    body: &[ast::Stmt],
) -> EvaluationState {
    for stmt in body {
        state = walk_stmt(evaluator, state, stmt);
    }
    state
}

fn walk_stmt(
    evaluator: &mut SemanticEvaluator<'_>,
    mut state: EvaluationState,
    stmt: &ast::Stmt,
) -> EvaluationState {
    match stmt {
        ast::Stmt::Assign(assign) => evaluator.walk_assign(&mut state, assign),
        ast::Stmt::AnnAssign(assign) => evaluator.walk_ann_assign(&mut state, assign),
        ast::Stmt::AugAssign(assign) => evaluator.walk_aug_assign(&mut state, assign),
        ast::Stmt::Expr(expr) => mutation::walk_expr(&evaluator.context, &mut state, &expr.value),
        ast::Stmt::Import(import) => evaluator.walk_import(&mut state, import),
        ast::Stmt::ImportFrom(import) => {
            imports::walk_import_from(&evaluator.context, &mut state, import);
        }
        ast::Stmt::If(stmt_if) => return walk_if(evaluator, state, stmt_if),
        ast::Stmt::For(stmt_for) => {
            evaluator.bind_for_target(&mut state, &stmt_for.target);
            return evaluator.degrade_loop_bodies(
                state,
                &[&stmt_for.body, &stmt_for.orelse],
                stmt_for.span(),
            );
        }
        ast::Stmt::While(stmt_while) => {
            return match evaluate_test(&state, &stmt_while.test) {
                Truthiness::AlwaysFalse => walk_body(evaluator, state, &stmt_while.orelse),
                Truthiness::AlwaysTrue | Truthiness::Ambiguous => evaluator.degrade_loop_bodies(
                    state,
                    &[&stmt_while.body, &stmt_while.orelse],
                    stmt_while.span(),
                ),
            };
        }
        ast::Stmt::With(stmt_with) => {
            for item in &stmt_with.items {
                if let Some(optional_vars) = &item.optional_vars {
                    evaluator.bind_for_target(&mut state, optional_vars);
                }
            }
            return walk_body(evaluator, state, &stmt_with.body);
        }
        ast::Stmt::Try(stmt_try) => return walk_try(evaluator, state, stmt_try),
        ast::Stmt::FunctionDef(function) => evaluator.bind_function(&mut state, function),
        ast::Stmt::ClassDef(class) => evaluator.bind_class(&mut state, class),
        ast::Stmt::Delete(delete) => {
            for target in &delete.targets {
                evaluator.bind_delete_target(&mut state, target);
            }
        }
        ast::Stmt::TypeAlias(type_alias) => evaluator.bind_type_alias(&mut state, type_alias),
        ast::Stmt::Match(stmt_match) => return walk_match(evaluator, state, stmt_match),
        ast::Stmt::Return(_)
        | ast::Stmt::Raise(_)
        | ast::Stmt::Assert(_)
        | ast::Stmt::Global(_)
        | ast::Stmt::Nonlocal(_)
        | ast::Stmt::Pass(_)
        | ast::Stmt::Break(_)
        | ast::Stmt::Continue(_)
        | ast::Stmt::IpyEscapeCommand(_) => {}
    }
    state
}

fn walk_if(
    evaluator: &mut SemanticEvaluator<'_>,
    state: EvaluationState,
    stmt_if: &ast::StmtIf,
) -> EvaluationState {
    match if_branches(
        &state,
        &stmt_if.test,
        &stmt_if.body,
        &stmt_if.elif_else_clauses,
        evaluate_test,
    ) {
        IfBranches::Deterministic(Some(body)) => walk_body(evaluator, state, body),
        IfBranches::Deterministic(None) => state,
        IfBranches::Ambiguous(arms) => {
            let paths: Vec<BranchPath<'_>> =
                arms.iter().map(|arm| BranchPath::Single(arm)).collect();
            evaluator.join_ambiguous_paths(state, &paths, stmt_if.span())
        }
    }
}

fn walk_try(
    evaluator: &mut SemanticEvaluator<'_>,
    state: EvaluationState,
    stmt_try: &ast::StmtTry,
) -> EvaluationState {
    if stmt_try.handlers.is_empty() {
        let state = walk_body(evaluator, state, &stmt_try.body);
        let state = walk_body(evaluator, state, &stmt_try.orelse);
        return walk_body(evaluator, state, &stmt_try.finalbody);
    }

    let state = evaluator.join_ambiguous_paths(state, &try_paths(stmt_try), stmt_try.span());
    walk_body(evaluator, state, &stmt_try.finalbody)
}

fn walk_match(
    evaluator: &mut SemanticEvaluator<'_>,
    mut state: EvaluationState,
    stmt_match: &ast::StmtMatch,
) -> EvaluationState {
    if stmt_match.cases.is_empty() {
        return state;
    }

    if stmt_match.cases.len() == 1 && is_irrefutable_match_case(&stmt_match.cases[0]) {
        evaluator.bind_pattern_names(&mut state, &stmt_match.cases[0].pattern);
        return walk_body(evaluator, state, &stmt_match.cases[0].body);
    }

    evaluator.join_match_cases(state, &stmt_match.cases, stmt_match.span())
}

fn evaluate_test(state: &EvaluationState, expr: &ast::Expr) -> Truthiness {
    evaluate_test_with(expr, |name| state.bool_value(name))
}

impl SemanticEvaluator<'_> {
    fn walk_assign(&mut self, state: &mut EvaluationState, assign: &ast::StmtAssign) {
        let value = expression::evaluate_binding(&self.context, state, &assign.value);
        for target in &assign.targets {
            assign_target(&self.context, state, target, &assign.value, value.clone());
        }
    }

    fn walk_ann_assign(&mut self, state: &mut EvaluationState, assign: &ast::StmtAnnAssign) {
        if let Some(value) = &assign.value {
            let evaluated = expression::evaluate_binding(&self.context, state, value);
            assign_target(&self.context, state, &assign.target, value, evaluated);
        }
    }

    fn walk_aug_assign(&mut self, state: &mut EvaluationState, assign: &ast::StmtAugAssign) {
        let origin = self.context.origin(assign);
        if assign.op == ast::Operator::Add
            && let Some(name) = assign.target.name_target()
            && let Some(left) = state.value_for_name(name)
        {
            let right = expression::evaluate_value(&self.context, state, &assign.value);
            state.assign_value(name, expression::add_values(left, right, origin), origin);
            let target = MutationTarget::from_expr(&assign.target)
                .expect("a name target is a supported mutation target");
            state
                .mutations
                .push(target.into_fact(PythonMutationOperation::Extend, origin));
            return;
        }

        if assign.op == ast::Operator::Add
            && let Some(target) = MutationTarget::from_expr(&assign.target)
        {
            let extension = expression::evaluate_value(&self.context, state, &assign.value);
            mutation::apply_augmented_add(state, target, &extension, origin);
            return;
        }

        bind_unknown_targets(
            &self.context,
            state,
            &assign.target,
            &PythonUnknownCause::UnsupportedMutation,
        );
    }

    fn walk_import(&mut self, state: &mut EvaluationState, import: &ast::StmtImport) {
        for alias in &import.names {
            let bound_name = alias.asname.as_ref().map_or_else(
                || first_import_segment(alias.name.as_str()),
                ast::Identifier::as_str,
            );
            state.bind_unknown(
                bound_name,
                &PythonUnknownCause::UnsupportedExpression,
                self.context.origin(alias),
            );
        }
    }

    fn bind_for_target(&mut self, state: &mut EvaluationState, target: &ast::Expr) {
        bind_unknown_targets(
            &self.context,
            state,
            target,
            &PythonUnknownCause::UnsupportedExpression,
        );
    }

    fn bind_function(&mut self, state: &mut EvaluationState, function: &ast::StmtFunctionDef) {
        state.bind_unknown(
            function.name.as_str(),
            &PythonUnknownCause::UnsupportedExpression,
            self.context.origin(function),
        );
    }

    fn bind_class(&mut self, state: &mut EvaluationState, class: &ast::StmtClassDef) {
        state.bind_unknown(
            class.name.as_str(),
            &PythonUnknownCause::UnsupportedExpression,
            self.context.origin(class),
        );
    }

    fn bind_delete_target(&mut self, state: &mut EvaluationState, target: &ast::Expr) {
        bind_unknown_targets(
            &self.context,
            state,
            target,
            &PythonUnknownCause::UnsupportedMutation,
        );
    }

    fn bind_type_alias(&mut self, state: &mut EvaluationState, alias: &ast::StmtTypeAlias) {
        bind_unknown_targets(
            &self.context,
            state,
            &alias.name,
            &PythonUnknownCause::UnsupportedExpression,
        );
    }

    fn bind_pattern_names(&mut self, state: &mut EvaluationState, pattern: &ast::Pattern) {
        for name in pattern_bound_names(pattern) {
            state.bind_unknown(
                name,
                &PythonUnknownCause::UnsupportedExpression,
                self.context.origin(pattern),
            );
        }
    }

    fn degrade_loop_bodies(
        &mut self,
        mut state: EvaluationState,
        bodies: &[&[ast::Stmt]],
        control_span: Span,
    ) -> EvaluationState {
        let base = state.clone();
        let mut writes = TouchedNames::default();
        for body in bodies {
            let body_state = walk_body(self, base.clone(), body);
            writes.merge(EvaluationState::changed_writes_from(&base, &body_state));
            extend_ordered_unique(
                &mut state.dependencies.files,
                &body_state.dependencies.files,
            );
            extend_ordered_unique(
                &mut state.dependencies.imports,
                &body_state.dependencies.imports,
            );
            extend_ordered_unique(&mut state.mutations, &body_state.mutations);
            state.namespace_causes.extend(body_state.namespace_causes);
        }
        state.degrade_names(
            writes.names,
            &PythonUnknownCause::UnsupportedExpression,
            self.context.origin_at(control_span),
        );
        state
    }

    fn join_ambiguous_paths(
        &mut self,
        state: EvaluationState,
        paths: &[BranchPath<'_>],
        control_span: Span,
    ) -> EvaluationState {
        let mut writes = TouchedNames::default();
        for path in paths {
            for segment in path.segments() {
                writes.merge(collect_touched_names(segment));
            }
        }
        let base = state;
        let mut branches = Vec::with_capacity(paths.len());
        for path in paths {
            let mut branch = base.clone();
            for segment in path.segments() {
                branch = walk_body(self, branch, segment);
            }
            branches.push(branch);
        }
        EvaluationState::join_branches(
            base,
            &branches,
            &writes,
            self.context.origin_at(control_span),
        )
    }

    fn join_match_cases(
        &mut self,
        state: EvaluationState,
        cases: &[ast::MatchCase],
        control_span: Span,
    ) -> EvaluationState {
        let mut writes = TouchedNames::default();
        for case in cases {
            for name in pattern_bound_names(&case.pattern) {
                writes.record(name);
            }
            writes.merge(collect_touched_names(&case.body));
        }
        let base = state;
        let mut branches = Vec::with_capacity(cases.len() + 1);
        for case in cases {
            let mut branch = base.clone();
            self.bind_pattern_names(&mut branch, &case.pattern);
            branches.push(walk_body(self, branch, &case.body));
        }
        if !cases.iter().any(is_irrefutable_match_case) {
            branches.push(base.clone());
        }
        EvaluationState::join_branches(
            base,
            &branches,
            &writes,
            self.context.origin_at(control_span),
        )
    }
}

fn assign_target(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    target: &ast::Expr,
    expression: &ast::Expr,
    value: PythonBinding,
) {
    if let Some(name) = target.name_target() {
        let origin = context.origin(expression);
        if let Some(source_name) = expression.name_target()
            && state.assign_from_name(name, source_name, origin)
        {
            return;
        }
        state.assign_binding(name, value, origin);
    } else {
        bind_unknown_targets(
            context,
            state,
            target,
            &PythonUnknownCause::UnsupportedExpression,
        );
    }
}

fn bind_unknown_targets(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    target: &ast::Expr,
    cause: &PythonUnknownCause,
) {
    let origin = context.origin(target);
    for name in target_write_names(target) {
        state.bind_unknown(name, cause, origin);
    }
}
