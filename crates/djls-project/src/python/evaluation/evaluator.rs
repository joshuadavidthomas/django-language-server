mod expression;
mod imports;

use djls_source::File;
use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;

use super::BranchConstraints;
use super::PythonBinding;
use super::PythonBindingAlternative;
use super::PythonBindings;
use super::PythonBoundValue;
use super::PythonDict;
use super::PythonDictItem;
use super::PythonImportOutcome;
use super::PythonList;
use super::PythonListItem;
use super::PythonModuleDependencies;
use super::PythonModuleEvaluation;
use super::PythonModuleValues;
use super::PythonModuleValuesOutcome;
use super::PythonMutation;
use super::PythonNamespaceCause;
use super::PythonNamespaceRemainder;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::control_flow::BranchPath;
use super::control_flow::Truthiness;
use super::control_flow::evaluate_test_with;
use super::control_flow::is_irrefutable_match_case;
use super::mutation::MutationTarget;
use super::statement_walk::StatementInterpreter;
use super::statement_walk::{
    self,
};
use super::touched_names::TouchedNames;
use super::touched_names::collect_touched_names;
use super::touched_names::first_import_segment;
use super::touched_names::pattern_bound_names;
use super::touched_names::target_write_names;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonImportRequest;
use crate::python::PythonModule;
use crate::python::PythonPathBindings;
use crate::python::evaluate_path;

pub(super) fn evaluate_body(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
    body: &[ast::Stmt],
) -> PythonModuleEvaluation {
    let context = EvaluationContext { db, project, file };
    let state = EvaluationState {
        dependencies: PythonModuleDependencies {
            files: vec![file],
            imports: Vec::new(),
        },
        ..EvaluationState::default()
    };
    let mut evaluator = SemanticEvaluator { context };
    let state = statement_walk::walk_body(&mut evaluator, state, body);
    PythonModuleEvaluation::evaluated(
        PythonModuleValuesOutcome::Readable(PythonModuleValues {
            bindings: state.bindings,
            namespace_remainder: (!state.namespace_causes.is_empty())
                .then(|| PythonNamespaceRemainder::new(state.namespace_causes)),
            syntax_errors: Vec::new(),
            syntax_impacts: Vec::new(),
            mutations: state.mutations,
        }),
        state.dependencies,
    )
}

pub(super) struct EvaluationContext<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    file: File,
}

impl EvaluationContext<'_> {
    pub(super) fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.file, ranged.span())
    }

    fn origin_at(&self, span: Span) -> Origin {
        Origin::new(self.file, span)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct EvaluationState {
    pub(super) bindings: PythonBindings,
    namespace_causes: Vec<PythonNamespaceCause>,
    pub(super) mutations: Vec<PythonMutation>,
    dependencies: PythonModuleDependencies,
}

impl EvaluationState {
    fn binding(&self, name: &str) -> Option<&PythonBinding> {
        self.bindings.get(name)
    }

    fn assign_value(&mut self, name: &str, value: PythonValue, origin: Origin) {
        self.assign_binding(
            name,
            PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
                value,
                binding_origins: vec![origin],
            })]),
            origin,
        );
    }

    fn assign_binding(&mut self, name: &str, mut binding: PythonBinding, origin: Origin) {
        self.mutations.retain(|mutation| mutation.root != name);
        for alternative in binding.alternatives_mut() {
            if let PythonBindingAlternative::Bound(bound) = alternative {
                bound.binding_origins = vec![origin];
            }
        }
        self.bindings.insert(name.to_string(), binding);
    }

    fn assign_from_name(&mut self, name: &str, source: &str, origin: Origin) -> bool {
        let Some(mut binding) = self.binding(source).cloned() else {
            return false;
        };
        for alternative in binding.alternatives_mut() {
            if let PythonBindingAlternative::Bound(bound) = alternative {
                bound.binding_origins = vec![origin];
            }
        }
        self.bindings.insert(name.to_string(), binding);
        let copied = self
            .mutations
            .iter()
            .filter(|mutation| mutation.root == source)
            .cloned()
            .map(|mut mutation| {
                mutation.root = name.to_string();
                mutation
            })
            .collect::<Vec<_>>();
        self.mutations.retain(|mutation| mutation.root != name);
        self.mutations.extend(copied);
        true
    }

    fn bind_unknown(&mut self, name: &str, cause: PythonUnknownCause, origin: Origin) {
        self.assign_value(name, PythonValue::unknown(cause, Some(origin)), origin);
    }

    fn value_for_name(&self, name: &str) -> Option<PythonValue> {
        let binding = self.binding(name)?;
        Some(binding.single_bound()?.value.clone())
    }

    fn bool_value(&self, name: &str) -> Option<bool> {
        let binding = self.binding(name)?;
        let mut values = binding.alternatives();
        let PythonBindingAlternative::Bound(first) = values.next()? else {
            return None;
        };
        let PythonValueKind::Bool(value) = first.value.kind else {
            return None;
        };
        values
            .all(|alternative| {
                matches!(alternative, PythonBindingAlternative::Bound(bound)
                    if matches!(bound.value.kind, PythonValueKind::Bool(other) if other == value))
            })
            .then_some(value)
    }

    fn path_bindings(&self) -> PythonPathBindings {
        let mut paths = PythonPathBindings::default();
        for (name, binding) in &self.bindings.0 {
            let Some(bound) = binding.single_bound() else {
                continue;
            };
            if let PythonValueKind::Path(path) = &bound.value.kind {
                paths.set(name.clone(), path.clone());
            }
        }
        paths
    }

    #[allow(clippy::needless_pass_by_value)]
    fn degrade_all_bindings(
        &mut self,
        cause: PythonUnknownCause,
        origin: Origin,
        constraints: &BranchConstraints,
    ) {
        for binding in self.bindings.0.values_mut() {
            let unknown = imports::constrained_unknown_binding(cause.clone(), origin, constraints)
                .expect("a namespace cause must have a feasible branch");
            *binding = binding.clone().join(unknown, origin);
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn invalidate_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        cause: PythonUnknownCause,
        origin: Origin,
    ) {
        for name in names {
            self.bind_unknown(&name, cause.clone(), origin);
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn degrade_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        cause: PythonUnknownCause,
        origin: Origin,
    ) {
        for name in names {
            let unknown =
                PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
                    value: PythonValue::unknown(cause.clone(), Some(origin)),
                    binding_origins: vec![origin],
                })]);
            let binding = self
                .bindings
                .0
                .remove(&name)
                .map_or(unknown.clone(), |binding| binding.join(unknown, origin));
            self.bindings.insert(name, binding);
        }
    }

    fn apply_star_import(&mut self, values: &PythonModuleValues, import_origin: Origin) {
        if let Some(remainder) = &values.namespace_remainder {
            for cause in &remainder.causes {
                self.degrade_all_bindings(
                    cause.unknown.cause.clone(),
                    import_origin,
                    &cause.constraints,
                );
            }
        }
        for (name, binding) in &values.bindings.0 {
            let prior = self.bindings.0.get(name).cloned();
            let mut binding = binding.clone();
            rebase_cycle_unknowns(&mut binding, import_origin);
            self.bindings.insert(
                name.clone(),
                binding.replace_unbound_with(prior, import_origin),
            );
        }
        let mut namespace_errors = Vec::new();
        for impact in &values.syntax_impacts {
            let affected = self
                .bindings
                .0
                .keys()
                .filter(|name| impact.affects(name))
                .cloned()
                .collect::<Vec<_>>();
            if !affected.is_empty() {
                self.degrade_names(
                    affected,
                    PythonUnknownCause::SyntaxErrors(vec![impact.error.clone()]),
                    import_origin,
                );
            }
            if impact.namespace_open {
                namespace_errors.push(impact.error.clone());
            }
        }
        if !namespace_errors.is_empty() {
            self.namespace_causes
                .push(PythonNamespaceCause::unconstrained(PythonUnknown {
                    cause: PythonUnknownCause::SyntaxErrors(namespace_errors),
                    origin: Some(import_origin),
                }));
        }
        extend_unique(&mut self.mutations, &values.mutations);
        if let Some(remainder) = &values.namespace_remainder {
            self.namespace_causes
                .extend(remainder.causes.iter().cloned().map(|mut cause| {
                    cause.unknown.origin = Some(import_origin);
                    cause
                }));
        }
    }

    fn bind_named_import(
        &mut self,
        values: &PythonModuleValues,
        imported_name: &str,
        bound_name: &str,
        origin: Origin,
    ) {
        let syntax_errors = values
            .syntax_impacts
            .iter()
            .filter(|impact| impact.affects(imported_name))
            .map(|impact| impact.error.clone())
            .collect::<Vec<_>>();
        let mut binding = values
            .bindings
            .get(imported_name)
            .cloned()
            .unwrap_or_else(|| PythonBinding::new(vec![PythonBindingAlternative::Unbound]));
        for alternative in binding.alternatives_mut() {
            if let PythonBindingAlternative::Bound(bound) = alternative {
                bound.binding_origins = vec![origin];
            }
        }
        rebase_cycle_unknowns(&mut binding, origin);

        let unbound_constraints = binding
            .alternatives_with_constraints()
            .filter_map(|(alternative, constraints)| {
                (*alternative == PythonBindingAlternative::Unbound).then_some(constraints.clone())
            })
            .collect::<Vec<_>>();
        if let Some(remainder) = &values.namespace_remainder {
            for unbound in &unbound_constraints {
                for cause in &remainder.causes {
                    let constraints = unbound.intersection(&cause.constraints);
                    if let Some(unknown) = imports::constrained_unknown_binding(
                        cause.unknown.cause.clone(),
                        origin,
                        &constraints,
                    ) {
                        binding = binding.join(unknown, origin);
                    }
                }
            }
        }
        if !syntax_errors.is_empty() {
            let unknown =
                PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
                    value: PythonValue::unknown(
                        PythonUnknownCause::SyntaxErrors(syntax_errors),
                        Some(origin),
                    ),
                    binding_origins: vec![origin],
                })]);
            binding = binding.join(unknown, origin);
        }
        self.bindings.insert(bound_name.to_string(), binding);
        let copied = values
            .mutations
            .iter()
            .filter(|mutation| mutation.root == imported_name)
            .cloned()
            .map(|mut mutation| {
                mutation.root = bound_name.to_string();
                mutation
            })
            .collect::<Vec<_>>();
        extend_unique(&mut self.mutations, &copied);
    }

    fn join_branches(
        mut base: Self,
        branches: &[Self],
        writes: &TouchedNames,
        origin: Origin,
    ) -> Self {
        let mut names = writes.names.clone();
        for branch in branches {
            for (name, binding) in &branch.bindings.0 {
                if base.binding(name) != Some(binding) {
                    names.insert(name.clone());
                }
            }
        }
        for name in names {
            let mut joined = None;
            for (arm, branch) in branches.iter().enumerate() {
                let mut candidate = branch
                    .binding(&name)
                    .cloned()
                    .unwrap_or_else(|| PythonBinding::new(vec![PythonBindingAlternative::Unbound]));
                candidate.select_branch(origin, arm);
                joined = Some(joined.map_or(candidate.clone(), |current: PythonBinding| {
                    current.join(candidate, origin)
                }));
            }
            if let Some(binding) = joined {
                base.bindings.insert(name, binding);
            }
        }
        base.namespace_causes.clear();
        base.mutations.clear();
        base.dependencies = PythonModuleDependencies::default();
        for (arm, branch) in branches.iter().enumerate() {
            base.namespace_causes
                .extend(branch.namespace_causes.iter().cloned().map(|mut cause| {
                    cause.select_branch(origin, arm);
                    cause
                }));
            extend_unique(&mut base.mutations, &branch.mutations);
            extend_unique(&mut base.dependencies.files, &branch.dependencies.files);
            extend_unique(&mut base.dependencies.imports, &branch.dependencies.imports);
        }
        base
    }

    fn changed_writes_from(base: &Self, changed: &Self) -> TouchedNames {
        let mut writes = TouchedNames::default();
        for (name, binding) in &changed.bindings.0 {
            if base.binding(name) != Some(binding) {
                writes.record(name);
            }
        }
        for name in base.bindings.0.keys() {
            if changed.binding(name).is_none() {
                writes.record(name);
            }
        }
        writes
    }

    fn record_import(&mut self, outcome: PythonImportOutcome) {
        if !self.dependencies.imports.contains(&outcome) {
            self.dependencies.imports.push(outcome);
        }
    }

    fn absorb_dependencies(&mut self, evaluation: &PythonModuleEvaluation) {
        extend_unique(&mut self.dependencies.files, &evaluation.dependencies.files);
        extend_unique(
            &mut self.dependencies.imports,
            &evaluation.dependencies.imports,
        );
    }
}

fn rebase_cycle_unknowns(binding: &mut PythonBinding, origin: Origin) {
    for alternative in binding.alternatives_mut() {
        let PythonBindingAlternative::Bound(bound) = alternative else {
            continue;
        };
        let PythonValueKind::Unknown(unknown) = &mut bound.value.kind else {
            continue;
        };
        if unknown.cause == PythonUnknownCause::Cycle {
            unknown.origin = Some(origin);
            bound.binding_origins = vec![origin];
            bound.value.rebase_origin(origin);
        }
    }
}

struct SemanticEvaluator<'db> {
    context: EvaluationContext<'db>,
}

impl StatementInterpreter for SemanticEvaluator<'_> {
    type State = EvaluationState;

    fn walk_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAssign) {
        let value = expression::evaluate_binding(&self.context, state, &assign.value);
        for target in &assign.targets {
            assign_target(&self.context, state, target, &assign.value, value.clone());
        }
    }

    fn walk_ann_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAnnAssign) {
        if let Some(value) = &assign.value {
            let evaluated = expression::evaluate_binding(&self.context, state, value);
            assign_target(&self.context, state, &assign.target, value, evaluated);
        }
    }

    fn walk_aug_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAugAssign) {
        let origin = self.context.origin(assign);
        if assign.op == ast::Operator::Add
            && let Some(name) = assign.target.name_target()
            && let Some(left) = state.value_for_name(name)
        {
            let right = expression::evaluate_value(&self.context, state, &assign.value);
            state.assign_value(name, expression::add_values(left, right, origin), origin);
            state.mutations.push(PythonMutation {
                root: name.to_string(),
                access: Vec::new(),
                method: "extend".to_string(),
                origin,
            });
            return;
        }

        if assign.op == ast::Operator::Add
            && let Some(target) = MutationTarget::from_expr(&assign.target)
        {
            let extension = expression::evaluate_value(&self.context, state, &assign.value);
            super::mutation::apply_augmented_add(state, &target, &extension, origin);
            return;
        }

        bind_unknown_targets(
            &self.context,
            state,
            &assign.target,
            PythonUnknownCause::UnsupportedMutation,
        );
    }

    fn walk_import(&mut self, state: &mut Self::State, import: &ast::StmtImport) {
        for alias in &import.names {
            let bound_name = alias.asname.as_ref().map_or_else(
                || first_import_segment(alias.name.as_str()),
                ast::Identifier::as_str,
            );
            state.bind_unknown(
                bound_name,
                PythonUnknownCause::UnsupportedExpression,
                self.context.origin(alias),
            );
        }
    }

    fn walk_import_from(&mut self, state: &mut Self::State, import: &ast::StmtImportFrom) {
        imports::walk_import_from(&self.context, state, import);
    }

    fn walk_expr(&mut self, state: &mut Self::State, expr: &ast::StmtExpr) {
        super::mutation::walk_expr(&self.context, state, &expr.value);
    }

    fn bind_for_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        bind_unknown_targets(
            &self.context,
            state,
            target,
            PythonUnknownCause::UnsupportedExpression,
        );
    }

    fn bind_with_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        self.bind_for_target(state, target);
    }

    fn bind_function(&mut self, state: &mut Self::State, function: &ast::StmtFunctionDef) {
        state.bind_unknown(
            function.name.as_str(),
            PythonUnknownCause::UnsupportedExpression,
            self.context.origin(function),
        );
    }

    fn bind_class(&mut self, state: &mut Self::State, class: &ast::StmtClassDef) {
        state.bind_unknown(
            class.name.as_str(),
            PythonUnknownCause::UnsupportedExpression,
            self.context.origin(class),
        );
    }

    fn bind_delete_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        bind_unknown_targets(
            &self.context,
            state,
            target,
            PythonUnknownCause::UnsupportedMutation,
        );
    }

    fn bind_type_alias(&mut self, state: &mut Self::State, alias: &ast::StmtTypeAlias) {
        bind_unknown_targets(
            &self.context,
            state,
            &alias.name,
            PythonUnknownCause::UnsupportedExpression,
        );
    }

    fn bind_pattern_names(&mut self, state: &mut Self::State, pattern: &ast::Pattern) {
        for name in pattern_bound_names(pattern) {
            state.bind_unknown(
                name,
                PythonUnknownCause::UnsupportedExpression,
                self.context.origin(pattern),
            );
        }
    }

    fn evaluate_test(&self, state: &Self::State, expr: &ast::Expr) -> Truthiness {
        evaluate_test_with(expr, |name| state.bool_value(name))
    }

    fn degrade_loop_bodies(
        &mut self,
        mut state: Self::State,
        bodies: &[&[ast::Stmt]],
        control_span: Span,
    ) -> Self::State {
        let base = state.clone();
        let mut writes = TouchedNames::default();
        for body in bodies {
            let body_state = statement_walk::walk_body(self, base.clone(), body);
            writes.merge(EvaluationState::changed_writes_from(&base, &body_state));
            extend_unique(
                &mut state.dependencies.files,
                &body_state.dependencies.files,
            );
            extend_unique(
                &mut state.dependencies.imports,
                &body_state.dependencies.imports,
            );
            extend_unique(&mut state.mutations, &body_state.mutations);
            state.namespace_causes.extend(body_state.namespace_causes);
        }
        state.degrade_names(
            writes.names,
            PythonUnknownCause::UnsupportedExpression,
            self.context.origin_at(control_span),
        );
        state
    }

    fn join_ambiguous_paths(
        &mut self,
        state: Self::State,
        paths: &[BranchPath<'_>],
        control_span: Span,
    ) -> Self::State {
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
                branch = statement_walk::walk_body(self, branch, segment);
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
        state: Self::State,
        cases: &[ast::MatchCase],
        control_span: Span,
    ) -> Self::State {
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
            branches.push(statement_walk::walk_body(self, branch, &case.body));
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
            PythonUnknownCause::UnsupportedExpression,
        );
    }
}

pub(super) fn evaluate_value(
    context: &EvaluationContext<'_>,
    state: &EvaluationState,
    expression: &ast::Expr,
) -> PythonValue {
    expression::evaluate_value(context, state, expression)
}

#[allow(clippy::needless_pass_by_value)]
fn bind_unknown_targets(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    target: &ast::Expr,
    cause: PythonUnknownCause,
) {
    let origin = context.origin(target);
    for name in target_write_names(target) {
        state.bind_unknown(name, cause.clone(), origin);
    }
}

fn extend_unique<T: Clone + PartialEq>(target: &mut Vec<T>, incoming: &[T]) {
    for item in incoming {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}
