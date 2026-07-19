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
use super::Evaluator;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::import::DirectImportClause;

impl Evaluator<'_> {
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
                    let evaluated = self.evaluate_binding(value);
                    self.assign_target(&assign.target, value, evaluated);
                }
            }
            ast::Stmt::AugAssign(assign) => self.walk_aug_assign(assign),
            ast::Stmt::Expr(expr) => self.evaluate_expression_statement(&expr.value),
            ast::Stmt::Import(import) => self.walk_import(import),
            ast::Stmt::ImportFrom(import) => self.evaluate_import_from(import),
            ast::Stmt::If(stmt_if) => self.walk_if(stmt_if),
            ast::Stmt::For(stmt_for) => {
                self.bind_for_target(&stmt_for.target);
                self.degrade_loop_bodies(&[&stmt_for.body, &stmt_for.orelse], stmt_for.span());
            }
            ast::Stmt::While(stmt_while) => match self.test_truthiness(&stmt_while.test) {
                Truthiness::AlwaysFalse => self.evaluate_body(&stmt_while.orelse),
                Truthiness::AlwaysTrue | Truthiness::Ambiguous => self.degrade_loop_bodies(
                    &[&stmt_while.body, &stmt_while.orelse],
                    stmt_while.span(),
                ),
            },
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
        match self.test_truthiness(&stmt_if.test) {
            Truthiness::AlwaysTrue => self.evaluate_body(&stmt_if.body),
            Truthiness::AlwaysFalse => {
                let clauses = &stmt_if.elif_else_clauses;
                let control_span = stmt_if.span();
                for (index, clause) in clauses.iter().enumerate() {
                    let Some(test) = &clause.test else {
                        self.evaluate_body(&clause.body);
                        return;
                    };

                    match self.test_truthiness(test) {
                        Truthiness::AlwaysTrue => {
                            self.evaluate_body(&clause.body);
                            return;
                        }
                        Truthiness::AlwaysFalse => {}
                        Truthiness::Ambiguous => {
                            self.join_reachable_if_bodies(None, &clauses[index..], control_span);
                            return;
                        }
                    }
                }
            }
            Truthiness::Ambiguous => self.join_reachable_if_bodies(
                Some(&stmt_if.body),
                &stmt_if.elif_else_clauses,
                stmt_if.span(),
            ),
        }
    }

    fn join_reachable_if_bodies(
        &mut self,
        first_body: Option<&[ast::Stmt]>,
        clauses: &[ast::ElifElseClause],
        control_span: Span,
    ) {
        let mut bodies = Vec::with_capacity(clauses.len() + 2);
        if let Some(body) = first_body {
            bodies.push(body);
        }

        let mut has_fallthrough = true;
        for clause in clauses {
            let Some(test) = &clause.test else {
                bodies.push(clause.body.as_slice());
                has_fallthrough = false;
                break;
            };

            match self.test_truthiness(test) {
                Truthiness::AlwaysTrue => {
                    bodies.push(clause.body.as_slice());
                    has_fallthrough = false;
                    break;
                }
                Truthiness::AlwaysFalse => {}
                Truthiness::Ambiguous => bodies.push(clause.body.as_slice()),
            }
        }
        if has_fallthrough {
            bodies.push(&[]);
        }

        let mut branches = Vec::with_capacity(bodies.len());
        for body in bodies {
            let mut branch = self.fork();
            branch.evaluate_body(body);
            branches.push(branch);
        }
        self.join_forks(branches, self.origin_at(control_span));
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
        let value = self.evaluate_binding(&assign.value);
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
        for target in &assign.targets {
            self.assign_target(target, &assign.value, value.clone());
        }
    }

    fn walk_aug_assign(&mut self, assign: &ast::StmtAugAssign) {
        let origin = self.origin(assign);
        if assign.op == ast::Operator::Add
            && let Some(name) = assign.target.name_target()
            && let Some(left) = self.state.value_for_name(name)
        {
            let right = self.evaluate_value(&assign.value);
            self.apply_name_augmented_add(name, left, &right, &assign.target, origin);
            return;
        }

        if assign.op == ast::Operator::Add
            && let Some(target) = MutationTarget::from_expr(&assign.target)
        {
            let extension = self.evaluate_value(&assign.value);
            self.state.apply_augmented_add(target, &extension, origin);
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
        name: &str,
        left: PythonValue,
        right: &PythonValue,
        target: &ast::Expr,
        origin: Origin,
    ) {
        let extend_fact = || {
            MutationTarget::from_expr(target)
                .expect("a name target is a supported mutation target")
                .into_fact(PythonMutationOperation::Extend, origin)
        };
        let mut left = left;
        match &mut left.kind {
            PythonValueKind::List(list) => {
                let mut stale_aliases = self
                    .state
                    .stale_alias_names_after_mutation(name, &PythonMutationPath::default());
                let extended = list.extend_from(right, origin);
                if extended.is_some() {
                    left.record_origin(origin);
                    // A successful in-place list `+=` updates the binding
                    // without clearing prior mutation facts; the `Extend` fact
                    // below then accumulates onto them.
                    self.state.update_bound_value(name, left);
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
                self.state.mutations.insert(extend_fact());
            }
            PythonValueKind::Tuple(_) | PythonValueKind::Str(_) => {
                let value = left.add(right, origin);
                self.state.assign_value(name, value, origin);
            }
            PythonValueKind::Dict(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Unknown(_) => {
                let stale_aliases = self
                    .state
                    .stale_alias_names_after_mutation(name, &PythonMutationPath::default());
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
                self.state.mutations.insert(extend_fact());
            }
        }
    }

    fn walk_import(&mut self, import: &ast::StmtImport) {
        for clause in DirectImportClause::lower(import) {
            self.state.bind_unknown(
                clause.bound(),
                &PythonUnknownCause::UnsupportedExpression,
                self.origin_at(clause.binding_span()),
            );
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

    fn assign_target(&mut self, target: &ast::Expr, expression: &ast::Expr, value: PythonBinding) {
        if let Some(name) = target.name_target() {
            let origin = self.origin(expression);
            if let Some(source_name) = expression.name_target()
                && self.state.assign_from_name(name, source_name, origin)
            {
                return;
            }
            self.state.assign_binding(name, value, origin);
        } else {
            self.bind_unknown_targets(target, &PythonUnknownCause::UnsupportedExpression);
        }
    }

    fn bind_unknown_targets(&mut self, target: &ast::Expr, cause: &PythonUnknownCause) {
        let origin = self.origin(target);
        for name in target_write_names(target) {
            self.state.bind_unknown(name, cause, origin);
        }
    }

    fn test_truthiness(&self, expression: &ast::Expr) -> Truthiness {
        Truthiness::of_expr(expression, &|name| self.state.bool_value(name))
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
