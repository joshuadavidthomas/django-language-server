use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;

use super::super::PythonBinding;
use super::super::PythonMutationOperation;
use super::super::PythonMutationPath;
use super::super::PythonUnknownCause;
use super::super::PythonValue;
use super::super::PythonValueKind;
use super::super::mutation::MutationTarget;
use super::super::name_analysis::pattern_bound_names;
use super::super::name_analysis::target_write_names;
use super::super::truthiness::Truthiness;
use super::PythonModuleEvaluator;
use crate::ast::ExprExt;
use crate::ast::RangedExt;

impl<'db> PythonModuleEvaluator<'db> {
    pub(super) fn evaluate_body(&mut self, body: &[ast::Stmt]) {
        for stmt in body {
            self.walk_stmt(stmt);
        }
    }

    fn walk_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::Assign(assign) => self.walk_assign(assign),
            ast::Stmt::AnnAssign(assign) => {
                if let Some(value) = &assign.value {
                    self.record_unsupported_call_effects(value);
                    let evaluated = self.evaluate_binding(value);
                    self.assign_target(&assign.target, value, evaluated, self.origin(assign));
                }
            }
            ast::Stmt::AugAssign(assign) => self.walk_aug_assign(assign),
            ast::Stmt::Expr(expr) => self.evaluate_expression_statement(&expr.value),
            ast::Stmt::Import(statement) => self.evaluate_direct_import_statement(statement),
            ast::Stmt::ImportFrom(statement) => self.evaluate_from_import(statement),
            ast::Stmt::If(stmt_if) => self.walk_if(stmt_if),
            ast::Stmt::For(stmt_for) => {
                self.bind_for_target(&stmt_for.target);
                self.degrade_loop_bodies(&[&stmt_for.body, &stmt_for.orelse], stmt_for.span());
            }
            ast::Stmt::While(stmt_while) => {
                self.record_unsupported_call_effects(&stmt_while.test);
                match self.test_truthiness(&stmt_while.test) {
                    Some(Truthiness::Falsy) => self.evaluate_body(&stmt_while.orelse),
                    Some(Truthiness::Truthy) | None => self.degrade_loop_bodies(
                        &[&stmt_while.body, &stmt_while.orelse],
                        stmt_while.span(),
                    ),
                }
            }
            ast::Stmt::With(stmt_with) => {
                for item in &stmt_with.items {
                    if let Some(optional_vars) = &item.optional_vars {
                        self.bind_for_target(optional_vars);
                    }
                }
                self.evaluate_body(&stmt_with.body);
            }
            ast::Stmt::Try(stmt_try) => self.walk_try(stmt_try),
            ast::Stmt::FunctionDef(function) => self.state.bind_unknown(
                function.name.as_str(),
                &PythonUnknownCause::UnsupportedExpression,
                self.origin(function),
            ),
            ast::Stmt::ClassDef(class) => self.state.bind_unknown(
                class.name.as_str(),
                &PythonUnknownCause::UnsupportedExpression,
                self.origin(class),
            ),
            ast::Stmt::Delete(delete) => {
                for target in &delete.targets {
                    self.bind_unknown_targets(target, &PythonUnknownCause::UnsupportedMutation);
                }
            }
            ast::Stmt::TypeAlias(type_alias) => self
                .bind_unknown_targets(&type_alias.name, &PythonUnknownCause::UnsupportedExpression),
            ast::Stmt::Match(stmt_match) => self.walk_match(stmt_match),
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
    }

    fn walk_if(&mut self, stmt_if: &ast::StmtIf) {
        let clauses = &stmt_if.elif_else_clauses;
        let arm_count = 1
            + clauses.len()
            + usize::from(clauses.last().is_none_or(|clause| clause.test.is_some()));
        let mut remaining = self.active_constraints.clone();
        let mut branches = Vec::with_capacity(arm_count);

        self.add_guarded_if_body(
            0,
            &stmt_if.test,
            &stmt_if.body,
            &mut remaining,
            &mut branches,
        );
        for (index, clause) in clauses.iter().enumerate() {
            if remaining.is_impossible() {
                break;
            }
            let arm = index + 1;
            if let Some(test) = &clause.test {
                self.add_guarded_if_body(arm, test, &clause.body, &mut remaining, &mut branches);
            } else {
                let mut branch = self.fork();
                branch.active_constraints = remaining.clone();
                branch.evaluate_body(&clause.body);
                branches.push((arm, branch));
                remaining = super::BranchConstraints::impossible();
                break;
            }
        }

        if !remaining.is_impossible() {
            let mut fallthrough = self.fork();
            fallthrough.active_constraints = remaining;
            branches.push((arm_count - 1, fallthrough));
        }
        self.join_guarded_forks(branches, self.origin(stmt_if), arm_count);
    }

    fn add_guarded_if_body(
        &mut self,
        arm: usize,
        test: &ast::Expr,
        body: &[ast::Stmt],
        remaining: &mut super::BranchConstraints,
        branches: &mut Vec<(usize, PythonModuleEvaluator<'db>)>,
    ) {
        self.record_unsupported_call_effects(test);
        let binding = self.evaluate_binding(test);
        let partition = self.truth_partition(&binding, self.origin(test), false);
        let body_constraints = remaining.intersection(&partition.truthy);
        if !body_constraints.is_impossible() {
            let mut branch = self.fork();
            branch.active_constraints = body_constraints;
            branch.evaluate_body(body);
            branches.push((arm, branch));
        }
        *remaining = remaining.intersection(&partition.falsy);
    }

    fn walk_try(&mut self, stmt_try: &ast::StmtTry) {
        if stmt_try.handlers.is_empty() {
            self.evaluate_body(&stmt_try.body);
            self.evaluate_body(&stmt_try.orelse);
            self.evaluate_body(&stmt_try.finalbody);
            return;
        }

        let mut branches =
            Vec::with_capacity(1 + stmt_try.handlers.len() * stmt_try.body.len().max(1));
        let mut success = self.fork();
        success.evaluate_body(&stmt_try.body);
        success.evaluate_body(&stmt_try.orelse);
        branches.push(success);

        for handler in &stmt_try.handlers {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            for prefix_len in 0..stmt_try.body.len().max(1) {
                let mut branch = self.fork();
                branch.evaluate_body(&stmt_try.body[..prefix_len]);
                branch.evaluate_body(&handler.body);
                branches.push(branch);
            }
        }

        self.join_forks(branches, self.origin(stmt_try));
        self.evaluate_body(&stmt_try.finalbody);
    }

    fn walk_match(&mut self, stmt_match: &ast::StmtMatch) {
        if stmt_match.cases.is_empty() {
            return;
        }

        if stmt_match.cases.len() == 1 && is_irrefutable_match_case(&stmt_match.cases[0]) {
            self.bind_pattern_names(&stmt_match.cases[0].pattern);
            self.evaluate_body(&stmt_match.cases[0].body);
            return;
        }

        let mut branches = Vec::with_capacity(stmt_match.cases.len() + 1);
        for case in &stmt_match.cases {
            let mut branch = self.fork();
            branch.bind_pattern_names(&case.pattern);
            branch.evaluate_body(&case.body);
            branches.push(branch);
        }
        if !stmt_match.cases.iter().any(is_irrefutable_match_case) {
            branches.push(self.fork());
        }
        self.join_forks(branches, self.origin(stmt_match));
    }

    fn walk_assign(&mut self, assign: &ast::StmtAssign) {
        self.record_unsupported_call_effects(&assign.value);
        let mut value = self.evaluate_binding(&assign.value);
        let aliases_mutable_value =
            assign.targets.len() > 1 && !value.reachable_allocation_sites().is_empty();
        if aliases_mutable_value {
            let cause = PythonUnknownCause::UnsupportedExpression;
            let aliased_names = self.state.mutable_alias_names(&value);
            for name in aliased_names {
                self.state
                    .bind_unknown(&name, &cause, self.origin(assign.value.as_ref()));
            }
            for target in &assign.targets {
                self.bind_unknown_targets(target, &cause);
            }
            return;
        }
        let assignment_origin = self.origin(assign);
        if assign.targets.len() > 1
            && assign
                .targets
                .iter()
                .all(|target| target.name_target().is_some())
            && let Some(shared_name) = assign.targets[0].name_target()
        {
            value.discriminate_predicates(
                shared_name,
                self.origin(assign.value.as_ref()),
                Some(&self.module),
            );
        }
        for target in &assign.targets {
            self.assign_target(target, &assign.value, value.clone(), assignment_origin);
        }
    }

    fn walk_aug_assign(&mut self, assign: &ast::StmtAugAssign) {
        let origin = self.origin(assign);
        if assign.op == ast::Operator::Add
            && let Some(target) = MutationTarget::from_expr(&assign.target)
            && assign.target.name_target().is_some()
            && let Some(left) = self
                .state
                .value_for_name(target.binding, &self.active_constraints)
        {
            let right = self.evaluate_value(&assign.value);
            self.apply_name_augmented_add(target, left, &right, origin);
            return;
        }

        if assign.op == ast::Operator::Add
            && let Some(target) = MutationTarget::from_expr(&assign.target)
        {
            let extension = self.evaluate_value(&assign.value);
            self.state
                .apply_augmented_add(target, &extension, origin, &self.active_constraints);
            return;
        }

        self.bind_unknown_targets(&assign.target, &PythonUnknownCause::UnsupportedMutation);
    }

    /// Apply `name += rhs` where `name` is already bound to `left`. The receiver
    /// kind selects one of three contracts:
    ///
    /// - a list is mutated in place by consuming the RHS as an iterable,
    ///   preserving its allocation sites and recording an `Extend` fact; a
    ///   non-iterable RHS (bool) fails the mutation, replacing the target with
    ///   an unsupported-expression unknown and degrading stale aliases;
    /// - a tuple or string performs nominal addition and rebinds the name to the
    ///   new immutable value with no mutation fact and no alias effects;
    /// - any other receiver is an unsupported in-place add: the target becomes
    ///   an unsupported-expression unknown, stale aliases become
    ///   unsupported-mutation unknowns, and an `Extend` fact is recorded.
    fn apply_name_augmented_add(
        &mut self,
        target: MutationTarget<'_>,
        left: PythonValue,
        right: &PythonValue,
        origin: Origin,
    ) {
        let name = target.binding;
        let mut left = left;
        match &mut left.kind {
            PythonValueKind::List(list) => {
                let mut stale_aliases = self.state.stale_alias_names_after_mutation(
                    name,
                    &PythonMutationPath::default(),
                    &self.active_constraints,
                );
                let embedded_sites = right.iterated_reachable_allocation_sites();
                let recursive = self.state.sites_reach_mutation_target(
                    &target,
                    &embedded_sites,
                    &self.active_constraints,
                );
                let extended = if recursive {
                    None
                } else {
                    list.extend_from(right, origin)
                };
                if extended.is_some() {
                    left.record_origin(origin);
                    // A successful in-place list `+=` updates the binding
                    // without clearing prior mutation facts; the `Extend` fact
                    // below then accumulates onto them.
                    self.state
                        .update_bound_value(name, left, origin, &self.active_constraints);
                    self.state.invalidate_names(
                        stale_aliases,
                        &PythonUnknownCause::UnsupportedExpression,
                        origin,
                    );
                } else {
                    // The direct target's failed-operation state wins when the
                    // target also appears in its own reachable alias graph.
                    // Other stale aliases still receive the mutation-specific
                    // failure cause.
                    stale_aliases.retain(|alias| alias != name);
                    self.state.assign_value(
                        name,
                        PythonValue::unknown(
                            PythonUnknownCause::UnsupportedExpression,
                            Some(origin),
                        ),
                        origin,
                    );
                    self.state.invalidate_names(
                        stale_aliases,
                        &PythonUnknownCause::UnsupportedMutation,
                        origin,
                    );
                }
                self.state
                    .mutations
                    .insert(target.into_fact(PythonMutationOperation::Extend, origin));
            }
            PythonValueKind::Tuple(_) | PythonValueKind::Str(_) => {
                let value = left.add(right, origin);
                self.state.assign_value(name, value, origin);
            }
            PythonValueKind::Dict(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Bool(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => {
                let stale_aliases = self.state.stale_alias_names_after_mutation(
                    name,
                    &PythonMutationPath::default(),
                    &self.active_constraints,
                );
                self.state.assign_value(
                    name,
                    PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
                    origin,
                );
                self.state.invalidate_names(
                    stale_aliases,
                    &PythonUnknownCause::UnsupportedMutation,
                    origin,
                );
                self.state
                    .mutations
                    .insert(target.into_fact(PythonMutationOperation::Extend, origin));
            }
        }
    }

    fn bind_for_target(&mut self, target: &ast::Expr) {
        self.bind_unknown_targets(target, &PythonUnknownCause::UnsupportedExpression);
    }

    fn bind_pattern_names(&mut self, pattern: &ast::Pattern) {
        for name in pattern_bound_names(pattern) {
            self.state.bind_unknown(
                name,
                &PythonUnknownCause::UnsupportedExpression,
                self.origin(pattern),
            );
        }
    }

    fn degrade_loop_bodies(&mut self, bodies: &[&[ast::Stmt]], control_span: Span) {
        let baseline = self.fork();
        let mut evaluated_bodies = Vec::with_capacity(bodies.len());
        for body in bodies {
            let mut evaluator = baseline.fork();
            evaluator.evaluate_body(body);
            evaluated_bodies.push(evaluator.state);
        }

        self.state = baseline
            .state
            .degrade_loop_effects(evaluated_bodies, self.origin_at(control_span));
    }

    fn assign_target(
        &mut self,
        target: &ast::Expr,
        expression: &ast::Expr,
        value: PythonBinding,
        assignment_origin: Origin,
    ) {
        let origin = self.origin(expression);
        if let Some(name) = target.name_target() {
            if let Some(source_name) = expression.name_target() {
                self.state
                    .assign_from_name(name, source_name, value, origin);
            } else {
                self.state.assign_binding(name, value, origin);
            }
            return;
        }

        if let ast::Expr::Subscript(subscript) = target
            && let Some(key) = subscript.slice.string_literal()
            && let Some(receiver) = MutationTarget::from_expr(&subscript.value)
        {
            self.state.assign_string_key(
                &receiver,
                key,
                self.origin(subscript.slice.as_ref()),
                &value,
                assignment_origin,
                &self.active_constraints,
            );
            return;
        }

        self.bind_unknown_targets(target, &PythonUnknownCause::UnsupportedExpression);
    }

    fn bind_unknown_targets(&mut self, target: &ast::Expr, cause: &PythonUnknownCause) {
        let origin = self.origin(target);
        let mut names = Vec::new();
        let target_names = target_write_names(target);
        if target_names.is_empty() {
            names = self.state.all_path_intrinsic_write_names();
        } else {
            for target_name in target_names {
                for alias in self.state.path_intrinsic_write_names(target_name) {
                    if !names.contains(&alias) {
                        names.push(alias);
                    }
                }
            }
        }
        for name in names {
            self.state.bind_unknown(&name, cause, origin);
        }
    }

    fn test_truthiness(&self, expression: &ast::Expr) -> Option<Truthiness> {
        Truthiness::of_expr(expression, &|name| self.state.known_truthiness(name))
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
