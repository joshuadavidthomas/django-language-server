use djls_source::File;
use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;

use super::control_flow::BranchPath;
use super::control_flow::Truthiness;
use super::control_flow::evaluate_test_with;
use super::control_flow::is_irrefutable_match_case;
use super::evaluation::BranchConstraints;
use super::evaluation::PythonBinding;
use super::evaluation::PythonBindingAlternative;
use super::evaluation::PythonBindings;
use super::evaluation::PythonBoundValue;
use super::evaluation::PythonDict;
use super::evaluation::PythonDictItem;
use super::evaluation::PythonImportOutcome;
use super::evaluation::PythonList;
use super::evaluation::PythonListItem;
use super::evaluation::PythonModuleDependencies;
use super::evaluation::PythonModuleEvaluation;
use super::evaluation::PythonModuleValues;
use super::evaluation::PythonModuleValuesOutcome;
use super::evaluation::PythonMutation;
use super::evaluation::PythonNamespaceCause;
use super::evaluation::PythonNamespaceRemainder;
use super::evaluation::PythonUnknown;
use super::evaluation::PythonUnknownCause;
use super::evaluation::PythonValue;
use super::evaluation::PythonValueKind;
use super::mutation_target::MutationAccess;
use super::mutation_target::MutationTarget;
use super::mutations::PythonMutationAccess;
use super::statement_walk::StatementInterpreter;
use super::statement_walk::{
    self,
};
use super::touched_names::TouchedNames;
use super::touched_names::collect_touched_names;
use super::touched_names::expr_read_names;
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

struct EvaluationContext<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    file: File,
}

impl EvaluationContext<'_> {
    fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.file, ranged.span())
    }

    fn origin_at(&self, span: Span) -> Origin {
        Origin::new(self.file, span)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct EvaluationState {
    bindings: PythonBindings,
    namespace_causes: Vec<PythonNamespaceCause>,
    mutations: Vec<PythonMutation>,
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
            let unknown = constrained_unknown_binding(cause.clone(), origin, constraints)
                .expect("a namespace cause must have a feasible branch");
            *binding = binding.clone().join(unknown, origin);
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn invalidate_names(
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
    fn degrade_names(
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
                    if let Some(unknown) = constrained_unknown_binding(
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
        let value = evaluate_binding(&self.context, state, &assign.value);
        for target in &assign.targets {
            assign_target(&self.context, state, target, &assign.value, value.clone());
        }
    }

    fn walk_ann_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAnnAssign) {
        if let Some(value) = &assign.value {
            let evaluated = evaluate_binding(&self.context, state, value);
            assign_target(&self.context, state, &assign.target, value, evaluated);
        }
    }

    fn walk_aug_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAugAssign) {
        let origin = self.context.origin(assign);
        if assign.op == ast::Operator::Add
            && let Some(name) = assign.target.name_target()
            && let Some(left) = state.value_for_name(name)
        {
            let right = evaluate_value(&self.context, state, &assign.value);
            state.assign_value(name, add_values(left, right, origin), origin);
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
            let extension = evaluate_value(&self.context, state, &assign.value);
            let supported = mutate_target(state, &target, origin, |value| {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                match &extension.kind {
                    PythonValueKind::List(extension) => list.extend(extension, origin),
                    PythonValueKind::Unknown(unknown) => {
                        list.append(&PythonListItem::UnknownUnpack(unknown.clone()));
                    }
                    PythonValueKind::Str(_)
                    | PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::Dict(_) => return false,
                }
                true
            });
            state.mutations.push(PythonMutation {
                root: target.root.to_string(),
                access: target.access.iter().map(access_to_public).collect(),
                method: "extend".to_string(),
                origin,
            });
            if !supported {
                state.degrade_names(
                    [target.root.to_string()],
                    PythonUnknownCause::UnsupportedMutation,
                    origin,
                );
            }
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
        walk_import_from(&self.context, state, import);
    }

    fn walk_expr(&mut self, state: &mut Self::State, expr: &ast::StmtExpr) {
        walk_expr(&self.context, state, &expr.value);
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

fn walk_import_from(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    import: &ast::StmtImportFrom,
) {
    let origin = context.origin(import);
    let is_star = import.names.iter().any(|alias| alias.name.as_str() == "*");
    let request = PythonImportRequest {
        level: import.level,
        module: import.module.as_ref().map(ast::Identifier::as_str),
        importer: context.file.path(context.db),
    };
    let module = match PythonModule::resolve_import(context.db, context.project, request) {
        Err(reason) => {
            state.record_import(PythonImportOutcome::InvalidImport {
                origin,
                reason: reason.clone(),
            });
            apply_failed_import(
                state,
                import,
                is_star,
                origin,
                PythonUnknownCause::InvalidImport(reason),
            );
            return;
        }
        Ok(None) => {
            let Some(module) = import
                .module
                .as_ref()
                .and_then(|name| crate::python::PythonModuleName::parse(name.as_str()).ok())
            else {
                return;
            };
            state.record_import(PythonImportOutcome::NotFound {
                origin,
                module: module.clone(),
            });
            apply_failed_import(
                state,
                import,
                is_star,
                origin,
                PythonUnknownCause::ImportNotFound(module),
            );
            return;
        }
        Ok(Some(module)) => module,
    };
    if !is_star && !module.search_path().is_first_party() {
        state.record_import(PythonImportOutcome::SkippedExternal {
            origin,
            module: module.name().clone(),
        });
        apply_failed_import(
            state,
            import,
            false,
            origin,
            PythonUnknownCause::SkippedExternal(module.name().clone()),
        );
        return;
    }

    apply_resolved_import(context, state, import, is_star, origin, module.file());
}

fn apply_resolved_import(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    import: &ast::StmtImportFrom,
    is_star: bool,
    origin: Origin,
    imported_file: File,
) {
    extend_unique(&mut state.dependencies.files, &[imported_file]);
    let imported =
        super::evaluation_query::evaluate_python_module(context.db, context.project, imported_file);
    if imported.is_cycle_seed() {
        state.record_import(PythonImportOutcome::Cycle {
            origin,
            file: imported_file,
        });
        apply_failed_import(state, import, is_star, origin, PythonUnknownCause::Cycle);
        return;
    }
    state.absorb_dependencies(&imported);
    match &imported.values {
        PythonModuleValuesOutcome::Unreadable(error) => {
            state.record_import(PythonImportOutcome::Unreadable {
                origin,
                file: imported_file,
                error: error.clone(),
            });
            apply_failed_import(
                state,
                import,
                is_star,
                origin,
                PythonUnknownCause::Unreadable(error.clone()),
            );
        }
        PythonModuleValuesOutcome::Readable(values) => {
            let outcome = if values.syntax_errors.is_empty() {
                PythonImportOutcome::Resolved {
                    origin,
                    file: imported_file,
                }
            } else {
                PythonImportOutcome::SyntaxErrors {
                    origin,
                    file: imported_file,
                    errors: values.syntax_errors.clone(),
                }
            };
            state.record_import(outcome);
            if is_star {
                state.apply_star_import(values, origin);
            } else {
                for alias in &import.names {
                    let imported_name = alias.name.as_str();
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or(imported_name, ast::Identifier::as_str);
                    state.bind_named_import(
                        values,
                        imported_name,
                        bound_name,
                        context.origin(alias),
                    );
                }
            }
        }
    }
}

fn constrained_unknown_binding(
    cause: PythonUnknownCause,
    origin: Origin,
    constraints: &BranchConstraints,
) -> Option<PythonBinding> {
    PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
        value: PythonValue::unknown(cause, Some(origin)),
        binding_origins: vec![origin],
    })])
    .intersect_constraints(constraints)
}

fn apply_failed_import(
    state: &mut EvaluationState,
    import: &ast::StmtImportFrom,
    is_star: bool,
    origin: Origin,
    cause: PythonUnknownCause,
) {
    if is_star {
        state.degrade_all_bindings(cause.clone(), origin, &BranchConstraints::unconstrained());
        state
            .namespace_causes
            .push(PythonNamespaceCause::unconstrained(PythonUnknown {
                cause,
                origin: Some(origin),
            }));
    } else {
        for alias in &import.names {
            let bound_name = alias
                .asname
                .as_ref()
                .map_or_else(|| alias.name.as_str(), ast::Identifier::as_str);
            state.bind_unknown(bound_name, cause.clone(), origin);
        }
    }
}

fn evaluate_binding(
    context: &EvaluationContext<'_>,
    state: &EvaluationState,
    expression: &ast::Expr,
) -> PythonBinding {
    let origin = context.origin(expression);
    if let Some(value) = expression.string_literal() {
        return value_binding(
            known_value(PythonValueKind::Str(value.to_string()), origin),
            origin,
        );
    }
    if let Some(value) = expression.bool_literal() {
        return value_binding(known_value(PythonValueKind::Bool(value), origin), origin);
    }
    if let Some(path) = evaluate_path(
        expression,
        context.file.path(context.db),
        &state.path_bindings(),
    ) {
        return value_binding(known_value(PythonValueKind::Path(path), origin), origin);
    }
    if let Some(name) = expression.name_target()
        && let Some(binding) = state.binding(name)
    {
        return binding.clone();
    }
    match expression {
        ast::Expr::List(list) => evaluate_list_binding(context, state, &list.elts, origin),
        ast::Expr::Tuple(tuple) => evaluate_list_binding(context, state, &tuple.elts, origin),
        ast::Expr::BinOp(binary) if binary.op == ast::Operator::Add => combine_bindings(
            &evaluate_binding(context, state, &binary.left),
            &evaluate_binding(context, state, &binary.right),
            origin,
            |left, right| add_values(left, right, origin),
        ),
        ast::Expr::Dict(dict) => evaluate_dict_binding(context, state, dict, origin),
        _ => value_binding(
            PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
            origin,
        ),
    }
}

fn evaluate_value(
    context: &EvaluationContext<'_>,
    state: &EvaluationState,
    expression: &ast::Expr,
) -> PythonValue {
    let origin = context.origin(expression);
    evaluate_binding(context, state, expression)
        .single_bound()
        .map_or_else(
            || PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
            |bound| bound.value.clone(),
        )
}

fn value_binding(value: PythonValue, origin: Origin) -> PythonBinding {
    PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
        value,
        binding_origins: vec![origin],
    })])
}

fn correlated_value_binding(
    value: PythonValue,
    origin: Origin,
    constraints: BranchConstraints,
) -> PythonBinding {
    PythonBinding::correlated(
        PythonBindingAlternative::Bound(PythonBoundValue {
            value,
            binding_origins: vec![origin],
        }),
        constraints,
    )
}

fn combine_bindings(
    left: &PythonBinding,
    right: &PythonBinding,
    origin: Origin,
    combine: impl Fn(PythonValue, PythonValue) -> PythonValue,
) -> PythonBinding {
    let mut result = None;
    for (left, left_constraints) in left.alternatives_with_constraints() {
        let PythonBindingAlternative::Bound(left) = left else {
            continue;
        };
        for (right, right_constraints) in right.alternatives_with_constraints() {
            let PythonBindingAlternative::Bound(right) = right else {
                continue;
            };
            let constraints = left_constraints.intersection(right_constraints);
            if constraints.normalized_alternatives().is_empty() {
                continue;
            }
            let alternative = correlated_value_binding(
                combine(left.value.clone(), right.value.clone()),
                origin,
                constraints,
            );
            result = Some(
                result.map_or(alternative.clone(), |current: PythonBinding| {
                    current.join(alternative, origin)
                }),
            );
        }
    }
    result.unwrap_or_else(|| {
        value_binding(
            PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
            origin,
        )
    })
}

fn known_value(kind: PythonValueKind, origin: Origin) -> PythonValue {
    PythonValue::known(kind, origin)
}

fn evaluate_list_binding(
    context: &EvaluationContext<'_>,
    state: &EvaluationState,
    elements: &[ast::Expr],
    origin: Origin,
) -> PythonBinding {
    let mut lists = value_binding(
        known_value(PythonValueKind::List(PythonList::new(Vec::new())), origin),
        origin,
    );
    for element in elements {
        let element_origin = context.origin(element);
        let (expression, starred) = match element {
            ast::Expr::Starred(starred) => (starred.value.as_ref(), true),
            _ => (element, false),
        };
        let values = evaluate_binding(context, state, expression);
        lists = combine_bindings(&lists, &values, element_origin, |mut result, value| {
            let PythonValueKind::List(list) = &mut result.kind else {
                unreachable!("list construction starts with a list")
            };
            if starred {
                match value.kind {
                    PythonValueKind::List(unpacked) => list.extend(&unpacked, element_origin),
                    PythonValueKind::Unknown(unknown) => {
                        list.append(&PythonListItem::UnknownUnpack(unknown));
                    }
                    PythonValueKind::Str(_)
                    | PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::Dict(_) => {
                        list.append(&PythonListItem::UnknownUnpack(PythonUnknown {
                            cause: PythonUnknownCause::UnsupportedExpression,
                            origin: Some(element_origin),
                        }));
                    }
                }
            } else {
                let item = match value.kind {
                    PythonValueKind::Unknown(unknown) => PythonListItem::UnknownElement(unknown),
                    PythonValueKind::Str(_)
                    | PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::List(_)
                    | PythonValueKind::Dict(_) => PythonListItem::Value(value),
                };
                list.append(&item);
            }
            result
        });
    }
    lists
}

fn add_values(left: PythonValue, right: PythonValue, origin: Origin) -> PythonValue {
    match (left.kind, right.kind) {
        (PythonValueKind::List(mut left), PythonValueKind::List(right)) => {
            left.extend(&right, origin);
            known_value(PythonValueKind::List(left), origin)
        }
        (
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::List(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Unknown(_),
            _,
        ) => PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
    }
}

fn evaluate_dict_binding(
    context: &EvaluationContext<'_>,
    state: &EvaluationState,
    dictionary: &ast::ExprDict,
    origin: Origin,
) -> PythonBinding {
    let mut dictionaries = value_binding(
        known_value(
            PythonValueKind::Dict(PythonDict { items: Vec::new() }),
            origin,
        ),
        origin,
    );
    for item in &dictionary.items {
        let item_origin = context.origin(&item.value);
        let Some(key) = &item.key else {
            let unpacked = evaluate_binding(context, state, &item.value);
            dictionaries = combine_bindings(
                &dictionaries,
                &unpacked,
                item_origin,
                |mut result, unpacked| {
                    let PythonValueKind::Dict(dictionary) = &mut result.kind else {
                        unreachable!("dictionary construction starts with a dictionary")
                    };
                    match unpacked.kind {
                        PythonValueKind::Dict(unpacked) => dictionary.items.extend(unpacked.items),
                        PythonValueKind::Unknown(unknown) => {
                            dictionary
                                .items
                                .push(PythonDictItem::UnknownUnpack(unknown));
                        }
                        PythonValueKind::Str(_)
                        | PythonValueKind::Bool(_)
                        | PythonValueKind::Path(_)
                        | PythonValueKind::List(_) => {
                            dictionary
                                .items
                                .push(PythonDictItem::UnknownUnpack(PythonUnknown {
                                    cause: PythonUnknownCause::UnsupportedExpression,
                                    origin: Some(item_origin),
                                }));
                        }
                    }
                    result
                },
            );
            continue;
        };

        let keys = evaluate_binding(context, state, key);
        dictionaries = combine_bindings(&dictionaries, &keys, item_origin, |mut result, key| {
            let PythonValueKind::Dict(dictionary) = &mut result.kind else {
                unreachable!("dictionary construction starts with a dictionary")
            };
            dictionary.items.push(PythonDictItem::Entry {
                key,
                value: PythonValue::unknown(
                    PythonUnknownCause::UnsupportedExpression,
                    Some(item_origin),
                ),
            });
            result
        });
        let values = evaluate_binding(context, state, &item.value);
        dictionaries =
            combine_bindings(&dictionaries, &values, item_origin, |mut result, value| {
                let PythonValueKind::Dict(dictionary) = &mut result.kind else {
                    unreachable!("dictionary construction starts with a dictionary")
                };
                let Some(PythonDictItem::Entry { value: slot, .. }) = dictionary.items.last_mut()
                else {
                    unreachable!("a dictionary entry was just appended")
                };
                *slot = value;
                result
            });
    }
    dictionaries
}

fn walk_expr(context: &EvaluationContext<'_>, state: &mut EvaluationState, expression: &ast::Expr) {
    let ast::Expr::Call(call) = expression else {
        state.degrade_names(
            expr_read_names(expression),
            PythonUnknownCause::UnsupportedExpression,
            context.origin(expression),
        );
        return;
    };
    let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
        state.degrade_names(
            expr_read_names(expression),
            PythonUnknownCause::UnsupportedMutation,
            context.origin(expression),
        );
        return;
    };
    let Some(target) = MutationTarget::from_expr(&attribute.value) else {
        state.degrade_names(
            expr_read_names(expression),
            PythonUnknownCause::UnsupportedMutation,
            context.origin(expression),
        );
        return;
    };
    let method = attribute.attr.as_str();
    let origin = context.origin(call);
    let supported = apply_mutation_call(context, state, call, &target, method, origin);
    state.mutations.push(PythonMutation {
        root: target.root.to_string(),
        access: target.access.iter().map(access_to_public).collect(),
        method: method.to_string(),
        origin,
    });
    if !supported {
        state.invalidate_names(
            [target.root.to_string()],
            PythonUnknownCause::UnsupportedMutation,
            origin,
        );
    }
}

fn apply_mutation_call(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    call: &ast::ExprCall,
    target: &MutationTarget<'_>,
    method: &str,
    origin: Origin,
) -> bool {
    let arguments = call
        .arguments
        .args
        .iter()
        .map(|argument| evaluate_value(context, state, argument))
        .collect::<Vec<_>>();
    match (method, arguments.as_slice()) {
        ("append", [argument]) => mutate_target(state, target, origin, |value| {
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            list.append(&PythonListItem::Value(argument.clone()));
            true
        }),
        ("extend", [extension]) => mutate_target(state, target, origin, |value| {
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            match &extension.kind {
                PythonValueKind::List(extension) => list.extend(extension, origin),
                PythonValueKind::Unknown(unknown) => {
                    list.append(&PythonListItem::UnknownUnpack(unknown.clone()));
                }
                PythonValueKind::Str(_)
                | PythonValueKind::Bool(_)
                | PythonValueKind::Path(_)
                | PythonValueKind::Dict(_) => return false,
            }
            true
        }),
        ("insert", [_, argument]) => {
            let index = &call.arguments.args[0];
            let non_negative = index.non_negative_integer();
            let negative = index.negative_integer();
            (non_negative.is_some() || negative.is_some())
                && mutate_target(state, target, origin, |value| {
                    let PythonValueKind::List(list) = &mut value.kind else {
                        return false;
                    };
                    if !list_is_authoritative(list) {
                        return false;
                    }
                    let index = non_negative.map_or_else(
                        || {
                            let magnitude = negative.expect("insert index is an integer literal");
                            if magnitude == 0 {
                                0
                            } else {
                                list.items.len().saturating_sub(magnitude)
                            }
                        },
                        |index| index.min(list.items.len()),
                    );
                    list.insert(index, &PythonListItem::Value(argument.clone()));
                    true
                })
        }
        ("remove", [argument]) => mutate_target(state, target, origin, |value| {
            let PythonValueKind::Str(needle) = &argument.kind else {
                return false;
            };
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            if !list_is_authoritative(list) {
                return false;
            }
            let Some(index) = list.items.iter().position(|item| {
                matches!(item, PythonListItem::Value(PythonValue {
                    kind: PythonValueKind::Str(candidate), ..
                }) if candidate == needle)
            }) else {
                return false;
            };
            list.remove(index);
            true
        }),
        ("append" | "extend" | "insert" | "remove" | _, _) => false,
    }
}

fn list_is_authoritative(list: &PythonList) -> bool {
    list.items
        .iter()
        .all(|item| matches!(item, PythonListItem::Value(_)))
}

fn mutate_target(
    state: &mut EvaluationState,
    target: &MutationTarget<'_>,
    origin: Origin,
    mutate: impl Fn(&mut PythonValue) -> bool,
) -> bool {
    let Some(binding) = state.bindings.0.get_mut(target.root) else {
        return false;
    };
    let Some(bound) = binding.single_bound_mut() else {
        return false;
    };
    let supported = mutate_at_access(&mut bound.value, &target.access, origin, &mutate);
    if supported {
        bound.value.normalize();
    }
    supported
}

fn mutate_at_access(
    value: &mut PythonValue,
    access: &[MutationAccess],
    origin: Origin,
    mutate: &impl Fn(&mut PythonValue) -> bool,
) -> bool {
    let Some((next_access, remaining)) = access.split_first() else {
        if !mutate(value) {
            return false;
        }
        value.record_origin(origin);
        value.normalize();
        return true;
    };

    match next_access {
        MutationAccess::Index(index) => {
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            let Some(PythonListItem::Value(next)) = list.items.get_mut(*index) else {
                return false;
            };
            if !mutate_at_access(next, remaining, origin, mutate) {
                return false;
            }
            for variant in &mut list.variants {
                if super::evaluation::is_list_variant_limit_unknown(&variant.items) {
                    continue;
                }
                let Some(PythonListItem::Value(next)) = variant.items.get_mut(*index) else {
                    return false;
                };
                if !mutate_at_access(next, remaining, origin, mutate) {
                    return false;
                }
            }
            true
        }
        MutationAccess::Key(key) => {
            let PythonValueKind::Dict(dict) = &mut value.kind else {
                return false;
            };
            let Some(next) = dict.items.iter_mut().rev().find_map(|item| match item {
                PythonDictItem::Entry { key: candidate, value }
                    if matches!(&candidate.kind, PythonValueKind::Str(candidate) if candidate == key) =>
                {
                    Some(value)
                }
                PythonDictItem::Entry { .. } | PythonDictItem::UnknownUnpack(_) => None,
            }) else {
                return false;
            };
            mutate_at_access(next, remaining, origin, mutate)
        }
    }
}

fn access_to_public(access: &MutationAccess) -> PythonMutationAccess {
    match access {
        MutationAccess::Index(index) => PythonMutationAccess::Index(*index),
        MutationAccess::Key(key) => PythonMutationAccess::Key(key.clone()),
    }
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
