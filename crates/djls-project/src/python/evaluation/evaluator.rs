pub(super) mod expression;
mod imports;
mod statement;

#[cfg(test)]
use std::cell::Cell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
#[cfg(test)]
use std::rc::Rc;

use djls_source::File;
use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;

use super::BranchConstraints;
use super::BranchJoin;
use super::ChildImportFallback;
use super::PredicateIdentity;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonImportOutcome;
use super::PythonImportTrace;
use super::PythonModuleEffects;
use super::PythonModuleFacts;
use super::PythonMutation;
use super::PythonMutationOperation;
use super::PythonMutationPath;
use super::PythonMutationPathSegment;
use super::PythonNamespaceCause;
use super::PythonNamespaceRemainder;
use super::PythonSyntaxErrorImpact;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ReachableAllocationSites;
use super::UniqueVec;
use super::module_object::IntrinsicContamination;
use super::name_analysis::reachable_expr_calls;
use super::truthiness::Truthiness;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonIntrinsic;
use crate::python::PythonIntrinsicCall;
use crate::python::PythonIntrinsicNamespace;
use crate::python::PythonSourceModule;
use crate::python::PythonSyntaxError;

#[cfg(test)]
#[derive(Default)]
struct EvaluationStats {
    evaluated_expressions: Cell<usize>,
    materialized_dictionaries: Cell<usize>,
}

fn merge_truth_guard(target: &mut Option<BranchConstraints>, incoming: BranchConstraints) {
    if incoming.is_impossible() {
        return;
    }
    if let Some(existing) = target {
        existing.merge(incoming);
    } else {
        *target = Some(incoming);
    }
}

/// Context-bearing abstract evaluator that evaluates Python syntax into a forkable state.
pub(super) struct PythonModuleEvaluator<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    module: PythonSourceModule,
    pub(super) state: PythonEvaluationState,
    pub(super) active_constraints: BranchConstraints,
    // Forks share work counts; independent module evaluations get fresh statistics.
    #[cfg(test)]
    stats: Rc<EvaluationStats>,
}

struct TruthPartition {
    truthy: BranchConstraints,
    falsy: BranchConstraints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsupportedCallEffect {
    None,
    Arguments,
    ReceiverAndArguments,
}

impl<'db> PythonModuleEvaluator<'db> {
    pub(super) fn new(
        db: &'db dyn ProjectDb,
        project: Project,
        module: PythonSourceModule,
        intrinsic_contamination: IntrinsicContamination,
    ) -> Self {
        let state = PythonEvaluationState::with_intrinsic_contamination(
            module.file(),
            intrinsic_contamination,
            Some(module.clone()),
        );
        Self {
            db,
            project,
            module,
            state,
            active_constraints: BranchConstraints::unconstrained(),
            #[cfg(test)]
            stats: Rc::default(),
        }
    }

    pub(super) fn evaluate<'ast>(
        mut self,
        body: impl IntoIterator<Item = impl Into<super::slice::SelectedStatement<'ast>>>,
        syntax_errors: Vec<PythonSyntaxError>,
        syntax_impacts: Vec<PythonSyntaxErrorImpact>,
    ) -> (PythonModuleFacts, PythonImportTrace, PythonModuleEffects) {
        for statement in body {
            match statement.into() {
                super::slice::SelectedStatement::Full(statement) => {
                    self.evaluate_body(std::slice::from_ref(statement));
                }
                super::slice::SelectedStatement::UnobservedAssignment(assign) => {
                    self.walk_unobserved_assign(assign);
                }
            }
        }
        self.state.finish(syntax_errors, syntax_impacts)
    }

    fn fork(&self) -> Self {
        Self {
            db: self.db,
            project: self.project,
            module: self.module.clone(),
            state: self.state.clone(),
            active_constraints: self.active_constraints.clone(),
            #[cfg(test)]
            stats: Rc::clone(&self.stats),
        }
    }

    fn join_forks(&mut self, forks: Vec<Self>, origin: Origin) {
        let arm_count = forks.len();
        self.join_indexed_forks(forks.into_iter().enumerate().collect(), origin, arm_count);
    }

    fn join_indexed_forks(&mut self, forks: Vec<(usize, Self)>, origin: Origin, arm_count: usize) {
        let branches = forks
            .into_iter()
            .map(|(arm, evaluator)| (arm, evaluator.state))
            .collect::<Vec<_>>();
        let join = BranchJoin::new(self.module.clone(), origin, arm_count);
        self.state =
            PythonEvaluationState::join_indexed_branches(self.state.clone(), &branches, &join);
    }

    fn join_guarded_forks(&mut self, forks: Vec<(usize, Self)>, origin: Origin, arm_count: usize) {
        let branches = forks
            .into_iter()
            .map(|(arm, evaluator)| (arm, evaluator.active_constraints, evaluator.state))
            .collect::<Vec<_>>();
        let join = BranchJoin::new(self.module.clone(), origin, arm_count);
        self.state =
            PythonEvaluationState::join_guarded_branches(self.state.clone(), &branches, &join);
    }

    pub(super) fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.module.file(), ranged.span())
    }

    fn origin_at(&self, span: Span) -> Origin {
        Origin::new(self.module.file(), span)
    }

    fn truth_partition(
        &self,
        binding: &PythonBinding,
        fallback_origin: Origin,
        track_fallback: bool,
    ) -> TruthPartition {
        let mut truthy = None;
        let mut falsy = None;

        for (state, case_constraints, predicate_identity) in binding.alternatives_with_predicates()
        {
            let case_constraints = case_constraints.intersection(&self.active_constraints);
            if case_constraints.is_impossible() {
                continue;
            }
            match state {
                PythonBindingState::Bound(bound) => match &bound.value.kind {
                    PythonValueKind::Bool(true)
                    | PythonValueKind::Module(_)
                    | PythonValueKind::Intrinsic(
                        PythonIntrinsic::OsEnvironGetFunction | PythonIntrinsic::OsGetenvFunction,
                    ) => {
                        merge_truth_guard(&mut truthy, case_constraints);
                    }
                    PythonValueKind::Bool(false) => {
                        merge_truth_guard(&mut falsy, case_constraints);
                    }
                    PythonValueKind::Str(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::Intrinsic(
                        PythonIntrinsic::BuiltinsModule
                        | PythonIntrinsic::BuiltinsStrType
                        | PythonIntrinsic::PathlibModule
                        | PythonIntrinsic::PathlibPathType
                        | PythonIntrinsic::OsModule
                        | PythonIntrinsic::OsPathModule
                        | PythonIntrinsic::OsPathJoinFunction
                        | PythonIntrinsic::OsPathDirnameFunction
                        | PythonIntrinsic::OsPathAbspathFunction
                        | PythonIntrinsic::OsEnvironObject,
                    )
                    | PythonValueKind::UnsupportedLiteral
                    | PythonValueKind::List(_)
                    | PythonValueKind::Tuple(_)
                    | PythonValueKind::Dict(_)
                    | PythonValueKind::Unknown(_) => {
                        if let Some(identity) = predicate_identity
                            && (track_fallback || identity.origin() != fallback_origin)
                        {
                            self.add_ambiguous_truth_identity(
                                &mut truthy,
                                &mut falsy,
                                &case_constraints,
                                identity,
                            );
                        } else {
                            merge_truth_guard(&mut truthy, case_constraints.clone());
                            merge_truth_guard(&mut falsy, case_constraints);
                        }
                    }
                },
                PythonBindingState::Unbound => {
                    merge_truth_guard(&mut truthy, case_constraints.clone());
                    merge_truth_guard(&mut falsy, case_constraints);
                }
            }
        }

        TruthPartition {
            truthy: truthy.unwrap_or_else(BranchConstraints::impossible),
            falsy: falsy.unwrap_or_else(BranchConstraints::impossible),
        }
    }

    fn add_ambiguous_truth_identity(
        &self,
        truthy: &mut Option<BranchConstraints>,
        falsy: &mut Option<BranchConstraints>,
        constraints: &BranchConstraints,
        identity: &PredicateIdentity,
    ) {
        if constraints.is_impossible() {
            return;
        }
        let join = BranchJoin::predicate(self.module.clone(), identity);
        let falsy_constraints = constraints.intersection(&BranchConstraints::required(
            join.clone(),
            Truthiness::Falsy.branch_arm(),
        ));
        let truthy_constraints = constraints.intersection(&BranchConstraints::required(
            join,
            Truthiness::Truthy.branch_arm(),
        ));
        merge_truth_guard(falsy, falsy_constraints);
        merge_truth_guard(truthy, truthy_constraints);
    }

    /// Invalidate intrinsic namespaces and aliases exposed to unsupported calls.
    /// Value evaluation is read-only, so statement execution applies this separately,
    /// even when the expression's result will be discarded.
    pub(super) fn invalidate_intrinsics_for_calls(&mut self, expression: &ast::Expr) {
        let calls = reachable_expr_calls(expression, &|value| {
            Truthiness::of_expr(value, &|name| self.state.known_truthiness(name))
        });
        for call in calls {
            let include_receiver = match self.unsupported_call_effect(&call) {
                UnsupportedCallEffect::None => continue,
                UnsupportedCallEffect::Arguments => false,
                UnsupportedCallEffect::ReceiverAndArguments => true,
            };
            let mut mutation_candidates = Vec::new();
            if include_receiver && let ast::Expr::Attribute(attribute) = call.func.as_ref() {
                mutation_candidates.push(self.evaluate_binding(&attribute.value));
            }
            mutation_candidates.extend(
                call.arguments
                    .args
                    .iter()
                    .map(|argument| self.evaluate_binding(argument)),
            );
            mutation_candidates.extend(
                call.arguments
                    .keywords
                    .iter()
                    .map(|keyword| self.evaluate_binding(&keyword.value)),
            );
            self.state
                .degrade_intrinsic_values(&mutation_candidates, self.origin(&call));
        }
    }

    pub(super) fn call_is_known_env_read(&self, call: &ast::ExprCall) -> bool {
        let binding = self.evaluate_binding(&call.func);
        let mut alternatives = binding.alternatives();
        let Some(first) = alternatives.next() else {
            return false;
        };
        let is_env_read = |alternative: &PythonBindingState| {
            let PythonBindingState::Bound(bound) = alternative else {
                return false;
            };
            let PythonValueKind::Intrinsic(intrinsic) = &bound.value.kind else {
                return false;
            };
            intrinsic.call() == Some(PythonIntrinsicCall::EnvironmentRead)
                && !self
                    .state
                    .module_effects
                    .namespace_is_contaminated(intrinsic.mutable_namespace())
        };
        is_env_read(first) && alternatives.all(is_env_read)
    }

    fn unsupported_call_effect(&self, call: &ast::ExprCall) -> UnsupportedCallEffect {
        let binding = if let ast::Expr::Attribute(attribute) = call.func.as_ref()
            && matches!(attribute.attr.as_str(), "resolve" | "joinpath")
        {
            self.evaluate_binding(&attribute.value)
        } else {
            self.evaluate_binding(&call.func)
        };
        let is_path_method = matches!(
            call.func.as_ref(),
            ast::Expr::Attribute(attribute)
                if matches!(attribute.attr.as_str(), "resolve" | "joinpath")
        );
        let mut has_known_nonmutating = false;
        let mut has_unsupported = false;
        for alternative in binding.alternatives() {
            let known = match alternative {
                PythonBindingState::Bound(bound) if is_path_method => {
                    matches!(&bound.value.kind, PythonValueKind::Path(_))
                }
                PythonBindingState::Bound(bound) => match &bound.value.kind {
                    PythonValueKind::Intrinsic(intrinsic) => {
                        intrinsic.call().is_some()
                            && !self
                                .state
                                .module_effects
                                .namespace_is_contaminated(intrinsic.mutable_namespace())
                    }
                    PythonValueKind::Str(_)
                    | PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::UnsupportedLiteral
                    | PythonValueKind::List(_)
                    | PythonValueKind::Tuple(_)
                    | PythonValueKind::Dict(_)
                    | PythonValueKind::Module(_)
                    | PythonValueKind::Unknown(_) => false,
                },
                PythonBindingState::Unbound => false,
            };
            has_known_nonmutating |= known;
            has_unsupported |= !known;
        }
        match (has_known_nonmutating, has_unsupported) {
            (true, false) => UnsupportedCallEffect::None,
            (true, true) => UnsupportedCallEffect::Arguments,
            (false, _) => UnsupportedCallEffect::ReceiverAndArguments,
        }
    }
}

/// Cloneable abstract state for context-free evaluation transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PythonEvaluationState {
    pub(super) bindings: BTreeMap<String, PythonBinding>,
    namespace_causes: Vec<PythonNamespaceCause>,
    pub(super) mutations: UniqueVec<PythonMutation>,
    import_trace: PythonImportTrace,
    /// Private recursive-import effect state. It is never projected into
    /// `PythonModuleFacts` equality; only the complete internal result carries
    /// it out through `evaluate_python_module`.
    module_effects: PythonModuleEffects,
    pub(super) predicate_module: Option<PythonSourceModule>,
}

impl PythonEvaluationState {
    #[cfg(test)]
    fn new(file: File) -> Self {
        Self::with_intrinsic_contamination(file, IntrinsicContamination::default(), None)
    }

    fn with_intrinsic_contamination(
        file: File,
        intrinsic_contamination: IntrinsicContamination,
        predicate_module: Option<PythonSourceModule>,
    ) -> Self {
        Self {
            bindings: BTreeMap::new(),
            namespace_causes: Vec::new(),
            mutations: UniqueVec::new(),
            import_trace: PythonImportTrace::rooted(file),
            module_effects: PythonModuleEffects::with_intrinsic_contamination(
                intrinsic_contamination,
            ),
            predicate_module,
        }
    }

    fn finish(
        self,
        syntax_errors: Vec<PythonSyntaxError>,
        syntax_impacts: Vec<PythonSyntaxErrorImpact>,
    ) -> (PythonModuleFacts, PythonImportTrace, PythonModuleEffects) {
        (
            PythonModuleFacts {
                bindings: self.bindings,
                namespace_remainder: (!self.namespace_causes.is_empty())
                    .then(|| PythonNamespaceRemainder::new(self.namespace_causes)),
                syntax_errors,
                syntax_impacts,
                mutations: self.mutations,
            },
            self.import_trace,
            self.module_effects,
        )
    }

    fn binding(&self, name: &str) -> Option<&PythonBinding> {
        self.bindings.get(name)
    }

    fn binding_with_intrinsic(
        &self,
        name: &str,
        intrinsic: PythonIntrinsic,
        origin: Origin,
    ) -> PythonBinding {
        let value = if self
            .module_effects
            .namespace_is_contaminated(intrinsic.mutable_namespace())
        {
            PythonValue::unknown(PythonUnknownCause::UnsupportedMutation, Some(origin))
        } else {
            PythonValue::intrinsic(intrinsic, origin)
        };
        self.binding_with_implicit_value(name, value, origin)
    }

    fn binding_with_implicit_value(
        &self,
        name: &str,
        value: PythonValue,
        origin: Origin,
    ) -> PythonBinding {
        let mut fallback = PythonBinding::bound(value, origin);
        for cause in &self.namespace_causes {
            if let Some(unknown) =
                PythonBinding::constrained_unknown(&cause.unknown.cause, origin, &cause.constraints)
            {
                fallback = fallback.join(unknown, origin);
            }
        }
        match self.binding(name).cloned() {
            Some(binding) => binding.replace_unbound_with(Some(fallback), origin),
            None => fallback,
        }
    }

    fn assign_value(&mut self, name: &str, value: PythonValue, origin: Origin) {
        self.assign_binding(name, PythonBinding::bound(value, origin), origin);
    }

    fn assign_intrinsic(&mut self, name: &str, intrinsic: PythonIntrinsic, origin: Origin) {
        if self
            .module_effects
            .namespace_is_contaminated(intrinsic.mutable_namespace())
        {
            self.bind_unknown(name, &PythonUnknownCause::UnsupportedMutation, origin);
        } else {
            self.assign_value(name, PythonValue::intrinsic(intrinsic, origin), origin);
        }
    }

    /// Update a name's single bound value after a successful in-place mutation.
    /// This preserves the binding's assignment origins, branch constraints, and
    /// prior mutation facts; only rebinding operations replace that state.
    ///
    /// If the binding no longer has one bound value, retain that loss of
    /// precision as an unknown instead of treating it as an evaluator invariant.
    fn update_bound_value(
        &mut self,
        name: &str,
        value: PythonValue,
        origin: Origin,
        active_constraints: &BranchConstraints,
    ) {
        let updated = self.bindings.get(name).cloned().and_then(|binding| {
            let mut binding = binding.intersect_constraints(active_constraints)?;
            binding.single_bound_mut()?.value = value;
            binding.replace_predicate_identities(name, origin, self.predicate_module.as_ref());
            Some(binding)
        });
        if let Some(binding) = updated {
            self.bindings.insert(name.to_string(), binding);
        } else {
            self.assign_value(
                name,
                PythonValue::unknown(PythonUnknownCause::UnsupportedMutation, Some(origin)),
                origin,
            );
        }
    }

    fn assign_binding(&mut self, name: &str, mut binding: PythonBinding, origin: Origin) {
        self.mutations.retain(|mutation| mutation.binding != name);
        binding.discriminate_predicates(name, origin, self.predicate_module.as_ref());
        self.bindings
            .insert(name.to_string(), binding.rebase_binding_origin(origin));
    }

    fn assign_from_name(
        &mut self,
        name: &str,
        source: &str,
        mut binding: PythonBinding,
        origin: Origin,
    ) {
        binding.discriminate_predicates(name, origin, self.predicate_module.as_ref());
        self.bindings
            .insert(name.to_string(), binding.rebase_binding_origin(origin));
        let copied = self
            .mutations
            .iter()
            .filter(|mutation| mutation.binding == source)
            .cloned()
            .map(|mut mutation| {
                mutation.binding = name.to_string();
                mutation
            })
            .collect::<Vec<_>>();
        self.mutations.retain(|mutation| mutation.binding != name);
        self.mutations.extend(copied);
    }

    fn bind_unknown(&mut self, name: &str, cause: &PythonUnknownCause, origin: Origin) {
        self.assign_binding(name, PythonBinding::unknown(cause, origin), origin);
    }

    pub(super) fn binding_has_intrinsic(&self, name: &str) -> bool {
        self.binding(name).is_some_and(|binding| {
            binding.alternatives().any(|alternative| {
                matches!(alternative, PythonBindingState::Bound(bound) if bound.value.mutable_namespace().is_some())
            })
        })
    }

    pub(super) fn intrinsic_write_names(&mut self, name: &str) -> Vec<String> {
        let Some(binding) = self.binding(name) else {
            return vec![name.to_string()];
        };
        let namespaces = binding
            .alternatives()
            .filter_map(|alternative| {
                let PythonBindingState::Bound(bound) = alternative else {
                    return None;
                };
                bound.value.mutable_namespace()
            })
            .collect::<Vec<_>>();
        if namespaces.is_empty() {
            return vec![name.to_string()];
        }
        for namespace in &namespaces {
            self.module_effects.contaminate_namespace(*namespace);
        }
        self.bindings
            .iter()
            .filter(|&(_candidate, binding)| {
                binding.alternatives().any(|alternative| {
                    let PythonBindingState::Bound(bound) = alternative else {
                        return false;
                    };
                    bound
                        .value
                        .mutable_namespace()
                        .is_some_and(|candidate| namespaces.contains(&candidate))
                })
            })
            .map(|(candidate, _binding)| candidate.clone())
            .collect()
    }

    fn all_intrinsic_write_names(&mut self) -> Vec<String> {
        let names = self
            .bindings
            .iter()
            .filter(|&(_name, binding)| {
                binding.alternatives().any(|alternative| {
                    let PythonBindingState::Bound(bound) = alternative else {
                        return false;
                    };
                    bound.value.mutable_namespace().is_some()
                })
            })
            .map(|(name, _binding)| name.clone())
            .collect::<Vec<_>>();
        for namespace in [
            PythonIntrinsicNamespace::Builtins,
            PythonIntrinsicNamespace::Pathlib,
            PythonIntrinsicNamespace::Os,
        ] {
            self.module_effects.contaminate_namespace(namespace);
        }
        names
    }

    fn mutable_alias_names(&self, binding: &PythonBinding) -> Vec<String> {
        let wanted = binding.reachable_allocation_sites();
        if wanted.is_empty() {
            return Vec::new();
        }
        self.bindings
            .iter()
            .filter(|(_name, candidate)| candidate.reachable_allocation_sites().intersects(&wanted))
            .map(|(name, _binding)| name.clone())
            .collect()
    }

    pub(super) fn stale_alias_names_after_mutation(
        &self,
        name: &str,
        path: &PythonMutationPath,
        active_constraints: &BranchConstraints,
    ) -> Vec<String> {
        let mut wanted = ReachableAllocationSites::default();
        let Some(binding) = self
            .binding(name)
            .cloned()
            .and_then(|binding| binding.intersect_constraints(active_constraints))
        else {
            return Vec::new();
        };
        for state in binding.alternatives() {
            let PythonBindingState::Bound(bound) = state else {
                continue;
            };
            wanted.absorb(path.possible_target_allocation_sites(&bound.value));
        }
        if wanted.is_empty() {
            return Vec::new();
        }
        self.bindings
            .iter()
            .filter(|(candidate_name, candidate)| {
                let occurrences = (*candidate)
                    .clone()
                    .intersect_constraints(active_constraints)
                    .map_or(0, |candidate| {
                        candidate.allocation_site_occurrences(&wanted)
                    });
                occurrences > usize::from(candidate_name.as_str() == name)
            })
            .map(|(name, _binding)| name.clone())
            .collect()
    }

    fn value_for_name(&self, name: &str, constraints: &BranchConstraints) -> Option<PythonValue> {
        self.binding(name)?
            .clone()
            .intersect_constraints(constraints)?
            .merged_semantic_value()
    }

    fn bool_value(&self, name: &str) -> Option<bool> {
        let binding = self.binding(name)?;
        let mut values = binding.alternatives();
        let PythonBindingState::Bound(first) = values.next()? else {
            return None;
        };
        let PythonValueKind::Bool(value) = first.value.kind else {
            return None;
        };
        values
            .all(|alternative| {
                matches!(alternative, PythonBindingState::Bound(bound)
                    if matches!(bound.value.kind, PythonValueKind::Bool(other) if other == value))
            })
            .then_some(value)
    }

    /// The definite truthiness of a name, if any: a uniformly boolean binding
    /// yields its constant value, and a uniformly module-valued binding is
    /// always truthy (Python module objects are never falsy).
    pub(super) fn known_truthiness(&self, name: &str) -> Option<Truthiness> {
        if let Some(value) = self.bool_value(name) {
            return Some(Truthiness::from_bool(value));
        }
        let binding = self.binding(name)?;
        let is_always_truthy = |state: &PythonBindingState| {
            matches!(state, PythonBindingState::Bound(bound)
            if matches!(
                bound.value.kind,
                PythonValueKind::Module(_)
                    | PythonValueKind::Intrinsic(
                        PythonIntrinsic::OsEnvironGetFunction
                            | PythonIntrinsic::OsGetenvFunction
                    )
            ))
        };
        let mut alternatives = binding.alternatives();
        let first = alternatives.next()?;
        (is_always_truthy(first) && alternatives.all(is_always_truthy))
            .then_some(Truthiness::Truthy)
    }

    fn degrade_all_bindings(
        &mut self,
        cause: &PythonUnknownCause,
        origin: Origin,
        constraints: &BranchConstraints,
    ) {
        let Some(unknown) = PythonBinding::constrained_unknown(cause, origin, constraints) else {
            return;
        };
        for (name, binding) in &mut self.bindings {
            let mut degraded = unknown.clone();
            degraded.discriminate_predicates(name, origin, self.predicate_module.as_ref());
            if let Some(module) = &self.predicate_module {
                let choice = BranchJoin::binding_choice(module.clone(), origin, name);
                let mut prior = binding.clone();
                prior.select_branch(choice.clone(), 0);
                degraded.select_branch(choice, 1);
                *binding = prior.join(degraded, origin);
            } else {
                *binding = binding.clone().join(degraded, origin);
            }
            binding.force_predicate_identity(name, origin, self.predicate_module.as_ref());
        }
    }

    pub(super) fn invalidate_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        cause: &PythonUnknownCause,
        origin: Origin,
    ) {
        for name in names {
            self.bind_unknown(&name, cause, origin);
        }
    }

    fn degrade_intrinsic_values(&mut self, values: &[PythonBinding], origin: Origin) {
        let namespaces = values
            .iter()
            .flat_map(PythonBinding::alternatives)
            .filter_map(|alternative| {
                let PythonBindingState::Bound(bound) = alternative else {
                    return None;
                };
                bound.value.mutable_namespace()
            })
            .collect::<Vec<_>>();
        for namespace in &namespaces {
            self.module_effects.contaminate_namespace(*namespace);
        }
        let aliases = self
            .bindings
            .iter()
            .filter(|&(_candidate, binding)| {
                binding.alternatives().any(|alternative| {
                    let PythonBindingState::Bound(bound) = alternative else {
                        return false;
                    };
                    bound
                        .value
                        .mutable_namespace()
                        .is_some_and(|candidate| namespaces.contains(&candidate))
                })
            })
            .map(|(candidate, _binding)| candidate.clone())
            .collect::<BTreeSet<_>>();
        if !aliases.is_empty() {
            self.degrade_names(aliases, &PythonUnknownCause::UnsupportedMutation, origin);
        }
    }

    pub(super) fn degrade_unsupported_mutation_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        origin: Origin,
    ) {
        let mut names = names.into_iter().collect::<BTreeSet<_>>();
        for name in names.clone() {
            names.extend(self.intrinsic_write_names(&name));
        }
        self.degrade_names(names, &PythonUnknownCause::UnsupportedMutation, origin);
    }

    pub(super) fn degrade_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        cause: &PythonUnknownCause,
        origin: Origin,
    ) {
        let mut names = names.into_iter().collect::<BTreeSet<_>>();
        for name in names.clone() {
            if let Some(binding) = self.binding(&name) {
                names.extend(self.mutable_alias_names(binding));
            }
        }
        for name in names {
            let mut unknown = PythonBinding::unknown(cause, origin);
            unknown.discriminate_predicates(&name, origin, self.predicate_module.as_ref());
            let binding = match self.bindings.remove(&name) {
                Some(binding) => binding.join(unknown, origin),
                None => unknown,
            };
            self.bindings.insert(name, binding);
        }
    }

    fn degrade_loop_effects(mut self, evaluated_bodies: Vec<Self>, origin: Origin) -> Self {
        let changed_names = evaluated_bodies
            .iter()
            .flat_map(|body| body.changed_names_from(&self))
            .collect::<BTreeSet<_>>();

        let mut body_objects = Vec::with_capacity(evaluated_bodies.len());
        for body in evaluated_bodies {
            let Self {
                bindings: _,
                namespace_causes,
                mutations,
                import_trace,
                module_effects,
                predicate_module: _,
            } = body;
            self.namespace_causes.extend(namespace_causes);
            self.mutations.extend(mutations);
            self.import_trace.absorb(&import_trace);
            body_objects.push(module_effects);
        }
        self.module_effects.degrade_loop(body_objects, origin);

        self.degrade_names(
            changed_names,
            &PythonUnknownCause::UnsupportedExpression,
            origin,
        );
        self
    }

    fn join_indexed_branches(
        mut base: Self,
        branches: &[(usize, Self)],
        join: &BranchJoin,
    ) -> Self {
        let origin = join.origin();
        let names = branches
            .iter()
            .flat_map(|(_, branch)| branch.changed_names_from(&base))
            .collect::<BTreeSet<_>>();
        for name in names {
            let mut groups: Vec<(PythonBinding, Vec<usize>)> = Vec::new();
            for (arm, branch) in branches {
                let candidate = branch
                    .binding(&name)
                    .cloned()
                    .unwrap_or_else(PythonBinding::unbound);
                if let Some((_, arms)) =
                    groups.iter_mut().find(|(grouped, _)| *grouped == candidate)
                {
                    arms.push(*arm);
                } else {
                    groups.push((candidate, vec![*arm]));
                }
            }
            let mut grouped = groups
                .iter()
                .map(|(candidate, _)| candidate.clone())
                .collect::<Vec<_>>();
            let joined = if PythonBinding::supports_grouped_branch_join(&grouped) {
                for (candidate, (_, arms)) in grouped.iter_mut().zip(&groups) {
                    candidate.select_branches(join.to_owned(), arms);
                }
                PythonBinding::join_grouped_branches(grouped, origin)
            } else {
                branches
                    .iter()
                    .map(|(arm, branch)| {
                        let mut candidate = branch
                            .binding(&name)
                            .cloned()
                            .unwrap_or_else(PythonBinding::unbound);
                        candidate.select_branch(join.to_owned(), *arm);
                        candidate
                    })
                    .reduce(|current, candidate| current.join(candidate, origin))
                    .unwrap_or_else(PythonBinding::unbound)
            };
            base.bindings.insert(name, joined);
        }
        base.namespace_causes.clear();
        base.mutations.clear();
        base.import_trace = PythonImportTrace::default();
        for (arm, branch) in branches {
            base.namespace_causes
                .extend(branch.namespace_causes.iter().cloned().map(|mut cause| {
                    cause.select_branch(join.to_owned(), *arm);
                    cause
                }));
            base.mutations.extend(branch.mutations.iter().cloned());
            base.import_trace.absorb(&branch.import_trace);
        }
        let branch_effects = branches
            .iter()
            .map(|(arm, branch)| (*arm, &branch.module_effects))
            .collect::<Vec<_>>();
        base.module_effects = PythonModuleEffects::join_indexed_branches(&branch_effects, join);
        base
    }

    fn join_guarded_branches(
        mut base: Self,
        branches: &[(usize, BranchConstraints, Self)],
        join: &BranchJoin,
    ) -> Self {
        let origin = join.origin();
        let names = branches
            .iter()
            .flat_map(|(_, _, branch)| branch.changed_names_from(&base))
            .collect::<BTreeSet<_>>();
        for name in names {
            let mut joined: Option<PythonBinding> = None;
            for (arm, constraints, branch) in branches {
                let mut candidate = branch
                    .binding(&name)
                    .cloned()
                    .unwrap_or_else(PythonBinding::unbound);
                candidate.select_branch(join.to_owned(), *arm);
                let candidate = candidate.intersect_constraints(constraints);
                let Some(candidate) = candidate else {
                    continue;
                };
                joined = Some(match joined {
                    Some(current) => current.join(candidate, origin),
                    None => candidate,
                });
            }
            if let Some(binding) = joined {
                base.bindings.insert(name, binding);
            }
        }

        base.namespace_causes.clear();
        base.mutations.clear();
        base.import_trace = PythonImportTrace::default();
        for (arm, constraints, branch) in branches {
            base.namespace_causes
                .extend(branch.namespace_causes.iter().filter_map(|cause| {
                    let mut cause = cause.clone();
                    cause.select_branch(join.to_owned(), *arm);
                    cause.constraints = cause.constraints.intersection(constraints);
                    (!cause.constraints.is_impossible()).then_some(cause)
                }));
            base.mutations.extend(branch.mutations.iter().cloned());
            base.import_trace.absorb(&branch.import_trace);
        }
        let branch_effects = branches
            .iter()
            .map(|(arm, constraints, branch)| (*arm, constraints, &branch.module_effects))
            .collect::<Vec<_>>();
        base.module_effects = PythonModuleEffects::join_guarded_branches(&branch_effects, join);
        base
    }

    #[cfg(test)]
    fn join_branches(base: Self, branches: &[Self], origin: Origin) -> Self {
        let indexed = branches.iter().cloned().enumerate().collect::<Vec<_>>();
        let join = origin.into();
        Self::join_indexed_branches(base, &indexed, &join)
    }

    fn changed_names_from(&self, base: &Self) -> BTreeSet<String> {
        let mut changed = base
            .bindings
            .keys()
            .chain(self.bindings.keys())
            .filter(|name| base.binding(name) != self.binding(name))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mutation_roots = base
            .mutations
            .iter()
            .chain(&self.mutations)
            .map(|mutation| mutation.binding.as_str())
            .collect::<BTreeSet<_>>();
        for name in mutation_roots {
            if !base
                .rooted_mutation_evidence(name)
                .eq(self.rooted_mutation_evidence(name))
            {
                changed.insert(name.to_string());
            }
        }
        changed
    }

    fn rooted_mutation_evidence<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<
        Item = (
            &'a [PythonMutationPathSegment],
            PythonMutationOperation,
            Origin,
        ),
    > + 'a {
        self.mutations
            .iter()
            .filter(move |mutation| mutation.binding == name)
            .map(|mutation| {
                (
                    mutation.path.as_slice(),
                    mutation.operation,
                    mutation.origin,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io;
    use std::rc::Rc;
    use std::sync::Arc;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_conf::Settings;
    use djls_source::CaseSensitivity;
    use djls_source::Db as _;
    use djls_source::File;
    use djls_source::FileReadErrorKind;
    use djls_source::FileSystem;
    use djls_source::InMemoryFileSystem;
    use djls_source::RootWalk;
    use djls_source::Span;
    use djls_source::WalkOptions;
    use djls_testing::OsTestDatabase;
    use djls_testing::TestDatabase;
    use salsa::Setter;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::Origin;
    use super::PythonBinding;
    use super::PythonBindingState;
    use super::PythonEvaluationState;
    use super::PythonImportOutcome;
    use super::PythonIntrinsic;
    use super::PythonModuleEvaluator;
    use super::PythonModuleFacts;
    use super::PythonMutation;
    use super::PythonMutationOperation;
    use super::PythonMutationPath;
    use super::PythonMutationPathSegment;
    use super::PythonNamespaceCause;
    use super::PythonUnknown;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use super::PythonValueKind;
    use super::Truthiness;
    use crate::python::PythonModule;
    use crate::python::PythonModuleName;
    use crate::python::PythonNamespacePackage;
    use crate::python::evaluation::EvaluationDemand;
    use crate::python::evaluation::PythonBoundValue;
    use crate::python::evaluation::PythonImportEvaluationStatus;
    use crate::python::evaluation::PythonSequenceItem;
    use crate::python::evaluation::python_import_trace;
    use crate::python::evaluation::python_module_facts;
    use crate::python::module::PythonImportNameError;

    #[salsa::db]
    impl crate::db::Db for OsTestDatabase {
        fn project(&self) -> Option<crate::project::Project> {
            None
        }
    }

    struct ReadFailingFileSystem {
        inner: InMemoryFileSystem,
        unreadable: Utf8PathBuf,
    }

    impl FileSystem for ReadFailingFileSystem {
        fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
            if path == self.unreadable {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "test file is unreadable",
                ));
            }
            self.inner.read_to_string(path)
        }

        fn exists(&self, path: &Utf8Path) -> bool {
            self.inner.exists(path)
        }
        fn is_file(&self, path: &Utf8Path) -> bool {
            self.inner.is_file(path)
        }
        fn is_dir(&self, path: &Utf8Path) -> bool {
            self.inner.is_dir(path)
        }
        fn case_sensitivity(&self) -> CaseSensitivity {
            self.inner.case_sensitivity()
        }
        fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
            self.inner.path_exists_case_sensitive(path, prefix)
        }
        fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
            self.inner.walk_root(root, options)
        }
    }

    fn evaluate_unreadable_module(
        files: &[(&str, &str)],
        unreadable: &str,
        source: &str,
    ) -> (
        OsTestDatabase,
        File,
        PythonModuleFacts,
        super::PythonImportTrace,
    ) {
        let mut inner = InMemoryFileSystem::new();
        for (path, content) in files {
            inner.add_file((*path).into(), (*content).to_string());
        }
        inner.add_file("/project/settings.py".into(), source.to_string());
        let fs = ReadFailingFileSystem {
            inner,
            unreadable: unreadable.into(),
        };
        let mut db = OsTestDatabase::with_file_system(Arc::new(fs), ["/project".into()]);
        let project =
            crate::project::Project::initial(&db, Utf8Path::new("/project"), &Settings::default());
        let search_paths = crate::python::SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &crate::python::PythonEnvironment::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        project.set_search_paths(&mut db).to(search_paths);
        let module = crate::python::file_to_module(&db, project, "/project/settings.py".into())
            .expect("module resolves");
        let facts = python_module_facts(&db, project, module.clone(), EvaluationDemand::Full)
            .as_ref()
            .expect("module evaluates")
            .clone();
        let trace =
            python_import_trace(&db, project, module.clone(), EvaluationDemand::Full).clone();
        (db, module.file(), facts, trace)
    }

    fn evaluate_source(source: &str) -> PythonModuleFacts {
        evaluate_source_with_file(source).1
    }

    fn evaluate_source_with_file(source: &str) -> (File, PythonModuleFacts) {
        let db = TestDatabase::new();
        let project =
            crate::project::Project::initial(&db, Utf8Path::new("/project"), &Settings::default());
        db.add_file("/project/settings.py", source)
            .expect("fixture should be writable");
        let module = crate::python::file_to_module(&db, project, "/project/settings.py".into())
            .expect("module should resolve");
        let file = module.file();
        let parsed = ruff_python_parser::parse_module(source)
            .expect("fixture should parse")
            .into_syntax();
        let facts = PythonModuleEvaluator::new(
            &db,
            project,
            module,
            super::IntrinsicContamination::default(),
        )
        .evaluate(&parsed.body, Vec::new(), Vec::new())
        .0;
        (file, facts)
    }

    fn evaluate_imports(
        files: &[(&str, &str)],
        source: &str,
    ) -> (File, PythonModuleFacts, super::PythonImportTrace) {
        evaluate_imports_at(files, "/project/settings.py", source)
    }

    fn evaluate_imports_at(
        files: &[(&str, &str)],
        entry: &str,
        source: &str,
    ) -> (File, PythonModuleFacts, super::PythonImportTrace) {
        let (_, file, facts, trace) = evaluate_imports_with_paths(files, entry, source, &[]);
        (file, facts, trace)
    }

    fn evaluation_project(
        files: &[(&str, &str)],
        pythonpath: &[Utf8PathBuf],
    ) -> (TestDatabase, crate::project::Project) {
        let mut db = TestDatabase::new();
        for (path, content) in files {
            db.add_file(path, content)
                .expect("fixture should be writable");
        }
        let project =
            crate::project::Project::initial(&db, Utf8Path::new("/project"), &Settings::default());
        let paths = crate::python::SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &crate::python::PythonEnvironment::Auto,
            pythonpath,
        );
        paths.register_roots(&db);
        project.set_search_paths(&mut db).to(paths);
        (db, project)
    }

    fn cycle_products(
        files: &[(&str, &str)],
        first: &str,
        roots: &[&str],
    ) -> Vec<(PythonModuleFacts, super::PythonImportTrace)> {
        let (db, project) = evaluation_project(files, &[]);
        for (path, _) in files {
            db.file(Utf8Path::new(path)).expect("fixture file exists");
        }
        let resolve = |name: &str| {
            crate::python::PythonSourceModule::resolve(
                &db,
                project,
                PythonModuleName::parse(name).expect("valid module name"),
            )
            .expect("module resolves")
        };
        python_module_facts(&db, project, resolve(first), EvaluationDemand::Full)
            .as_ref()
            .expect("evaluation succeeds");
        roots
            .iter()
            .map(|name| {
                let module = resolve(name);
                let facts =
                    python_module_facts(&db, project, module.clone(), EvaluationDemand::Full)
                        .as_ref()
                        .expect("evaluation succeeds")
                        .clone();
                let trace =
                    python_import_trace(&db, project, module, EvaluationDemand::Full).clone();
                (facts, trace)
            })
            .collect()
    }

    fn assert_cycle_products_equal(
        left: &[(PythonModuleFacts, super::PythonImportTrace)],
        right: &[(PythonModuleFacts, super::PythonImportTrace)],
    ) {
        assert_eq!(left.len(), right.len());
        for ((left, left_trace), (right, right_trace)) in left.iter().zip(right) {
            assert_eq!(
                left.bindings.keys().collect::<Vec<_>>(),
                right.bindings.keys().collect::<Vec<_>>()
            );
            for (name, binding) in &left.bindings {
                assert_eq!(
                    binding.alternatives().collect::<Vec<_>>(),
                    right.bindings[name].alternatives().collect::<Vec<_>>()
                );
            }
            assert_eq!(left.namespace_remainder, right.namespace_remainder);
            assert_eq!(left.syntax_errors, right.syntax_errors);
            assert_eq!(left.mutations, right.mutations);
            assert_eq!(left_trace, right_trace);
        }
    }

    fn evaluate_imports_with_paths(
        files: &[(&str, &str)],
        entry: &str,
        source: &str,
        pythonpath: &[camino::Utf8PathBuf],
    ) -> (
        TestDatabase,
        File,
        PythonModuleFacts,
        super::PythonImportTrace,
    ) {
        let mut db = TestDatabase::new();
        for (path, content) in files {
            db.add_file(path, content)
                .expect("fixture should be writable");
        }
        db.add_file(entry, source)
            .expect("fixture should be writable");
        let project =
            crate::project::Project::initial(&db, Utf8Path::new("/project"), &Settings::default());
        let search_paths = crate::python::SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &crate::python::PythonEnvironment::Auto,
            pythonpath,
        );
        search_paths.register_roots(&db);
        project.set_search_paths(&mut db).to(search_paths);
        let module = crate::python::file_to_module(&db, project, entry.into())
            .expect("module should resolve");
        let facts = python_module_facts(&db, project, module.clone(), EvaluationDemand::Full)
            .as_ref()
            .expect("module should evaluate")
            .clone();
        let trace =
            python_import_trace(&db, project, module.clone(), EvaluationDemand::Full).clone();
        (db, module.file(), facts, trace)
    }

    fn bound<'a>(facts: &'a PythonModuleFacts, name: &str) -> &'a PythonBoundValue {
        let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
        let [PythonBindingState::Bound(value)] = alternatives.as_slice() else {
            panic!("expected one bound alternative for {name}")
        };
        value
    }

    fn module_name<'a>(facts: &'a PythonModuleFacts, name: &str) -> (&'a str, bool) {
        let PythonValueKind::Module(module) = &bound(facts, name).value.kind else {
            panic!("expected a module for {name}")
        };
        (
            module.name().as_str(),
            matches!(module, PythonModule::Namespace(_)),
        )
    }

    fn import_names(trace: &super::PythonImportTrace) -> Vec<String> {
        trace
            .imports()
            .map(|outcome| match outcome {
                PythonImportOutcome::Evaluated {
                    edge,
                    status: PythonImportEvaluationStatus::Resolved,
                } => format!("resolved:{}", edge.imported.name().as_str()),
                PythonImportOutcome::Evaluated {
                    edge,
                    status: PythonImportEvaluationStatus::Cycle { .. },
                } => format!("cycle:{}", edge.imported.name().as_str()),
                PythonImportOutcome::NotFound { module, .. } => {
                    format!("notfound:{}", module.as_str())
                }
                PythonImportOutcome::SkippedExternal { module, .. } => {
                    format!("external:{}", module.as_str())
                }
                other @ (PythonImportOutcome::Evaluated { .. }
                | PythonImportOutcome::InvalidImport { .. }
                | PythonImportOutcome::Unreadable { .. }) => {
                    panic!("unexpected import outcome: {other:?}")
                }
            })
            .collect()
    }

    fn binding_strings<'a>(
        facts: &'a PythonModuleFacts,
        name: &str,
    ) -> std::collections::BTreeSet<&'a str> {
        facts.bindings[name]
            .alternatives()
            .filter_map(|state| {
                let PythonBindingState::Bound(bound) = state else {
                    return None;
                };
                let PythonValueKind::Str(value) = &bound.value.kind else {
                    return None;
                };
                Some(value.as_str())
            })
            .collect()
    }

    fn nested_tuple_list(value: &PythonValue) -> &PythonValue {
        let PythonValueKind::Tuple(tuple) = &value.kind else {
            panic!("expected tuple")
        };
        let [PythonSequenceItem::Value(nested)] = tuple.semantic_items() else {
            panic!("expected one tuple item")
        };
        nested
    }

    fn string_items(value: &PythonValue) -> Vec<&str> {
        let PythonValueKind::List(list) = &value.kind else {
            panic!("expected list")
        };
        list.semantic_items()
            .iter()
            .map(|item| {
                let PythonSequenceItem::Value(value) = item else {
                    panic!("expected value")
                };
                let PythonValueKind::Str(text) = &value.kind else {
                    panic!("expected string")
                };
                text.as_str()
            })
            .collect()
    }

    fn sequence_items(value: &PythonValue) -> Option<Vec<String>> {
        let items = match &value.kind {
            PythonValueKind::List(items) => items.semantic_items(),
            PythonValueKind::Tuple(items) => items.semantic_items(),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Intrinsic(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => return None,
        };
        Some(
            items
                .iter()
                .map(|item| match item {
                    PythonSequenceItem::Value(PythonValue {
                        kind: PythonValueKind::Str(text),
                        ..
                    }) => format!("str:{text}"),
                    PythonSequenceItem::Value(_) => "value".to_owned(),
                    PythonSequenceItem::UnknownElement(_) => "element".to_owned(),
                    PythonSequenceItem::UnknownUnpack(_) => "unpack".to_owned(),
                })
                .collect(),
        )
    }

    #[test]
    fn env_reads_keep_file_candidates_and_runtime_uncertainty() {
        let source = "import os\nfrom os import environ as imported_environ, getenv as imported_getenv\nENVIRON = os.environ\nENV_GET = os.environ.get\nGETENV = os.getenv\nPRESENT = os.environ.get('APP')\nALIASED = ENV_GET('APP')\nIMPORTED_GET = imported_environ.get('APP')\nIMPORTED_GETENV = imported_getenv('APP')\nDEFAULTED = GETENV('MISSING', 'fallback')\nGET_DEFAULTED = os.environ.get('MISSING', 'fallback')\nABSENT = os.environ.get('ABSENT')\nGETENV_ABSENT = os.getenv('ABSENT')\nDYNAMIC_KEY = os.getenv(KEY)\nDYNAMIC_DEFAULT = os.getenv('MISSING', fallback())\nKEYWORD_DEFAULT = os.getenv('MISSING', default='fallback')\nSUBSCRIPT = os.environ['APP']\nos.getenv('APP')\nAFTER_DISCARDED = os.getenv('APP')\nINSTALLED_APPS = [os.getenv('APP', 'fallback')]\n";
        let db = TestDatabase::new();
        for (path, content) in [
            ("/project/.env", "APP=stale.app\nAPP=env.app\n"),
            ("/project/manage.py", ""),
            ("/project/config/__init__.py", ""),
            ("/project/config/settings.py", source),
        ] {
            db.add_file(path, content)
                .expect("fixture should be writable");
        }
        let settings: Settings =
            toml::from_str("django_settings_module = 'config.settings'\n").expect("config parses");
        let project = crate::project::Project::bootstrap(&db, Utf8Path::new("/project"), &settings);
        let module =
            crate::python::file_to_module(&db, project, "/project/config/settings.py".into())
                .expect("module resolves");
        let facts = python_module_facts(&db, project, module, EvaluationDemand::Full);
        let facts = facts.as_ref().expect("evaluation succeeds");
        for (name, expected) in [
            ("ENVIRON", PythonIntrinsic::OsEnvironObject),
            ("ENV_GET", PythonIntrinsic::OsEnvironGetFunction),
            ("GETENV", PythonIntrinsic::OsGetenvFunction),
            ("imported_environ", PythonIntrinsic::OsEnvironObject),
            ("imported_getenv", PythonIntrinsic::OsGetenvFunction),
        ] {
            assert_eq!(
                bound(facts, name).value.kind,
                PythonValueKind::Intrinsic(expected)
            );
        }
        let assert_env_read = |name: &str, expected: PythonValueKind, key: &str| {
            let binding = &facts.bindings[name];
            assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if bound.value.kind == expected)), "{name}: {binding:#?}");
            assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::EnvValueUnknown { key: key.to_owned() }))), "{name}: {binding:#?}");
        };
        for name in [
            "PRESENT",
            "ALIASED",
            "IMPORTED_GET",
            "IMPORTED_GETENV",
            "AFTER_DISCARDED",
        ] {
            assert_env_read(name, PythonValueKind::Str("env.app".to_owned()), "APP");
        }
        for name in ["DEFAULTED", "GET_DEFAULTED"] {
            assert_env_read(name, PythonValueKind::Str("fallback".to_owned()), "MISSING");
        }
        for name in ["ABSENT", "GETENV_ABSENT"] {
            assert_env_read(name, PythonValueKind::UnsupportedLiteral, "ABSENT");
        }
        for name in [
            "DYNAMIC_KEY",
            "DYNAMIC_DEFAULT",
            "KEYWORD_DEFAULT",
            "SUBSCRIPT",
        ] {
            assert!(
                matches!(&bound(facts, name).value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression)
            );
        }
    }

    #[test]
    fn named_import_of_absent_open_name_preserves_member_and_namespace_uncertainty() {
        let source = "from plugin import INSTALLED_APPS\n";
        let (db, file, facts, trace) = evaluate_imports_with_paths(
            &[(
                "/project/plugin.py",
                "if ENABLED:\n    from missing import *\n",
            )],
            "/project/settings.py",
            source,
            &[],
        );
        let plugin = db
            .file(Utf8Path::new("/project/plugin.py"))
            .expect("plugin exists");
        let binding = &facts.bindings["INSTALLED_APPS"];
        assert!(
            !binding
                .alternatives()
                .any(|state| matches!(state, PythonBindingState::Unbound))
        );
        let text = "INSTALLED_APPS";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "plugin" && member == "INSTALLED_APPS") && unknown.origins().collect::<Vec<_>>() == [origin]) && bound.binding_origins().collect::<Vec<_>>() == [origin])));
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing") && unknown.origins().collect::<Vec<_>>() == [origin]) && bound.binding_origins().collect::<Vec<_>>() == [origin])));
        assert_eq!(trace.imports().filter(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.file() == plugin)).count(), 1);
        assert_eq!(trace.files().filter(|file| *file == plugin).count(), 1);
    }

    #[test]
    fn named_import_of_conditional_binding_preserves_known_member_and_namespace_outcomes() {
        let (db, _, facts, trace) = evaluate_imports_with_paths(
            &[(
                "/project/plugin.py",
                "if ENABLED:\n    INSTALLED_APPS = ['imported']\n    from missing import *\n",
            )],
            "/project/settings.py",
            "from plugin import INSTALLED_APPS\n",
            &[],
        );
        let plugin = db
            .file(Utf8Path::new("/project/plugin.py"))
            .expect("plugin exists");
        let binding = &facts.bindings["INSTALLED_APPS"];
        assert!(
            !binding
                .alternatives()
                .any(|state| matches!(state, PythonBindingState::Unbound))
        );
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::List(items) if matches!(items.semantic_items(), [PythonSequenceItem::Value(value)] if matches!(&value.kind, PythonValueKind::Str(text) if text == "imported"))))));
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "plugin" && member == "INSTALLED_APPS")))));
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing")))));
        assert_eq!(trace.imports().filter(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.file() == plugin)).count(), 1);
        assert_eq!(trace.files().filter(|file| *file == plugin).count(), 1);
    }

    #[test]
    fn mixed_direct_and_from_cycle_records_cycle_on_both_forms() {
        let files = [
            ("/project/a.py", "import b\nVALUE_A = 'a'\n"),
            (
                "/project/b.py",
                "from a import VALUE_A\nVALUE_B = VALUE_A\n",
            ),
        ];
        let from_a = cycle_products(&files, "a", &["a"]);
        let from_b = cycle_products(&files, "b", &["b"]);
        let edges = |trace: &super::PythonImportTrace| {
            trace
                .imports()
                .filter_map(|outcome| {
                    let PythonImportOutcome::Evaluated {
                        edge,
                        status:
                            PythonImportEvaluationStatus::Resolved
                            | PythonImportEvaluationStatus::Cycle { .. },
                    } = outcome
                    else {
                        return None;
                    };
                    Some((
                        edge.importer.name().as_str().to_owned(),
                        edge.imported.name().as_str().to_owned(),
                    ))
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        for (_, trace) in [&from_a[0], &from_b[0]] {
            assert_eq!(
                trace
                    .imports()
                    .filter(|outcome| matches!(
                        outcome,
                        PythonImportOutcome::Evaluated {
                            status: PythonImportEvaluationStatus::Cycle { .. },
                            ..
                        }
                    ))
                    .count(),
                1
            );
        }
        assert_eq!(edges(&from_a[0].1), edges(&from_b[0].1));
        let recorded = edges(&from_a[0].1);
        assert!(recorded.contains(&("a".to_owned(), "b".to_owned())));
        assert!(recorded.contains(&("b".to_owned(), "a".to_owned())));
        assert!(from_a[0].0.bindings["b"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "b"))));
    }

    #[test]
    fn external_star_import_of_cycle_is_entry_order_independent() {
        let files = [
            ("/project/a.py", "from b import *\n"),
            ("/project/b.py", "from a import *\n"),
            ("/project/external.py", "from a import *\n"),
        ];
        let a_first = cycle_products(&files, "a", &["external"]);
        let b_first = cycle_products(&files, "b", &["external"]);
        assert_cycle_products_equal(&a_first, &b_first);
        for (facts, trace) in [&a_first[0], &b_first[0]] {
            let external = trace.files().next().expect("external file exists");
            let origin = Origin::new(
                external,
                Span::saturating_from_parts_usize(0, "from a import *".len()),
            );
            assert!(
                matches!(facts.namespace_remainder.as_ref().expect("namespace open").as_slice(), [cause] if cause.unknown.cause == PythonUnknownCause::Cycle && cause.unknown.origins().collect::<Vec<_>>() == [origin])
            );
        }
    }

    #[test]
    fn python_module_cycle_keeps_local_mutations_without_copying_dynamic_star_mutations() {
        let files = [
            (
                "/project/a.py",
                "from b import *\nVALUES_A = []\nVALUES_A.append('a')\n",
            ),
            (
                "/project/b.py",
                "from a import *\nVALUES_B = []\nVALUES_B.append('b')\n",
            ),
        ];
        let a_first = cycle_products(&files, "a", &["a", "b"]);
        let b_first = cycle_products(&files, "b", &["a", "b"]);
        assert_cycle_products_equal(&a_first, &b_first);
        for ((facts, _), local, imported) in [
            (&a_first[0], "VALUES_A", "VALUES_B"),
            (&a_first[1], "VALUES_B", "VALUES_A"),
        ] {
            assert!(
                facts
                    .mutations
                    .iter()
                    .any(|mutation| mutation.binding == local)
            );
            assert!(
                facts
                    .mutations
                    .iter()
                    .all(|mutation| mutation.binding != imported),
                "a dynamic cycle alternative cannot select an imported mutation"
            );
        }
    }

    #[test]
    fn ordinary_import_cycle_products_are_entry_order_independent() {
        let files = [
            ("/project/a.py", "import b\nVALUE_A = 'a'\n"),
            ("/project/b.py", "import a\nVALUE_B = 'b'\n"),
        ];
        let a_first = cycle_products(&files, "a", &["a", "b"]);
        let b_first = cycle_products(&files, "b", &["a", "b"]);
        assert_cycle_products_equal(&a_first, &b_first);
        for (_, trace) in &a_first {
            assert_eq!(
                trace
                    .imports()
                    .filter(|outcome| matches!(
                        outcome,
                        PythonImportOutcome::Evaluated {
                            status: PythonImportEvaluationStatus::Cycle { .. },
                            ..
                        }
                    ))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn python_module_cycle_products_are_entry_order_independent() {
        let files = [
            ("/project/a.py", "from b import B\nA = B\nAFTER_A = 'a'\n"),
            ("/project/b.py", "from a import A\nB = A\nAFTER_B = 'b'\n"),
        ];
        let a_first = cycle_products(&files, "a", &["a", "b"]);
        let b_first = cycle_products(&files, "b", &["a", "b"]);
        assert_cycle_products_equal(&a_first, &b_first);
        for (_, trace) in &a_first {
            assert_eq!(
                trace
                    .imports()
                    .filter(|outcome| matches!(
                        outcome,
                        PythonImportOutcome::Evaluated {
                            status: PythonImportEvaluationStatus::Cycle { .. },
                            ..
                        }
                    ))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn python_module_cycle_widens_cyclic_values_but_keeps_post_cycle_assignments() {
        let files = [
            ("/project/a.py", "from b import B\nA = B\nAFTER_A = 'a'\n"),
            ("/project/b.py", "from a import A\nB = A\nAFTER_B = 'b'\n"),
        ];
        let products = cycle_products(&files, "a", &["a", "b"]);
        for ((facts, _), cycle_name, stable_name, stable_value) in [
            (&products[0], "A", "AFTER_A", "a"),
            (&products[1], "B", "AFTER_B", "b"),
        ] {
            assert!(
                matches!(&bound(facts, cycle_name).value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::Cycle)
            );
            assert!(
                matches!(&bound(facts, stable_name).value.kind, PythonValueKind::Str(value) if value == stable_value)
            );
        }
    }

    #[test]
    fn relative_import_cycle_uses_module_identities_in_overlapping_roots() {
        let (db, project) = evaluation_project(
            &[
                ("/project/lib/pkg/a.py", "from .b import B\nA = B\n"),
                ("/project/lib/pkg/b.py", "from lib.pkg.a import A\nB = A\n"),
            ],
            &[Utf8PathBuf::from("/project/lib")],
        );
        let module = crate::python::PythonSourceModule::resolve(
            &db,
            project,
            PythonModuleName::parse("pkg.a").expect("valid name"),
        )
        .expect("module resolves");
        let trace = python_import_trace(&db, project, module, EvaluationDemand::Full);
        let cycles = trace
            .imports()
            .filter_map(|outcome| {
                let PythonImportOutcome::Evaluated {
                    edge,
                    status: PythonImportEvaluationStatus::Cycle { .. },
                } = outcome
                else {
                    return None;
                };
                Some((edge.importer.name().as_str(), edge.imported.name().as_str()))
            })
            .collect::<Vec<_>>();
        assert_eq!(cycles, [("lib.pkg.a", "lib.pkg.b")]);
    }

    #[test]
    fn relative_import_uses_the_inbound_module_identity() {
        let (db, project) = evaluation_project(
            &[
                (
                    "/project/lib/pkg/settings.py",
                    "from .sibling import VALUE\n",
                ),
                (
                    "/project/pkg/sibling.py",
                    "VALUE = 'inbound module identity'\n",
                ),
                (
                    "/project/lib/pkg/sibling.py",
                    "VALUE = 'canonical file identity'\n",
                ),
            ],
            &[Utf8PathBuf::from("/project/lib")],
        );
        let module = crate::python::PythonSourceModule::resolve(
            &db,
            project,
            PythonModuleName::parse("pkg.settings").expect("valid name"),
        )
        .expect("module resolves");
        let sibling = db
            .file(Utf8Path::new("/project/pkg/sibling.py"))
            .expect("file exists");
        let trace = python_import_trace(&db, project, module.clone(), EvaluationDemand::Full);
        assert!(
            matches!(trace.imports().collect::<Vec<_>>().as_slice(), [PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved }] if edge.imported.file() == sibling)
        );
        let facts = python_module_facts(&db, project, module.clone(), EvaluationDemand::Full);
        assert!(
            matches!(&bound(facts.as_ref().expect("evaluation succeeds"), "VALUE").value.kind, PythonValueKind::Str(value) if value == "inbound module identity")
        );
        let canonical =
            crate::python::file_to_module(&db, project, "/project/lib/pkg/settings.py".into())
                .expect("canonical module resolves");
        let facts = python_module_facts(&db, project, canonical, EvaluationDemand::Full);
        assert!(
            matches!(&bound(facts.as_ref().expect("evaluation succeeds"), "VALUE").value.kind, PythonValueKind::Str(value) if value == "canonical file identity")
        );
    }

    #[test]
    fn python_module_package_identity_relative_import_from_init_alias_uses_parent_package() {
        let (db, project) = evaluation_project(
            &[
                ("/project/pkg/__init__.py", "from .base import VALUE\n"),
                ("/project/pkg/base.py", "VALUE = 'package value'\n"),
            ],
            &[],
        );
        let module = crate::python::PythonSourceModule::resolve(
            &db,
            project,
            PythonModuleName::parse("pkg.__init__").expect("valid name"),
        )
        .expect("module resolves");
        let facts = python_module_facts(&db, project, module, EvaluationDemand::Full);
        assert!(
            matches!(&bound(facts.as_ref().expect("evaluation succeeds"), "VALUE").value.kind, PythonValueKind::Str(value) if value == "package value")
        );
    }

    #[test]
    fn canonical_unknown_origins_merge_dynamic_namespace_import_edges() {
        let source = "from a import *\nfrom b import *\n";
        let (file, facts, _) = evaluate_imports(
            &[
                ("/project/a.py", "from b import *\n"),
                ("/project/b.py", "from a import *\n"),
            ],
            source,
        );
        let expected = ["from a import *", "from b import *"].map(|text| {
            Origin::new(
                file,
                Span::saturating_from_parts_usize(
                    source.find(text).expect("text exists"),
                    text.len(),
                ),
            )
        });
        let [cause] = facts
            .namespace_remainder
            .as_ref()
            .expect("namespace open")
            .as_slice()
        else {
            panic!("expected one namespace cause")
        };
        assert_eq!(cause.unknown.cause, PythonUnknownCause::Cycle);
        assert_eq!(cause.unknown.origins().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn canonical_unknown_origins_import_rebase_is_exactly_local() {
        let source = "from a import *\n";
        let (file, facts, _) = evaluate_imports(
            &[
                ("/project/a.py", "from b import INSTALLED_APPS\n"),
                ("/project/b.py", "from a import INSTALLED_APPS\n"),
            ],
            source,
        );
        let text = "from a import *";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        let binding = &facts.bindings["INSTALLED_APPS"];
        assert!(
            binding
                .alternatives()
                .any(|state| matches!(state, PythonBindingState::Unbound)),
            "dynamic star selection cannot guarantee the cyclic name is exported"
        );
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound)
            if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::Cycle && unknown.origins().collect::<Vec<_>>() == [origin])
            && bound.value.origins().collect::<Vec<_>>() == [origin]
            && bound.binding_origins().collect::<Vec<_>>() == [origin] )));
    }

    #[test]
    fn straight_line_unsupported_mutation_discards_stale_installed_apps() {
        let source = "INSTALLED_APPS = ['stale']\nINSTALLED_APPS.clear()";
        let facts = evaluate_source(source);
        let PythonValueKind::Unknown(unknown) = &bound(&facts, "INSTALLED_APPS").value.kind else {
            panic!("expected unknown value")
        };
        assert_eq!(unknown.cause, PythonUnknownCause::UnsupportedMutation);
        let origin = unknown
            .origins()
            .next()
            .expect("unknown should retain an origin");
        assert_eq!(
            &source[origin.span.start_usize()..origin.span.end_usize()],
            "INSTALLED_APPS.clear()"
        );
    }

    #[test]
    fn branch_unknowns_retain_distinct_predicates_and_all_spans() {
        let source = "if FLAG:\n    VALUE = first()\nelse:\n    VALUE = second()\n";
        let facts = evaluate_source(source);
        let unknowns = facts.bindings["VALUE"]
            .alternatives()
            .filter_map(|state| {
                let PythonBindingState::Bound(bound) = state else {
                    return None;
                };
                let PythonValueKind::Unknown(unknown) = &bound.value.kind else {
                    return None;
                };
                Some(unknown)
            })
            .collect::<Vec<_>>();
        assert_eq!(unknowns.len(), 2);
        assert!(
            unknowns
                .iter()
                .all(|unknown| unknown.cause == PythonUnknownCause::UnsupportedExpression)
        );
        let span = |text: &str| {
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len())
        };
        assert_eq!(
            unknowns
                .iter()
                .flat_map(|unknown| unknown.origins().map(|origin| origin.span))
                .collect::<Vec<_>>(),
            [span("first()"), span("second()")]
        );
    }

    #[test]
    fn star_import_translates_every_typed_namespace_cause_to_the_import_site() {
        let source = "VALUE = 'local'\nfrom plugin import *\n";
        let (file, facts, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "if ENABLED:\n    from missing import *\nelse:\n    from ...invalid import *\n",
            )],
            source,
        );
        let unknowns = facts.bindings["VALUE"]
            .alternatives()
            .filter_map(|alternative| {
                let PythonBindingState::Bound(bound) = alternative else {
                    return None;
                };
                let PythonValueKind::Unknown(unknown) = &bound.value.kind else {
                    return None;
                };
                Some(unknown)
            })
            .collect::<Vec<_>>();
        assert_eq!(unknowns.len(), 2, "{facts:#?}");
        assert!(unknowns.iter().any(|unknown| matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing")));
        assert!(unknowns.iter().any(|unknown| unknown.cause
            == PythonUnknownCause::InvalidImport(PythonImportNameError::TooManyDots)));
        let text = "from plugin import *";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(
            unknowns
                .iter()
                .all(|unknown| unknown.origins().collect::<Vec<_>>() == [origin])
        );
    }

    #[test]
    fn branch_constraints_distinguish_overlapping_root_module_identities() {
        let (_, _, facts, _) = evaluate_imports_with_paths(
            &[(
                "/project/lib/pkg/branch.py",
                "if FLAG:\n    VALUE = 'left'\nelse:\n    VALUE = 'right'\n",
            )],
            "/project/settings.py",
            "from lib.pkg.branch import VALUE as ROOT_VALUE\nfrom pkg.branch import VALUE as NESTED_VALUE\nPAIR = (ROOT_VALUE, NESTED_VALUE)\n",
            &[Utf8PathBuf::from("/project/lib")],
        );
        let pairs = facts.bindings["PAIR"]
            .alternatives()
            .filter_map(|state| {
                let PythonBindingState::Bound(bound) = state else {
                    return None;
                };
                sequence_items(&bound.value)
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            pairs,
            [
                vec!["str:left".to_owned(), "str:left".to_owned()],
                vec!["str:left".to_owned(), "str:right".to_owned()],
                vec!["str:right".to_owned(), "str:left".to_owned()],
                vec!["str:right".to_owned(), "str:right".to_owned()],
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn predicates_distinguish_overlapping_root_module_executions() {
        let (_, _, facts, _) = evaluate_imports_with_paths(
            &[("/project/lib/pkg/branch.py", "FLAG = get_flag()\n")],
            "/project/settings.py",
            "from lib.pkg.branch import FLAG as ROOT_FLAG\nfrom pkg.branch import FLAG as NESTED_FLAG\nINSTALLED_APPS = []\nif ROOT_FLAG:\n    INSTALLED_APPS.append('root')\nif NESTED_FLAG:\n    INSTALLED_APPS.append('nested')\n",
            &[Utf8PathBuf::from("/project/lib")],
        );
        let alternatives = facts.bindings["INSTALLED_APPS"]
            .alternatives()
            .filter_map(|state| {
                let PythonBindingState::Bound(bound) = state else {
                    return None;
                };
                sequence_items(&bound.value)
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            alternatives,
            std::collections::BTreeSet::from([
                vec![],
                vec!["str:nested".to_owned()],
                vec!["str:root".to_owned()],
                vec!["str:root".to_owned(), "str:nested".to_owned()],
            ])
        );
    }

    #[test]
    fn missing_resolved_named_import_member_is_typed_dynamic_and_replaces_stale_binding() {
        let source = "INSTALLED_APPS = ['stale']\nfrom plugin import MISSING as INSTALLED_APPS\n";
        let (db, settings, facts, trace) = evaluate_imports_with_paths(
            &[
                ("/project/config/__init__.py", ""),
                ("/project/plugin.py", "PRESENT = 'known'\n"),
            ],
            "/project/config/settings.py",
            source,
            &[],
        );
        let plugin = db
            .file(Utf8Path::new("/project/plugin.py"))
            .expect("plugin exists");
        let imports = trace.imports().collect::<Vec<_>>();
        assert!(
            matches!(imports.as_slice(), [PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved }] if edge.imported.file() == plugin && edge.importer.name().as_str() == "config.settings" && edge.imported.name().as_str() == "plugin")
        );
        assert_eq!(trace.files().collect::<Vec<_>>(), [settings, plugin]);
        let text = "MISSING as INSTALLED_APPS";
        let origin = Origin::new(
            settings,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        let binding = bound(&facts, "INSTALLED_APPS");
        assert!(
            matches!(&binding.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "plugin" && member == "MISSING") && unknown.origins().collect::<Vec<_>>() == [origin])
        );
        assert_eq!(binding.value.origins().collect::<Vec<_>>(), [origin]);
        assert_eq!(binding.binding_origins().collect::<Vec<_>>(), [origin]);
    }

    #[test]
    fn python_module_evaluation_keeps_typed_import_and_namespace_outcomes() {
        let source = "from missing_named import VALUE\nfrom missing_star import *\n";
        let (settings, facts, trace) = evaluate_imports(&[], source);
        let not_found = trace
            .imports()
            .map(|outcome| {
                let PythonImportOutcome::NotFound { origin, module } = outcome else {
                    return None;
                };
                Some((origin.file, origin.span, module.as_str()))
            })
            .collect::<Option<Vec<_>>>()
            .expect("evaluation should contain only typed not-found imports");
        let span = |text: &str| {
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len())
        };
        assert_eq!(
            not_found,
            [
                (
                    settings,
                    span("from missing_named import VALUE"),
                    "missing_named"
                ),
                (settings, span("from missing_star import *"), "missing_star"),
            ]
        );
        assert!(facts.bindings["VALUE"].alternatives().any(|alternative| matches!(alternative,
            PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing_named")))));
        let namespace = facts
            .namespace_remainder
            .as_ref()
            .expect("namespace should be open");
        assert!(
            matches!(namespace.as_slice(), [cause] if matches!(&cause.unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing_star"))
        );
    }

    #[test]
    fn typed_module_order_disjoint_import_cycles_are_root_order_independent() {
        let cycle_edges = |source| {
            let (_, _, trace) = evaluate_imports(
                &[
                    ("/project/a.py", "from b import B\nA = B\n"),
                    ("/project/b.py", "from a import A\nB = A\n"),
                    ("/project/x.py", "from y import Y\nX = Y\n"),
                    ("/project/y.py", "from x import X\nY = X\n"),
                ],
                source,
            );
            trace
                .imports()
                .filter_map(|outcome| {
                    let PythonImportOutcome::Evaluated {
                        edge,
                        status: PythonImportEvaluationStatus::Cycle { .. },
                    } = outcome
                    else {
                        return None;
                    };
                    Some((
                        edge.importer.name().as_str().to_owned(),
                        edge.imported.name().as_str().to_owned(),
                    ))
                })
                .collect::<Vec<_>>()
        };
        let expected = [
            ("a".to_owned(), "b".to_owned()),
            ("x".to_owned(), "y".to_owned()),
        ];
        assert_eq!(cycle_edges("from a import A\nfrom x import X\n"), expected);
        assert_eq!(cycle_edges("from x import X\nfrom a import A\n"), expected);
    }

    #[test]
    fn cycle_edge_preserves_imported_module_syntax_errors() {
        let (_, _, trace) = evaluate_imports_at(
            &[("/project/b.py", "from a import A\nB = A\nbroken(\n")],
            "/project/a.py",
            "from b import B\nA = B\n",
        );
        let cycle = trace
            .imports()
            .find_map(|outcome| {
                let PythonImportOutcome::Evaluated {
                    edge,
                    status: PythonImportEvaluationStatus::Cycle { syntax_errors },
                } = outcome
                else {
                    return None;
                };
                (edge.importer.name().as_str() == "a" && edge.imported.name().as_str() == "b")
                    .then_some(syntax_errors)
            })
            .expect("the canonical a-to-b cycle edge should be retained");
        assert!(
            cycle
                .iter()
                .any(|error| error.class == crate::python::PythonSyntaxErrorClass::Ordinary)
        );
    }

    #[test]
    fn python_module_evaluation_reports_import_syntax_errors() {
        let (db, settings, facts, trace) = evaluate_imports_with_paths(
            &[(
                "/project/broken.py",
                "if FLAG:\n    VALUE = 'known'\n    broken(\n",
            )],
            "/project/settings.py",
            "from broken import VALUE\n",
            &[],
        );
        let broken = db
            .file(Utf8Path::new("/project/broken.py"))
            .expect("broken file exists");
        let imports = trace.imports().collect::<Vec<_>>();
        let [
            PythonImportOutcome::Evaluated {
                edge,
                status: PythonImportEvaluationStatus::SyntaxErrors(errors),
            },
        ] = imports.as_slice()
        else {
            panic!("expected one syntax-error outcome")
        };
        assert_eq!(edge.origin.file, settings);
        assert_eq!(edge.imported.file(), broken);
        assert!(
            errors
                .iter()
                .any(|error| error.class == crate::python::PythonSyntaxErrorClass::Ordinary)
        );
        let binding = &facts.bindings["VALUE"];
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::SyntaxErrors(binding_errors) if binding_errors == errors)))));
        assert!(binding.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "broken" && member == "VALUE")))));
        assert!(
            !binding
                .alternatives()
                .any(|state| matches!(state, PythonBindingState::Unbound))
        );
    }

    #[test]
    fn ordinary_dotted_import_unreadable_leaf_leaves_failed_child_unattached() {
        let (db, _, facts, trace) = evaluate_unreadable_module(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "/project/pkg/sub.py",
            "import pkg\nimport pkg.sub as s\nCHILD = pkg.sub\n",
        );
        let pkg = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
            .expect("package exists");
        let sub = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/sub.py"))
            .expect("sub exists");
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::Evaluated { edge: a, status: PythonImportEvaluationStatus::Resolved },
             PythonImportOutcome::Evaluated { edge: b, status: PythonImportEvaluationStatus::Resolved },
             PythonImportOutcome::Unreadable { edge: c, error }]
            if a.imported.name().as_str() == "pkg" && b.imported.name().as_str() == "pkg" && c.imported.name().as_str() == "pkg.sub"
                && a.imported.file() == pkg && b.imported.file() == pkg && c.imported.file() == sub
                && error.kind() == FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)));
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert!(
            matches!(&bound(&facts, "s").value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Unreadable(_)))
        );
        assert!(
            matches!(&bound(&facts, "CHILD").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "sub"))
        );
    }

    #[test]
    fn ordinary_dotted_import_unreadable_intermediate_preserves_root_edge() {
        let (db, _, facts, trace) = evaluate_unreadable_module(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/mid/__init__.py", "MID = 'mid'\n"),
                ("/project/pkg/mid/leaf.py", "LEAF = 'leaf'\n"),
            ],
            "/project/pkg/mid/__init__.py",
            "import pkg.mid.leaf\n",
        );
        let pkg = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
            .expect("package exists");
        let mid = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/mid/__init__.py"))
            .expect("mid exists");
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::Evaluated { edge: first, status: PythonImportEvaluationStatus::Resolved }, PythonImportOutcome::Unreadable { edge: second, .. }]
            if first.imported.name().as_str() == "pkg" && first.imported.file() == pkg && second.imported.name().as_str() == "pkg.mid" && second.imported.file() == mid));
        assert!(
            matches!(&bound(&facts, "pkg").value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Unreadable(_)))
        );
    }

    #[test]
    fn from_dotted_import_unreadable_leaf_records_parent_prefix_and_fails_selection() {
        let (db, _, facts, trace) = evaluate_unreadable_module(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "VALUE = 'v'\n"),
            ],
            "/project/pkg/sub.py",
            "from pkg.sub import VALUE\n",
        );
        let pkg = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
            .expect("package exists");
        let sub = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/sub.py"))
            .expect("sub exists");
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::Evaluated { edge: first, status: PythonImportEvaluationStatus::Resolved }, PythonImportOutcome::Unreadable { edge: second, .. }]
            if first.imported.name().as_str() == "pkg" && first.imported.file() == pkg && second.imported.name().as_str() == "pkg.sub" && second.imported.file() == sub));
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Unreadable(_)))
        );
    }

    #[test]
    fn from_dotted_import_unreadable_intermediate_records_root_prefix_and_fails_selection() {
        let (db, _, facts, trace) = evaluate_unreadable_module(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/mid/__init__.py", "MID = 'mid'\n"),
                ("/project/pkg/mid/leaf.py", "VALUE = 'v'\n"),
            ],
            "/project/pkg/mid/__init__.py",
            "from pkg.mid.leaf import VALUE\n",
        );
        let pkg = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
            .expect("package exists");
        let mid = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/mid/__init__.py"))
            .expect("mid exists");
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::Evaluated { edge: first, status: PythonImportEvaluationStatus::Resolved }, PythonImportOutcome::Unreadable { edge: second, .. }]
            if first.imported.name().as_str() == "pkg" && first.imported.file() == pkg && second.imported.name().as_str() == "pkg.mid" && second.imported.file() == mid));
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Unreadable(_)))
        );
    }

    #[test]
    fn named_from_import_unreadable_child_records_edge_without_attachment() {
        let (db, settings, facts, trace) = evaluate_unreadable_module(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/child.py", "VALUE = 'hidden'\n"),
            ],
            "/project/pkg/child.py",
            "from pkg import child\nimport pkg\nATTR = pkg.child\n",
        );
        let package = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/__init__.py"))
            .expect("package exists");
        let child_file = djls_source::path_to_file(&db, Utf8Path::new("/project/pkg/child.py"))
            .expect("child exists");
        let imports = trace.imports().collect::<Vec<_>>();
        assert!(
            matches!(imports[0], PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.name().as_str() == "pkg")
        );
        assert!(
            matches!(imports[1], PythonImportOutcome::Unreadable { edge, .. } if edge.imported.name().as_str() == "pkg.child")
        );
        assert_eq!(
            &trace.files().collect::<Vec<_>>()[..3],
            [settings, package, child_file]
        );
        assert!(
            matches!(&bound(&facts, "child").value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Unreadable(_)))
        );
        assert!(
            matches!(&bound(&facts, "ATTR").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "child"))
        );
    }

    #[test]
    fn unreadable_star_source_preserves_stale_bindings_and_opens_the_namespace() {
        let source = "VALUE = 'local'\nfrom unreadable import *\n";
        let (_, file, facts, _) = evaluate_unreadable_module(
            &[("/project/unreadable.py", "VALUE = 'hidden'\n")],
            "/project/unreadable.py",
            source,
        );
        assert_eq!(
            binding_strings(&facts, "VALUE"),
            ["local"].into_iter().collect()
        );
        let span = "from unreadable import *";
        let expected = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(span).expect("span exists"), span.len()),
        );
        assert!(facts.bindings["VALUE"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Unreadable(_)) && unknown.origins().collect::<Vec<_>>() == [expected]))));
        assert!(
            matches!(facts.namespace_remainder.as_ref().expect("namespace is open").as_slice(), [cause] if matches!(cause.unknown.cause, PythonUnknownCause::Unreadable(_)))
        );
    }

    #[test]
    fn ordinary_import_unreadable_root_binds_typed_unreadable_unknown() {
        let (db, settings, facts, trace) = evaluate_unreadable_module(
            &[("/project/unreadable.py", "VALUE = 'hidden'\n")],
            "/project/unreadable.py",
            "import unreadable\n",
        );
        let unreadable = djls_source::path_to_file(&db, Utf8Path::new("/project/unreadable.py"))
            .expect("file exists");
        let imports = trace.imports().collect::<Vec<_>>();
        let [PythonImportOutcome::Unreadable { edge, error }] = imports.as_slice() else {
            panic!("expected one unreadable outcome")
        };
        assert_eq!(edge.origin.file, settings);
        assert_eq!(edge.imported.file(), unreadable);
        assert_eq!(edge.importer.name().as_str(), "settings");
        assert_eq!(edge.imported.name().as_str(), "unreadable");
        assert_eq!(
            error.kind(),
            FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)
        );
        assert!(
            matches!(&bound(&facts, "unreadable").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::Unreadable(error) if error.kind() == FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)))
        );
        assert_eq!(trace.files().collect::<Vec<_>>(), [settings, unreadable]);
    }

    #[test]
    fn python_module_evaluation_reports_unreadable_import() {
        let (db, settings, facts, trace) = evaluate_unreadable_module(
            &[("/project/unreadable.py", "VALUE = 'hidden'\n")],
            "/project/unreadable.py",
            "from unreadable import VALUE\n",
        );
        let unreadable = djls_source::path_to_file(&db, Utf8Path::new("/project/unreadable.py"))
            .expect("file exists");
        let imports = trace.imports().collect::<Vec<_>>();
        let [PythonImportOutcome::Unreadable { edge, error }] = imports.as_slice() else {
            panic!("expected one unreadable outcome")
        };
        assert_eq!(edge.origin.file, settings);
        assert_eq!(edge.imported.file(), unreadable);
        assert_eq!(error.path(), Utf8Path::new("/project/unreadable.py"));
        assert_eq!(
            error.kind(),
            FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)
        );
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::Unreadable(error) if error.path() == Utf8Path::new("/project/unreadable.py") && error.kind() == FileReadErrorKind::Filesystem(io::ErrorKind::PermissionDenied)))
        );
    }

    #[test]
    fn python_intrinsics_follow_import_and_assignment_aliases() {
        let (_, facts, trace) = evaluate_imports(
            &[],
            include_str!("../../../tests/testdata/settings_extraction/python_intrinsics.py"),
        );
        for (name, expected) in [
            ("P", PythonIntrinsic::PathlibPathType),
            ("operating_system", PythonIntrinsic::OsModule),
            ("path_join", PythonIntrinsic::OsPathJoinFunction),
            ("path_dirname", PythonIntrinsic::OsPathDirnameFunction),
            ("stringify", PythonIntrinsic::BuiltinsStrType),
        ] {
            assert_eq!(
                bound(&facts, name).value.kind,
                PythonValueKind::Intrinsic(expected),
                "{name}"
            );
        }
        for (name, expected) in [
            ("MODULE_FILE", "/project/settings.py"),
            ("TEMPLATES_DIR", "/project/templates"),
            ("STATIC_DIR", "/project/static"),
            ("PARENT", "/project"),
            ("EMPTY_PARENT", ""),
            ("TRAILING_PARENT", "/project"),
            ("ROOT_PARENT", "/"),
            ("STATIC_TEXT", "/project/static"),
        ] {
            assert!(
                matches!(&bound(&facts, name).value.kind, PythonValueKind::Str(value) if value == expected),
                "{name}"
            );
        }
        for (name, expected) in [
            ("ROOT", "/project"),
            ("RESOLVED", "/project/settings.py"),
            ("FIRST_PARENT", "/project"),
            ("SECOND_PARENT", "/"),
            ("NORMALIZED", "/"),
        ] {
            assert!(
                matches!(&bound(&facts, name).value.kind, PythonValueKind::Path(value) if value.as_path() == expected),
                "{name}"
            );
        }
        for name in [
            "RELATIVE_PATH",
            "INVALID_PARENTS",
            "INVALID_METHOD",
            "INVALID_DIVISION",
        ] {
            assert!(matches!(
                bound(&facts, name).value.kind,
                PythonValueKind::Unknown(_)
            ));
        }
        assert!(
            trace
                .imports()
                .all(|outcome| !matches!(outcome, PythonImportOutcome::NotFound { .. }))
        );
        assert!(
            trace
                .imports()
                .all(|outcome| matches!(outcome, PythonImportOutcome::SkippedExternal { .. }))
        );
    }

    #[test]
    fn ordinary_dotted_import_records_exact_prefix_dependencies_and_surviving_child_value() {
        let (db, settings, facts, trace) = evaluate_imports_with_paths(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "/project/settings.py",
            "import pkg.sub\nCHILD = pkg.sub\nVIA = pkg.sub.LEAF\n",
            &[],
        );
        let pkg_init = db
            .file(Utf8Path::new("/project/pkg/__init__.py"))
            .expect("package exists");
        let pkg_sub = db
            .file(Utf8Path::new("/project/pkg/sub.py"))
            .expect("submodule exists");
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg.sub"]);
        assert_eq!(
            trace.files().collect::<Vec<_>>(),
            [settings, pkg_init, pkg_sub]
        );
        assert_eq!(module_name(&facts, "CHILD"), ("pkg.sub", false));
        assert!(
            matches!(&bound(&facts, "VIA").value.kind, PythonValueKind::Str(text) if text == "leaf")
        );
    }

    #[test]
    fn ordinary_dotted_import_records_exact_outcome_sequence_and_identities() {
        let source = "import pkg.sub\n";
        let (db, settings, _, trace) = evaluate_imports_with_paths(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "/project/settings.py",
            source,
            &[],
        );
        let pkg_init = db
            .file(Utf8Path::new("/project/pkg/__init__.py"))
            .expect("package exists");
        let pkg_sub = db
            .file(Utf8Path::new("/project/pkg/sub.py"))
            .expect("submodule exists");
        let clause = Origin::new(
            settings,
            Span::saturating_from_parts_usize(7, "pkg.sub".len()),
        );
        let imports = trace.imports().collect::<Vec<_>>();
        assert!(matches!(imports.as_slice(),
            [PythonImportOutcome::Evaluated { edge: first, status: PythonImportEvaluationStatus::Resolved },
             PythonImportOutcome::Evaluated { edge: second, status: PythonImportEvaluationStatus::Resolved }]
            if first.origin == clause && first.importer.name().as_str() == "settings" && first.imported.file() == pkg_init && first.imported.name().as_str() == "pkg"
                && second.origin == clause && second.importer.name().as_str() == "settings" && second.imported.file() == pkg_sub && second.imported.name().as_str() == "pkg.sub"));
        assert_eq!(
            trace.files().collect::<Vec<_>>(),
            [settings, pkg_init, pkg_sub]
        );
    }

    #[test]
    fn python_module_evaluation_reports_skipped_external_import() {
        let (_, settings, facts, trace) = evaluate_imports_with_paths(
            &[("/vendor/site-packages/external.py", "VALUE = 'external'\n")],
            "/project/settings.py",
            "from external import VALUE\n",
            &["/vendor/site-packages".into()],
        );
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::SkippedExternal { origin, module }] if origin.file == settings && module.as_str() == "external"));
        let value = &facts.bindings["VALUE"];
        assert!(value.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::SkippedExternal(module) if module.as_str() == "external")))));
        assert!(value.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "external" && member == "VALUE")))));
    }

    #[test]
    fn python_module_evaluation_skips_external_star_import() {
        let (_, settings, facts, trace) = evaluate_imports_with_paths(
            &[("/vendor/site-packages/external.py", "VALUE = 'external'\n")],
            "/project/settings.py",
            "from external import *\n",
            &["/vendor/site-packages".into()],
        );
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::SkippedExternal { origin, module }] if origin.file == settings && module.as_str() == "external"));
        assert!(!facts.bindings.contains_key("VALUE"));
        assert_eq!(trace.files().collect::<Vec<_>>(), [settings]);
        assert!(
            matches!(facts.namespace_remainder.as_ref().expect("namespace is open").as_slice(),
            [cause] if matches!(&cause.unknown.cause, PythonUnknownCause::SkippedExternal(module) if module.as_str() == "external"))
        );
    }

    #[test]
    fn python_module_evaluation_skips_named_and_star_imports_from_editable_roots() {
        for source in ["from external import VALUE\n", "from external import *\n"] {
            let (_, settings, _, trace) = evaluate_imports_with_paths(
                &[
                    ("/vendor/site-packages/project.pth", "/editable-package\n"),
                    ("/editable-package/external.py", "VALUE = 'external'\n"),
                ],
                "/project/settings.py",
                source,
                &["/vendor/site-packages".into()],
            );
            assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
                [PythonImportOutcome::SkippedExternal { origin, module }] if origin.file == settings && module.as_str() == "external"));
            assert_eq!(trace.files().collect::<Vec<_>>(), [settings]);
        }
    }

    #[test]
    fn ordinary_import_binds_module_from_extra_root() {
        let (db, settings, facts, trace) = evaluate_imports_with_paths(
            &[("/vendor/shared.py", "VALUE = 'extra'\n")],
            "/project/settings.py",
            "import shared\n",
            &["/vendor".into()],
        );
        let shared = db
            .file(Utf8Path::new("/vendor/shared.py"))
            .expect("shared exists");
        assert_eq!(module_name(&facts, "shared"), ("shared", false));
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved }]
            if edge.imported.file() == shared && edge.imported.name().as_str() == "shared"));
        assert_eq!(trace.files().collect::<Vec<_>>(), [settings, shared]);
    }

    #[test]
    fn python_module_evaluation_follows_named_and_star_imports_from_extra_roots() {
        for source in ["from shared import VALUE\n", "from shared import *\n"] {
            let (db, settings, facts, trace) = evaluate_imports_with_paths(
                &[("/vendor/shared.py", "VALUE = 'extra'\n")],
                "/project/settings.py",
                source,
                &["/vendor".into()],
            );
            let shared = db
                .file(Utf8Path::new("/vendor/shared.py"))
                .expect("shared exists");
            assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
                [PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved }]
                if edge.imported.file() == shared));
            assert_eq!(trace.files().collect::<Vec<_>>(), [settings, shared]);
            assert!(
                matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Str(value) if value == "extra")
            );
        }
    }

    #[test]
    fn python_module_cycle_preserves_stable_side_dependencies() {
        let (_, _, trace) = evaluate_imports_at(
            &[
                ("/project/side.py", "SIDE = 'stable'\n"),
                ("/project/b.py", "from a import A\nB = A\n"),
            ],
            "/project/a.py",
            "from side import SIDE\nfrom b import B\nA = B\n",
        );
        let side = trace
            .imports()
            .find_map(|outcome| {
                if let PythonImportOutcome::Evaluated {
                    edge,
                    status: PythonImportEvaluationStatus::Resolved,
                } = outcome
                    && edge.imported.name().as_str() == "side"
                {
                    Some(edge.imported.file())
                } else {
                    None
                }
            })
            .expect("side import should resolve");
        assert!(trace.files().any(|file| file == side));
    }

    #[test]
    fn ordinary_dotted_import_self_cycle_attaches_open_child() {
        let (_, facts, trace) = evaluate_imports_at(
            &[("/project/pkg/__init__.py", "import pkg.sub\n")],
            "/project/pkg/sub.py",
            "import pkg.sub\nLEAF = 'sub'\nSELF = pkg.sub\nCYC = pkg.sub.LEAF\n",
        );
        let imports = import_names(&trace);
        assert!(imports.contains(&"resolved:pkg".to_string()));
        assert!(imports.contains(&"cycle:pkg.sub".to_string()));
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(module_name(&facts, "SELF"), ("pkg.sub", false));
        assert!(
            matches!(&bound(&facts, "CYC").value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Cycle))
        );
        assert!(
            matches!(&bound(&facts, "LEAF").value.kind, PythonValueKind::Str(text) if text == "sub")
        );
    }

    #[test]
    fn ordinary_import_cycle_widening_preserves_unrelated_child_coordinate() {
        let (_, facts, trace) = evaluate_imports_at(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/stable.py", "LEAF = 'stable'\n"),
                ("/project/pkg/cyc.py", "import root\nLEAF = 'cyc'\n"),
            ],
            "/project/root.py",
            "import pkg.stable\nimport pkg.cyc\nSTABLE = pkg.stable\nCYC = pkg.cyc\n",
        );
        assert!(facts.bindings["STABLE"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg.stable"))));
        assert!(trace.imports().any(|outcome| matches!(
            outcome,
            PythonImportOutcome::Evaluated {
                status: PythonImportEvaluationStatus::Cycle { .. },
                ..
            }
        )));
    }

    #[test]
    fn ordinary_dotted_import_branch_attachment_correlates_present_and_absent_child() {
        let (_, facts, _) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg\nif condition:\n    import pkg.sub\nCHILD = pkg.sub\n",
        );
        let child = &facts.bindings["CHILD"];
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg.sub"))));
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "sub")))));
    }

    #[test]
    fn ordinary_dotted_import_zero_iteration_loop_leaves_child_uncertain() {
        let (_, facts, _) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg\nfor item in ITEMS:\n    import pkg.sub\nCHILD = pkg.sub\n",
        );
        let child = &facts.bindings["CHILD"];
        assert!(!child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg.sub"))));
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::UnsupportedExpression)))));
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "sub")))));
    }

    #[test]
    fn ordinary_import_non_dotted_self_cycle_binds_handle_and_records_cycle_edge() {
        let (_, facts, trace) =
            evaluate_imports_at(&[], "/project/a.py", "import a\nVALUE = 'a'\n");
        assert!(import_names(&trace).contains(&"cycle:a".to_string()));
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Str(text) if text == "a")
        );
    }

    #[test]
    fn parent_package_component_cycle_records_the_parent_edge() {
        let (_, facts, trace) = evaluate_imports_at(
            &[("/project/pkg/__init__.py", "from pkg.sub import X\n")],
            "/project/pkg/sub.py",
            "import pkg\nX = 'x'\n",
        );
        let imports = import_names(&trace);
        assert!(
            imports
                .iter()
                .any(|entry| entry == "cycle:pkg" || entry == "resolved:pkg")
        );
        assert!(imports.iter().any(|entry| entry.starts_with("cycle:")));
        assert!(
            matches!(&bound(&facts, "X").value.kind, PythonValueKind::Str(text) if text == "x")
        );
    }

    #[test]
    fn dotted_chain_cycle_records_leaf_cycle_and_resolves_prefix() {
        let (_, facts, trace) = evaluate_imports_at(
            &[
                ("/project/a/__init__.py", ""),
                ("/project/a/b/__init__.py", ""),
            ],
            "/project/a/b/c.py",
            "import a.b.c\nVALUE = 'c'\n",
        );
        let imports = import_names(&trace);
        for prefix in ["a", "a.b"] {
            assert!(imports.contains(&format!("resolved:{prefix}")));
        }
        assert!(imports.contains(&"cycle:a.b.c".to_string()));
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Str(text) if text == "c")
        );
    }

    #[test]
    fn ordinary_dotted_import_self_cycle_binds_root_and_records_leaf_cycle() {
        let (_, facts, trace) = evaluate_imports_at(
            &[("/project/pkg/__init__.py", "import pkg.sub\n")],
            "/project/pkg/sub.py",
            "import pkg.sub\nLEAF = 'sub'\n",
        );
        let imports = import_names(&trace);
        assert!(imports.contains(&"resolved:pkg".to_string()));
        assert!(imports.contains(&"cycle:pkg.sub".to_string()));
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert!(
            matches!(&bound(&facts, "LEAF").value.kind, PythonValueKind::Str(text) if text == "sub")
        );
    }

    #[test]
    fn from_dotted_import_self_cycle_records_leaf_cycle_and_retains_local_value() {
        let (_, facts, trace) = evaluate_imports_at(
            &[("/project/pkg/__init__.py", "")],
            "/project/pkg/sub.py",
            "from pkg.sub import LEAF\nLEAF = 'sub'\n",
        );
        let imports = import_names(&trace);
        assert!(imports.contains(&"resolved:pkg".to_string()));
        assert!(imports.contains(&"cycle:pkg.sub".to_string()));
        assert!(
            matches!(&bound(&facts, "LEAF").value.kind, PythonValueKind::Str(text) if text == "sub")
        );
    }

    #[test]
    fn named_from_import_cycle_seed_parent_still_loads_exact_child() {
        let (package, facts, trace) = evaluate_imports_at(
            &[("/project/pkg/child.py", "VALUE = 'child'\n")],
            "/project/pkg/__init__.py",
            "from . import child\nATTR = child.VALUE\n",
        );
        assert_eq!(import_names(&trace), ["cycle:pkg", "resolved:pkg.child"]);
        let child_file = trace
            .imports()
            .find_map(|outcome| {
                if let PythonImportOutcome::Evaluated {
                    edge,
                    status: PythonImportEvaluationStatus::Resolved,
                } = outcome
                    && edge.imported.name().as_str() == "pkg.child"
                {
                    Some(edge.imported.file())
                } else {
                    None
                }
            })
            .expect("child should resolve");
        assert_eq!(trace.files().collect::<Vec<_>>(), [package, child_file]);
        assert!(facts.bindings["child"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::Cycle)))));
    }

    #[test]
    fn named_from_import_child_cycle_records_recovery_and_binds_handle() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                (
                    "/project/pkg/child.py",
                    "import settings\nVALUE = 'partial'\n",
                ),
            ],
            "from pkg import child\nATTR = child.VALUE\n",
        );
        assert_eq!(
            import_names(&trace),
            ["cycle:settings", "resolved:pkg", "resolved:pkg.child"]
        );
        assert_eq!(trace.files().count(), 3);
        assert_eq!(module_name(&facts, "child"), ("pkg.child", false));
        assert!(
            matches!(&bound(&facts, "ATTR").value.kind, PythonValueKind::Str(value) if value == "partial")
        );
    }

    #[test]
    fn ordinary_import_two_file_cycle_binds_handle_and_records_cycle_edge() {
        let (_, facts, trace) = evaluate_imports_at(
            &[("/project/b.py", "import a\nVALUE = 'b'\n")],
            "/project/a.py",
            "import b\nVALUE = 'a'\n",
        );
        assert_eq!(module_name(&facts, "b"), ("b", false));
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Str(text) if text == "a")
        );
        assert!(trace.imports().any(|outcome| matches!(
            outcome,
            PythonImportOutcome::Evaluated {
                status: PythonImportEvaluationStatus::Cycle { .. },
                ..
            }
        )));
    }

    #[test]
    fn python_module_long_cycle_stays_within_the_internal_iteration_guard() {
        const MEMBERS: usize = 20;
        let files = (1..MEMBERS)
            .map(|index| {
                let next = (index + 1) % MEMBERS;
                (
                    format!("/project/member_{index}.py"),
                    format!("from member_{next} import VALUE_{next}\nVALUE_{index} = '{index}'\n"),
                )
            })
            .collect::<Vec<_>>();
        let files = files
            .iter()
            .map(|(path, source)| (path.as_str(), source.as_str()))
            .collect::<Vec<_>>();
        let (_, facts, trace) = evaluate_imports_at(
            &files,
            "/project/member_0.py",
            "from member_1 import VALUE_1\nVALUE_0 = '0'\n",
        );
        assert!(
            matches!(&bound(&facts, "VALUE_0").value.kind, PythonValueKind::Str(value) if value == "0")
        );
        assert_eq!(
            trace
                .imports()
                .filter(|outcome| matches!(
                    outcome,
                    PythonImportOutcome::Evaluated {
                        status: PythonImportEvaluationStatus::Cycle { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn python_binding_normalizes_nested_unknowns_and_merges_their_evidence() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "if condition:\n    VALUE = [dynamic()]\nelse:\n    VALUE = [other_dynamic()]\n",
        );
        let value = &bound(&facts, "VALUE").value;
        let PythonValueKind::List(items) = &value.kind else {
            panic!("VALUE should remain a list");
        };
        let [PythonSequenceItem::UnknownElement(unknown)] = items.semantic_items() else {
            panic!("the list should contain one typed unknown element");
        };
        assert_eq!(unknown.cause, PythonUnknownCause::UnsupportedExpression);
        assert_eq!(
            unknown
                .origins()
                .next()
                .expect("unknown should retain an origin")
                .span
                .start(),
            27
        );
        assert_eq!(
            value
                .origins()
                .map(|origin| origin.span.start())
                .collect::<Vec<_>>(),
            [26, 56]
        );
    }

    #[test]
    fn python_module_evaluation_reports_invalid_import_with_typed_cause() {
        let source = "from ...missing import VALUE\n";
        let (file, facts, trace) = evaluate_imports(&[], source);
        let expected = Origin::new(
            file,
            Span::saturating_from_parts_usize(0, source.trim_end().len()),
        );
        assert!(matches!(trace.imports().collect::<Vec<_>>().as_slice(),
            [PythonImportOutcome::InvalidImport { origin, reason: PythonImportNameError::TooManyDots }]
            if *origin == expected));
        assert!(matches!(&bound(&facts, "VALUE").value.kind,
            PythonValueKind::Unknown(unknown)
            if unknown.cause == PythonUnknownCause::InvalidImport(PythonImportNameError::TooManyDots)
                && unknown.origins().collect::<Vec<_>>() == [expected]));
    }

    #[test]
    fn from_dotted_import_external_suffix_skips_and_fails_selection() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/ns/placeholder.py", ""),
                (
                    "/project/.venv/lib/python3.12/site-packages/ns/sub.py",
                    "VALUE = 'ext'\n",
                ),
            ],
            "from ns.sub import VALUE\n",
        );
        assert_eq!(import_names(&trace), ["external:ns.sub"]);
        let alternatives = facts.bindings["VALUE"].alternatives().collect::<Vec<_>>();
        assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind,
            PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::SkippedExternal(module) if module.as_str() == "ns.sub")))));
        assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind,
            PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "ns.sub" && member == "VALUE")))));
    }

    #[test]
    fn ordinary_dotted_import_external_suffix_attaches_open_child() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/ns/placeholder.py", ""),
                (
                    "/project/.venv/lib/python3.12/site-packages/ns/sub.py",
                    "VALUE = 'ext'\n",
                ),
            ],
            "import ns.sub\nCHILD = ns.sub\nATTR = ns.sub.VALUE\n",
        );
        assert_eq!(import_names(&trace), ["external:ns.sub"]);
        assert_eq!(module_name(&facts, "ns"), ("ns", true));
        assert_eq!(module_name(&facts, "CHILD"), ("ns.sub", false));
        let attr = &facts.bindings["ATTR"];
        assert!(attr.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::SkippedExternal(module) if module.as_str() == "ns.sub")))), "reading an external suffix attribute surfaces SkippedExternal: {attr:?}");
    }

    #[test]
    fn named_from_import_loads_namespace_child_and_types_missing_child() {
        let (_, success, trace) = evaluate_imports(
            &[("/project/ns/child.py", "")],
            "from ns import child\nimport ns as parent\nATTR = parent.child\n",
        );
        assert_eq!(module_name(&success, "child"), ("ns.child", false));
        assert_eq!(import_names(&trace), ["resolved:ns.child"]);
        assert_eq!(trace.files().count(), 2);
        assert_eq!(module_name(&success, "ATTR"), ("ns.child", false));
        let (_, missing, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "")],
            "from pkg import absent\nimport pkg\nATTR = pkg.absent\n",
        );
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "notfound:pkg.absent", "resolved:pkg"]
        );
        assert_eq!(trace.files().count(), 2);
        assert!(
            matches!(&bound(&missing, "absent").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "pkg" && member == "absent"))
        );
        assert!(
            matches!(&bound(&missing, "ATTR").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "absent"))
        );
    }

    #[test]
    fn named_from_import_external_package_child_keeps_identity_open_without_dependencies() {
        let (_, facts, trace) = evaluate_imports(
            &[
                (
                    "/project/.venv/lib/python3.12/site-packages/external/__init__.py",
                    "PARENT = 'not evaluated'\n",
                ),
                (
                    "/project/.venv/lib/python3.12/site-packages/external/child.py",
                    "VALUE = 'not evaluated'\n",
                ),
            ],
            "from external import child\nimport external as parent\nATTR = child.VALUE\nATTACHED = parent.child\n",
        );
        assert_eq!(
            &import_names(&trace)[..2],
            ["external:external", "external:external.child"]
        );
        assert_eq!(
            trace.files().count(),
            1,
            "external source files must not become dependencies"
        );
        for name in ["child", "ATTACHED"] {
            assert!(facts.bindings[name].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "external.child"))));
        }
        assert!(facts.bindings["ATTACHED"].alternatives().all(|state| !matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::ModuleAttribute { .. })))));
        assert!(facts.bindings["ATTR"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::SkippedExternal(module) if module.as_str() == "external.child")))));
    }

    #[test]
    fn named_from_import_conditional_member_attaches_child_only_when_absent() {
        let (_, facts, trace) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "if FLAG:\n    child = 'member'\n",
                ),
                ("/project/pkg/child.py", "import pkg.side\n"),
                ("/project/pkg/side.py", ""),
            ],
            "from pkg import child\nimport pkg\nATTR = pkg.child\nSIDE = pkg.side\n",
        );
        assert_eq!(
            &import_names(&trace)[..2],
            ["resolved:pkg", "resolved:pkg.child"]
        );
        assert_eq!(trace.files().count(), 4);
        for name in ["child", "ATTR"] {
            assert!(binding_strings(&facts, name).contains("member"));
            assert!(facts.bindings[name].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg.child"))));
        }
        let side = &facts.bindings["SIDE"];
        assert!(side.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg.side"))));
        assert!(side.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "side")))));
    }

    #[test]
    fn named_child_fallback_preserves_member_mutation_namespace_and_syntax_evidence() {
        let (_, facts, trace) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "if FLAG:\n    child = []\n    child.append('member')\nif BROKEN:\n    from clean import *\n    broken(]\nfrom missing import *\n",
                ),
                ("/project/pkg/child.py", ""),
                ("/project/clean.py", ""),
            ],
            "from pkg import child\n",
        );
        let child = &facts.bindings["child"];
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::List(_)))));
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg.child"))));
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing")))));
        assert!(child.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::SyntaxErrors(_))))));
        assert!(
            facts
                .mutations
                .iter()
                .any(|mutation| mutation.binding == "child")
        );
        let package_index = trace.imports().position(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::SyntaxErrors(_) } if edge.imported.name().as_str() == "pkg")).expect("recovered package edge exists");
        let child_index = trace.imports().position(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.name().as_str() == "pkg.child")).expect("fallback child edge exists");
        assert!(package_index < child_index);
        assert_eq!(trace.files().count(), 4);
    }

    #[test]
    fn ordinary_import_of_external_module_binds_handle_and_skips_body() {
        let (_, facts, trace) = evaluate_imports(
            &[(
                "/project/.venv/lib/python3.12/site-packages/ext/__init__.py",
                "VALUE = 'external'\n",
            )],
            "import ext\nATTR = ext.VALUE\n",
        );
        assert_eq!(module_name(&facts, "ext"), ("ext", false));
        assert_eq!(import_names(&trace), ["external:ext"]);
        let attr = &facts.bindings["ATTR"];
        assert!(attr.alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::SkippedExternal(module) if module.as_str() == "ext")))), "reading an external module attribute surfaces SkippedExternal: {attr:?}");
        assert!(attr.alternatives().all(|state| !matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Str(_)))), "the external body must not be evaluated");
    }

    #[test]
    fn ordinary_import_binds_typed_not_found_in_source_order() {
        let source = "stale = 'old'\nimport package.child\nimport other as stale, third as alias\n";
        let (file, facts, trace) = evaluate_imports(&[], source);
        for (name, clause, missing, target) in [
            ("package", "package.child", "package.child", "package"),
            ("stale", "other as stale", "other", "stale"),
            ("alias", "third as alias", "third", "alias"),
        ] {
            let value = bound(&facts, name);
            assert!(
                matches!(&value.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == missing)),
                "{name} should be ImportNotFound({missing})"
            );
            let start = source.find(clause).expect("clause exists")
                + clause.find(target).expect("target exists");
            assert_eq!(
                value.binding_origins().collect::<Vec<_>>(),
                [Origin::new(
                    file,
                    Span::saturating_from_parts_usize(start, target.len())
                )]
            );
        }
        assert_eq!(
            import_names(&trace),
            ["notfound:package.child", "notfound:other", "notfound:third"]
        );
        assert_eq!(trace.files().collect::<Vec<_>>(), [file]);
    }

    #[test]
    fn unresolved_moduleless_relative_import_records_canonical_failure() {
        let source = "VALUE = 'local'\nfrom . import VALUE\n";
        let (file, facts, trace) = evaluate_imports_at(&[], "/project/pkg/settings.py", source);
        let text = "from . import VALUE";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(
            matches!(trace.imports().collect::<Vec<_>>().as_slice(), [PythonImportOutcome::NotFound { origin: actual, module }] if *actual == origin && module.as_str() == "pkg.VALUE")
        );
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "pkg" && member == "VALUE"))
        );
    }

    #[test]
    fn relative_import_cannot_escape_the_importer_package() {
        let source = "from ..missing import VALUE\n";
        let (file, _, trace) = evaluate_imports_at(&[], "/project/pkg/settings.py", source);
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(0, source.trim_end().len()),
        );
        assert!(
            matches!(trace.imports().collect::<Vec<_>>().as_slice(), [PythonImportOutcome::InvalidImport { origin: actual, reason: PythonImportNameError::TooManyDots }] if *actual == origin)
        );
    }

    #[test]
    fn exact_file_assignment_after_open_star_import_shadows_namespace_uncertainty() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "from dynamic import *\nOPEN = __file__\n__file__ = '/override.py'\nAFTER = __file__\n",
        );
        assert!(binding_strings(&facts, "OPEN").contains("/project/settings.py"));
        assert!(facts.bindings["OPEN"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(_)))));
        assert!(
            matches!(&bound(&facts, "AFTER").value.kind, PythonValueKind::Str(path) if path == "/override.py")
        );
    }

    #[test]
    fn conditional_file_assignment_replaces_only_its_feasible_unbound_case() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "from dynamic import *\nif FLAG:\n    __file__ = '/override.py'\nVALUE = __file__\n",
        );
        for expected in ["/override.py", "/project/settings.py"] {
            assert!(binding_strings(&facts, "VALUE").contains(expected));
        }
        assert!(facts.bindings["VALUE"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(_)))));
    }

    #[test]
    fn open_star_imports_can_shadow_implicit_path_names() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "from pathlib import Path\nfrom dynamic import *\nTEXT = str(__file__)\nFILE = Path(__file__)\n",
        );
        for name in ["TEXT", "FILE"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(
                alternatives
                    .iter()
                    .any(|state| matches!(state, PythonBindingState::Bound(bound)
                if (name == "TEXT" && matches!(&bound.value.kind, PythonValueKind::Str(_)))
                || (name == "FILE" && matches!(&bound.value.kind, PythonValueKind::Path(_))))),
                "{name} should retain the intrinsic possibility: {alternatives:#?}"
            );
            assert!(
                alternatives
                    .iter()
                    .any(|state| matches!(state, PythonBindingState::Bound(bound)
                if matches!(&bound.value.kind, PythonValueKind::Unknown(_)))),
                "{name} should retain star-import shadowing"
            );
        }
    }

    #[test]
    fn unsupported_outer_calls_do_not_contaminate_nested_path_constructors() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "from pathlib import Path\nif FLAG:\n    stringify = str\nelse:\n    stringify = dynamic\nVALUE = stringify(Path(__file__))\n",
        );
        assert!(binding_strings(&facts, "VALUE").contains("/project/settings.py"));
        assert!(facts.bindings["VALUE"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(_)))));
        assert!(matches!(
            &bound(&facts, "Path").value.kind,
            PythonValueKind::Intrinsic(PythonIntrinsic::PathlibPathType)
        ));
    }

    #[test]
    fn python_intrinsics_respect_shadowing_and_branch_constraints() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "from pathlib import Path\nimport os\nif FLAG:\n    Path = custom_path\nif OTHER:\n    os = custom_os\nif THIRD:\n    str = custom_str\nPATH_VALUE = Path(__file__)\nOS_VALUE = os.path.join('/project', 'templates')\nSTR_VALUE = str('/project/templates')\n",
        );
        for name in ["PATH_VALUE", "OS_VALUE", "STR_VALUE"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound)
                if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression))), "{name} should retain the shadowed path");
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound)
                if matches!(&bound.value.kind, PythonValueKind::Path(_) | PythonValueKind::Str(_)))), "{name} should retain the intrinsic result");
        }
        let (_, unimported, _) = evaluate_imports(&[], "VALUE = Path(__file__)\n");
        assert!(matches!(
            &bound(&unimported, "VALUE").value.kind,
            PythonValueKind::Unknown(_)
        ));
    }

    #[test]
    fn project_modules_named_like_stdlib_path_helpers_are_not_intrinsics() {
        let (_, facts, _) = evaluate_imports(
            &[("/project/pathlib.py", "Path = custom\n")],
            "from pathlib import Path\nVALUE = Path(__file__)\n",
        );
        assert!(matches!(
            &bound(&facts, "VALUE").value.kind,
            PythonValueKind::Unknown(_)
        ));
    }

    #[test]
    fn partial_project_path_helper_chains_do_not_become_stdlib_intrinsics() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/os/__init__.py", "")],
            "import os.path as os_path\nfrom os.path import join\nDIRECT = os_path.join('/project', 'templates')\nNAMED = join('/project', 'static')\n",
        );
        for name in ["os_path", "join", "DIRECT", "NAMED"] {
            assert!(facts.bindings[name].alternatives().all(|state| !matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Path(_)))), "{name} must not become an intrinsic value");
        }
        assert!(
            trace
                .imports()
                .any(|outcome| matches!(outcome, PythonImportOutcome::NotFound { .. }))
        );
    }

    #[test]
    fn path_operations_preserve_conditionally_unbound_alternatives() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "from pathlib import Path\nimport os\nBASE = Path(__file__).parent\nif FLAG:\n    SEGMENT = 'templates'\nDIVIDED = BASE / SEGMENT\nMETHOD = BASE.joinpath(SEGMENT)\nSTRING = os.path.join(str(BASE), SEGMENT)\n",
        );
        for name in ["DIVIDED", "METHOD", "STRING"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Path(_) | PythonValueKind::Str(_)))), "{name} should retain the bound path");
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression))), "{name} should retain the unbound fallthrough");
        }
    }

    #[test]
    fn known_external_module_outcomes_do_not_depend_on_selected_members() {
        for source in [
            "from pathlib import PurePath\n",
            "from pathlib import PurePath, Path\n",
            "from pathlib import *\n",
        ] {
            let (_, facts, trace) = evaluate_imports(&[], source);
            assert_eq!(trace.imports().count(), 1, "{source}");
            assert!(
                matches!(trace.imports().next(), Some(PythonImportOutcome::SkippedExternal { module, .. }) if module.as_str() == "pathlib"),
                "{source}: {trace:?}"
            );
            if source.contains("PurePath") {
                assert!(facts.bindings["PurePath"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::SkippedExternal(module) if module.as_str() == "pathlib")))), "{source}");
            } else {
                assert!(facts.namespace_remainder.is_some(), "{source}");
            }
        }
    }

    #[test]
    fn intrinsic_namespace_contamination_propagates_across_project_imports() {
        let (_, facts, _) = evaluate_imports(
            &[
                (
                    "/project/helper.py",
                    "import os\nif FLAG:\n    os.path.join = replacement\n",
                ),
                (
                    "/project/consumer.py",
                    "import os\nVALUE = os.path.join('/project', 'templates')\n",
                ),
            ],
            "import os\nimport helper\nimport consumer\nVALUE = os.path.join('/project', 'templates')\nIMPORTED_VALUE = consumer.VALUE\n",
        );
        for name in ["VALUE", "IMPORTED_VALUE"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression))), "{name} should retain the imported contamination: {alternatives:?}");
            assert!(alternatives.iter().all(|state| !matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Path(_) | PythonValueKind::Str(_)))), "{name} must not regain exact intrinsic behavior");
        }
    }

    #[test]
    fn unsupported_calls_persist_intrinsic_namespace_contamination() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "def mutate(path):\n    pass\nimport os\n_ = mutate(os.path)\nimport os as fresh_os\nVALUE = fresh_os.path.join('/project', 'templates')\n",
        );
        for name in ["fresh_os", "VALUE"] {
            let cause = if name == "VALUE" {
                PythonUnknownCause::UnsupportedExpression
            } else {
                PythonUnknownCause::UnsupportedMutation
            };
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == cause))), "{name} should retain unsupported-call contamination: {alternatives:?}");
            assert!(alternatives.iter().all(|state| !matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Path(_) | PythonValueKind::Str(_)))), "{name} must not regain exact intrinsic behavior");
        }
    }

    #[test]
    fn intrinsic_namespace_contamination_survives_reimports() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "import pathlib\nimport os\nimport builtins\npathlib.Path = custom_path\nbuiltins.str = custom_str\n(os.path if FLAG else other).join = replacement\nimport pathlib as fresh_pathlib\nimport os as fresh_os\nfrom builtins import str as fresh_str\nPATH_VALUE = fresh_pathlib.Path(__file__).parent\nOS_VALUE = fresh_os.path.join('/project', 'templates')\nSTR_VALUE = fresh_str(__file__)\n",
        );
        for name in [
            "fresh_pathlib",
            "fresh_os",
            "fresh_str",
            "PATH_VALUE",
            "OS_VALUE",
            "STR_VALUE",
        ] {
            let cause = if matches!(name, "PATH_VALUE" | "OS_VALUE") {
                PythonUnknownCause::UnsupportedExpression
            } else {
                PythonUnknownCause::UnsupportedMutation
            };
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == cause))), "{name} should remain contaminated after re-import: {alternatives:?}");
            assert!(alternatives.iter().all(|state| !matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Path(_) | PythonValueKind::Str(_)))), "{name} must not regain exact intrinsic behavior");
        }
    }

    #[test]
    fn os_path_abspath_follows_import_and_assignment_aliases() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "import os as operating_system\nfrom os.path import abspath as path_abspath\nassigned_abspath = path_abspath\nROOT_ABS = operating_system.path.abspath(__file__)\nBASE_DIR = operating_system.path.dirname(operating_system.path.dirname(operating_system.path.abspath(__file__)))\nNORMALIZED_ABS = path_abspath('/project/one/../settings.py')\nASSIGNED_ABS = assigned_abspath(__file__)\nRELATIVE_ABS = path_abspath('relative')\nMISSING_ABS = path_abspath()\nKEYWORD_ABS = path_abspath(path=__file__)\n",
        );
        for name in ["path_abspath", "assigned_abspath"] {
            assert!(
                matches!(
                    &bound(&facts, name).value.kind,
                    PythonValueKind::Intrinsic(PythonIntrinsic::OsPathAbspathFunction)
                ),
                "{name}"
            );
        }
        for name in ["ROOT_ABS", "NORMALIZED_ABS", "ASSIGNED_ABS"] {
            assert!(
                matches!(&bound(&facts, name).value.kind, PythonValueKind::Str(value) if value == "/project/settings.py"),
                "{name}"
            );
        }
        assert!(
            matches!(&bound(&facts, "BASE_DIR").value.kind, PythonValueKind::Str(value) if value == "/")
        );
        for name in ["RELATIVE_ABS", "MISSING_ABS", "KEYWORD_ABS"] {
            assert!(
                matches!(&bound(&facts, name).value.kind, PythonValueKind::Unknown(_)),
                "{name}"
            );
        }
    }

    #[test]
    fn contaminated_os_namespace_rejects_env_reads() {
        for mutation in [
            "env.get = replacement",
            "env['APP'] = 'runtime'",
            "env['APP'] += '-changed'",
        ] {
            let source = format!(
                "import os\nGETENV = os.getenv\nenv = os.environ\n{mutation}\nimport os as fresh_os\nVALUE = fresh_os.getenv('APP')\nALIAS_VALUE = GETENV('APP')\n"
            );
            let (_, facts, _) = evaluate_imports(&[], &source);
            for name in ["fresh_os", "VALUE", "ALIAS_VALUE"] {
                let value = bound(&facts, name);
                assert!(
                    matches!(&value.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedMutation),
                    "{mutation}: {name}: {value:#?}"
                );
            }
        }
    }

    #[test]
    fn discarded_env_reads_preserve_argument_call_effects() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "def mutate(value):\n    pass\nimport os\nos.getenv(mutate(os.path))\nimport os as fresh_os\nVALUE = fresh_os.getenv('APP')\n",
        );
        for name in ["fresh_os", "VALUE"] {
            assert!(
                matches!(&bound(&facts, name).value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedMutation)
            );
        }
    }

    #[test]
    fn chained_container_assignment_invalidates_nested_mutable_sources() {
        let facts = evaluate_source(
            "DIRS = []\nWRAPPER = {'DIRS': DIRS}\nLEFT = RIGHT = WRAPPER\nRIGHT['DIRS'].append('/templates')",
        );
        assert!(
            facts.bindings["DIRS"].alternatives().any(|state| matches!(
                state,
                PythonBindingState::Bound(bound)
                    if matches!(&bound.value.kind, PythonValueKind::Unknown(_))
            )),
            "{facts:#?}"
        );
    }

    #[test]
    fn nested_dictionary_assignment_invalidates_stale_aliases_only() {
        let facts = evaluate_source(
            "OPTIONS = {}\nALIAS = OPTIONS\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'OPTIONS': OPTIONS}]\nTEMPLATES[0]['OPTIONS']['loaders'] = ('loader',)\n",
        );
        for name in ["OPTIONS", "ALIAS"] {
            assert!(facts.bindings[name].alternatives().any(|state| matches!(
                state,
                PythonBindingState::Bound(bound)
                    if matches!(&bound.value.kind, PythonValueKind::Unknown(_))
            )));
        }
    }

    #[test]
    fn handler_if_uses_stable_arms_across_try_prefixes() {
        let source = "try:\n    B = True\n    MARKER = 1\nexcept Exception:\n    if A:\n        VALUE = 'a'\n    elif B:\n        VALUE = 'b'\n    else:\n        VALUE = 'c'\n";
        let (_, facts, _) = evaluate_imports(&[], source);
        assert_eq!(
            binding_strings(&facts, "VALUE"),
            ["a", "b", "c"].into_iter().collect()
        );
    }

    #[test]
    fn short_circuit_returns_the_decisive_operand_without_boolean_coercion() {
        let (_, facts, _) = evaluate_imports(
            &[("/project/helper.py", "")],
            "import helper\nVALUE = helper or UNKNOWN\n",
        );
        assert_eq!(module_name(&facts, "VALUE"), ("helper", false));
    }

    #[test]
    fn python_module_evaluation_follows_recursive_imports() {
        let (settings, facts, trace) = evaluate_imports(
            &[("/project/base.py", "VALUE = 'from base'\n")],
            "from base import VALUE\nCOPY = VALUE\n",
        );
        assert_eq!(facts.bindings["VALUE"].alternatives().count(), 1);
        assert_eq!(facts.bindings["COPY"].alternatives().count(), 1);
        assert_eq!(trace.files().count(), 2);
        let base = trace
            .files()
            .find(|file| *file != settings)
            .expect("base dependency exists");
        assert!(trace.imports().any(|outcome| matches!(outcome,
            PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved }
                if edge.imported.name().as_str() == "base" && edge.imported.file() == base
        )));
    }

    #[test]
    fn intrinsic_attribute_writes_invalidate_aliasing_owners() {
        let (_, facts, _) = evaluate_imports(
            &[],
            "import os\npath_alias = os.path\npath_alias.join = replacement\nimport os as fresh_os\nVALUE = os.path.join('/project', 'templates')\nFRESH_VALUE = fresh_os.path.join('/project', 'static')\n",
        );
        for name in ["os", "path_alias", "fresh_os", "VALUE", "FRESH_VALUE"] {
            let cause = if name.ends_with("VALUE") {
                PythonUnknownCause::UnsupportedExpression
            } else {
                PythonUnknownCause::UnsupportedMutation
            };
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(alternatives.iter().any(|state| matches!(state,
                PythonBindingState::Bound(bound)
                    if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == cause)
            )), "{name} should be invalidated by the aliased write: {alternatives:?}");
            assert!(alternatives.iter().all(|state| !matches!(state,
                PythonBindingState::Bound(bound)
                    if matches!(&bound.value.kind, PythonValueKind::Path(_) | PythonValueKind::Str(_))
            )), "{name} must not retain an exact path result");
        }
    }

    #[test]
    fn star_import_copies_mutations_only_for_exact_or_absent_selected_names() {
        let (_, exact, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "SELECTED = []\nSELECTED.append('selected')\nOMITTED = []\nOMITTED.append('omitted')\n__all__ = ['SELECTED']\n",
            )],
            "from plugin import *\n",
        );
        assert_eq!(
            exact
                .mutations
                .iter()
                .map(|mutation| mutation.binding.as_str())
                .collect::<Vec<_>>(),
            ["SELECTED"]
        );
        let (caller_file, absent, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "PUBLIC = []\nPUBLIC.append('public')\n_PRIVATE = []\n_PRIVATE.append('private')\n",
            )],
            "PUBLIC = []\nPUBLIC.append('caller')\nfrom plugin import *\n",
        );
        assert_eq!(
            absent
                .mutations
                .iter()
                .map(|mutation| mutation.binding.as_str())
                .collect::<Vec<_>>(),
            ["PUBLIC"]
        );
        assert_ne!(
            absent.mutations.as_slice()[0].origin.file,
            caller_file,
            "definite absent-__all__ overwrite must discard the caller mutation"
        );
        let (_, dynamic, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "VALUE = []\nVALUE.append('dynamic')\n__all__ = NAMES\n",
            )],
            "from plugin import *\n",
        );
        assert!(dynamic.mutations.as_slice().is_empty());
    }

    #[test]
    fn ordinary_import_of_project_namespace_prefix_to_external_suffix() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/ns/placeholder.py", ""),
                (
                    "/project/.venv/lib/python3.12/site-packages/ns/sub.py",
                    "VALUE = 'external'\n",
                ),
            ],
            "import ns.sub\n",
        );
        assert_eq!(import_names(&trace), ["external:ns.sub"]);
        assert_eq!(module_name(&facts, "ns"), ("ns", true));
    }

    #[test]
    fn exact_all_attaches_selected_child_and_its_effects_only_on_selected_paths() {
        let (_, facts, _) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "if FLAG:\n    __all__ = ['child']\nelse:\n    __all__ = []\n",
                ),
                ("/project/pkg/child.py", "import pkg.side\n"),
                ("/project/pkg/side.py", ""),
            ],
            "from pkg import *\nimport pkg\nCHILD = pkg.child\nSIDE = pkg.side\n",
        );
        for (name, module) in [
            ("child", "pkg.child"),
            ("CHILD", "pkg.child"),
            ("SIDE", "pkg.side"),
        ] {
            let binding = &facts.bindings[name];
            assert!(binding.alternatives().any(|state| matches!(state,
                PythonBindingState::Bound(bound) if matches!(&bound.value.kind,
                    PythonValueKind::Module(PythonModule::Source(found)) if found.name().as_str() == module)
            )), "{name}: {binding:#?}");
        }
        assert!(
            facts.bindings["child"]
                .alternatives()
                .any(|state| matches!(state, PythonBindingState::Unbound))
        );
        for name in ["CHILD", "SIDE"] {
            assert!(facts.bindings[name].alternatives().any(|state| matches!(state,
                PythonBindingState::Bound(bound) if matches!(&bound.value.kind,
                    PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::ModuleAttribute { .. }))
            )), "{name} should preserve the omitted-path coordinate");
        }
    }

    #[test]
    fn successful_direct_imports_bind_exact_target_origins() {
        let source = "import root\nimport pkg.sub as salias\nimport dpkg.leaf\n";
        let (file, facts, _) = evaluate_imports(
            &[
                ("/project/root.py", "R = 'r'\n"),
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/sub.py", "S = 's'\n"),
                ("/project/dpkg/__init__.py", ""),
                ("/project/dpkg/leaf.py", "L = 'l'\n"),
            ],
            source,
        );
        for name in ["root", "salias", "dpkg"] {
            let start = source.find(name).expect("binding target exists");
            assert_eq!(
                bound(&facts, name).binding_origins().collect::<Vec<_>>(),
                [Origin::new(
                    file,
                    Span::saturating_from_parts_usize(start, name.len())
                )]
            );
        }
    }

    #[test]
    fn deterministic_false_while_has_no_unreachable_import() {
        let (_, _, trace) = evaluate_imports(
            &[],
            "while False:\n    from missing_body import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['else']\n",
        );
        assert_eq!(
            trace.imports().count(),
            0,
            "the unreachable while body must contribute no import outcome"
        );
    }

    #[test]
    fn ambiguous_while_records_import_and_mutation() {
        let source = "INSTALLED_APPS = []\nwhile FLAG:\n    INSTALLED_APPS.append('loop')\nelse:\n    from plugin import VALUE\n";
        let (file, facts, trace) =
            evaluate_imports(&[("/project/plugin.py", "VALUE = '/static/'")], source);
        let imported = trace
            .files()
            .find(|item| *item != file)
            .expect("plugin dependency exists");
        assert!(trace.imports().any(|outcome| matches!(outcome,
            PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.file() == imported
        )));
        assert_eq!(trace.imports().count(), 1);
        let text = "INSTALLED_APPS.append('loop')";
        assert!(matches!(facts.mutations.as_slice(), [mutation]
            if mutation.binding == "INSTALLED_APPS"
                && mutation.operation == PythonMutationOperation::Append
                && mutation.path.as_slice().is_empty()
                && mutation.origin == Origin::new(file, Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()))));
    }

    #[test]
    fn attached_all_member_is_classified_through_the_shared_projection() {
        let source = "PUBLIC = 'prior'\nimport pkg.__all__\nfrom pkg import *\n";
        let (file, facts, _) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PUBLIC = 'imported'\n"),
                ("/project/pkg/__all__.py", ""),
            ],
            source,
        );
        assert_eq!(
            binding_strings(&facts, "PUBLIC"),
            ["imported", "prior"].into_iter().collect()
        );
        assert!(facts.bindings["PUBLIC"].alternatives().any(|state| matches!(state,
            PythonBindingState::Bound(bound) if matches!(&bound.value.kind,
                PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression))));
        let text = "from pkg import *";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        let causes = facts
            .namespace_remainder
            .as_ref()
            .expect("namespace remains open")
            .as_slice();
        assert!(
            matches!(causes, [cause] if cause.unknown.cause == PythonUnknownCause::UnsupportedExpression && cause.unknown.origins().collect::<Vec<_>>() == [origin])
        );
    }

    #[test]
    fn syntax_uncertainty_in_all_is_dynamic_and_rebased_to_the_star_site() {
        let source = "VALUE = 'local'\nfrom plugin import *\n";
        let (file, facts, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "VALUE = 'imported'\n__all__ = [\n    'VALUE',\n    @\n]\n",
            )],
            source,
        );
        assert!(binding_strings(&facts, "VALUE").contains("local"));
        let text = "from plugin import *";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(
            facts
                .namespace_remainder
                .as_ref()
                .expect("namespace remains open")
                .as_slice()
                .iter()
                .any(
                    |cause| matches!(cause.unknown.cause, PythonUnknownCause::SyntaxErrors(_))
                        && cause.unknown.origins().collect::<Vec<_>>() == [origin]
                ),
            "{facts:#?}"
        );
    }

    #[test]
    fn cycle_uncertainty_in_all_preserves_stale_bindings_and_opens_the_namespace() {
        let source = "VALUE = 'local'\nfrom a import *\n";
        let (file, facts, _) = evaluate_imports(
            &[
                (
                    "/project/a.py",
                    "VALUE = 'imported'\n__all__ = ['VALUE']\nfrom b import *\n",
                ),
                ("/project/b.py", "from a import *\n"),
            ],
            source,
        );
        assert_eq!(
            binding_strings(&facts, "VALUE"),
            ["local"].into_iter().collect()
        );
        let text = "from a import *";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(
            facts
                .namespace_remainder
                .as_ref()
                .expect("namespace remains open")
                .as_slice()
                .iter()
                .any(|cause| cause.unknown.cause == PythonUnknownCause::Cycle
                    && cause.unknown.origins().collect::<Vec<_>>() == [origin]),
            "{facts:#?}"
        );
    }

    #[test]
    fn tuple_index_augmented_add_mutates_nested_list_transactionally() {
        let source = "ROOT = ([],)\nROOT[0] += ['a']\n";
        let (file, facts) = evaluate_source_with_file(source);
        let root = bound(&facts, "ROOT");
        assert_eq!(string_items(nested_tuple_list(&root.value)), ["a"]);
        let text = "ROOT[0] += ['a']";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(root.value.origins().any(|item| item == origin));
        assert!(
            matches!(facts.mutations.as_slice(), [mutation] if mutation.binding == "ROOT" && mutation.operation == PythonMutationOperation::Extend && mutation.path.as_slice() == [PythonMutationPathSegment::Index(0)])
        );
    }

    #[test]
    fn tuple_index_extend_mutates_nested_list_and_invalidates_stale_alias() {
        let facts = evaluate_source("INNER = []\nROOT = (INNER,)\nROOT[0].extend(['a'])\n");
        assert_eq!(
            string_items(nested_tuple_list(&bound(&facts, "ROOT").value)),
            ["a"]
        );
        assert!(facts.bindings["INNER"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(_)))));
        assert!(
            matches!(facts.mutations.as_slice(), [mutation] if mutation.binding == "ROOT" && mutation.operation == PythonMutationOperation::Extend && mutation.path.as_slice() == [PythonMutationPathSegment::Index(0)])
        );
    }

    #[test]
    fn tuple_index_augmented_add_bool_failure_degrades_root_and_alias() {
        let facts = evaluate_source("INNER = []\nROOT = (INNER,)\nROOT[0] += True\n");
        for name in ["ROOT", "INNER"] {
            assert!(facts.bindings[name].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(_)))));
        }
        assert!(
            facts
                .mutations
                .iter()
                .any(|mutation| mutation.binding == "ROOT"
                    && mutation.operation == PythonMutationOperation::Extend
                    && mutation.path.as_slice() == [PythonMutationPathSegment::Index(0)])
        );
    }

    #[test]
    fn nested_augmented_add_failure_joins_prior_values_with_mutation_unknowns() {
        let facts = evaluate_source("INNER = []\nROOT = (INNER,)\nROOT[0] += True\n");
        for name in ["ROOT", "INNER"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert_eq!(alternatives.len(), 2);
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if (name == "ROOT" && matches!(&bound.value.kind, PythonValueKind::Tuple(_))) || (name == "INNER" && matches!(&bound.value.kind, PythonValueKind::List(_))))));
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedMutation))));
        }
        assert!(
            matches!(facts.mutations.as_slice(), [mutation] if mutation.binding == "ROOT" && mutation.operation == PythonMutationOperation::Extend)
        );
    }

    #[test]
    fn name_target_list_augmented_add_preserves_prior_append_fact() {
        let source = "VALUES = []\nVALUES.append('a')\nVALUES += ['b']\n";
        let (file, facts) = evaluate_source_with_file(source);
        let value = bound(&facts, "VALUES");
        assert_eq!(string_items(&value.value), ["a", "b"]);
        assert_eq!(
            value.binding_origins().collect::<Vec<_>>(),
            [Origin::new(
                file,
                Span::saturating_from_parts_usize(source.find("[]").expect("text exists"), 2)
            )]
        );
        assert_eq!(
            facts
                .mutations
                .iter()
                .filter(|mutation| mutation.binding == "VALUES")
                .map(|mutation| mutation.operation)
                .collect::<Vec<_>>(),
            [
                PythonMutationOperation::Append,
                PythonMutationOperation::Extend
            ]
        );
    }

    #[test]
    fn selected_and_dynamic_star_paths_preserve_an_existing_child_coordinate() {
        for all in [
            "if FLAG:\n    __all__ = ['child']\nelse:\n    __all__ = []\n",
            "__all__ = NAMES\n",
        ] {
            let (_, facts, _) = evaluate_imports(
                &[
                    ("/project/pkg/__init__.py", all),
                    ("/project/pkg/child.py", ""),
                ],
                "import pkg.child\nfrom pkg import *\nATTR = pkg.child\n",
            );
            let alternatives = facts.bindings["ATTR"].alternatives().collect::<Vec<_>>();
            assert!(alternatives.iter().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg.child"))));
            assert!(alternatives.iter().all(|state| !matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::ModuleAttribute { .. })))));
        }
    }

    #[test]
    fn dynamic_all_does_not_scan_a_listable_package_child() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "__all__ = NAMES\n"),
                ("/project/pkg/child.py", "VALUE = 'child'\n"),
            ],
            "from pkg import *\nimport pkg\nCHILD = pkg.child\n",
        );
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg"]);
        assert!(
            matches!(&bound(&facts, "CHILD").value.kind, PythonValueKind::Unknown(unknown) if matches!(unknown.cause, PythonUnknownCause::ModuleAttribute { .. }))
        );
        assert!(facts.namespace_remainder.is_some());
    }

    #[test]
    fn exact_all_closes_an_otherwise_open_source_namespace() {
        let (_, facts, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "VALUE = 'imported'\nfrom missing import *\n__all__ = ['VALUE']\n",
            )],
            "VALUE = 'local'\nfrom plugin import *\n",
        );
        assert_eq!(
            binding_strings(&facts, "VALUE"),
            ["imported"].into_iter().collect()
        );
        assert!(facts.namespace_remainder.is_none());
        assert!(
            !facts.bindings["VALUE"]
                .alternatives()
                .any(|state| matches!(state, PythonBindingState::Unbound))
        );
    }

    #[test]
    fn exact_all_copies_only_mutations_from_selected_branch_paths() {
        let plugin = "if FLAG:\n    VALUE = []\n    VALUE.append('same')\n    __all__ = ['VALUE']\nelse:\n    VALUE = []\n    VALUE.append('same')\n    __all__ = []\n";
        let (_, facts, _) =
            evaluate_imports(&[("/project/plugin.py", plugin)], "from plugin import *\n");
        assert_eq!(facts.mutations.as_slice().len(), 1);
        let text = "VALUE.append('same')";
        assert_eq!(
            facts.mutations.as_slice()[0].origin.span,
            Span::saturating_from_parts_usize(plugin.find(text).expect("text exists"), text.len())
        );
    }

    #[test]
    fn exact_all_branch_child_effects_follow_arm_and_list_source_order() {
        let (_, _, trace) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "if FLAG:\n    __all__ = ['z_child', 'm_child']\nelse:\n    __all__ = ['a_child']\n",
                ),
                ("/project/pkg/z_child.py", ""),
                ("/project/pkg/m_child.py", ""),
                ("/project/pkg/a_child.py", ""),
            ],
            "from pkg import *\n",
        );
        assert_eq!(
            import_names(&trace),
            [
                "resolved:pkg",
                "resolved:pkg.z_child",
                "resolved:pkg.m_child",
                "resolved:pkg.a_child"
            ]
        );
    }

    #[test]
    fn exact_all_opposite_branch_orders_load_once_in_deterministic_arm_order() {
        let (_, _, trace) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "if FLAG:\n    __all__ = ['z_child', 'a_child']\nelse:\n    __all__ = ['a_child', 'z_child']\n",
                ),
                ("/project/pkg/z_child.py", ""),
                ("/project/pkg/a_child.py", ""),
            ],
            "from pkg import *\n",
        );
        assert_eq!(
            import_names(&trace),
            [
                "resolved:pkg",
                "resolved:pkg.z_child",
                "resolved:pkg.a_child"
            ]
        );
    }

    #[test]
    fn exact_all_conditional_member_preserves_prior_binding_and_mutation_path() {
        let plugin =
            "if FLAG:\n    VALUE = []\n    VALUE.append('imported')\n__all__ = ['VALUE']\n";
        let source = "VALUE = []\nVALUE.append('local')\nfrom plugin import *\n";
        let (file, facts, _) = evaluate_imports(&[("/project/plugin.py", plugin)], source);
        assert!(facts.bindings["VALUE"].alternatives().all(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::List(_)))));
        assert_eq!(facts.mutations.as_slice().len(), 2);
        for (body, text, local) in [
            (source, "VALUE.append('local')", true),
            (plugin, "VALUE.append('imported')", false),
        ] {
            let span = Span::saturating_from_parts_usize(
                body.find(text).expect("text exists"),
                text.len(),
            );
            assert!(
                facts
                    .mutations
                    .iter()
                    .any(|mutation| (mutation.origin.file == file) == local
                        && mutation.origin.span == span)
            );
        }
    }

    #[test]
    fn conditional_exact_and_absent_all_keep_selection_and_mutations_correlated() {
        let plugin = "VALUE = []\nVALUE.append('imported')\nif FLAG:\n    __all__ = []\n";
        let source = "VALUE = []\nVALUE.append('local')\nfrom plugin import *\n";
        let (file, facts, _) = evaluate_imports(&[("/project/plugin.py", plugin)], source);
        assert_eq!(facts.bindings["VALUE"].alternatives().filter(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::List(_)))).count(), 2);
        assert_eq!(facts.mutations.as_slice().len(), 2);
        for (body, text, local) in [
            (source, "VALUE.append('local')", true),
            (plugin, "VALUE.append('imported')", false),
        ] {
            let span = Span::saturating_from_parts_usize(
                body.find(text).expect("text exists"),
                text.len(),
            );
            assert!(
                facts
                    .mutations
                    .iter()
                    .any(|mutation| (mutation.origin.file == file) == local
                        && mutation.origin.span == span)
            );
        }
    }

    #[test]
    fn exact_all_cycle_member_preserves_caller_binding_and_mutation_path() {
        let source = "VALUE = []\nVALUE.append('local')\nfrom a import *\n";
        let (file, facts, _) = evaluate_imports(
            &[
                (
                    "/project/a.py",
                    "from b import VALUE\n__all__ = ['VALUE']\n",
                ),
                ("/project/b.py", "from a import VALUE\n"),
            ],
            source,
        );
        assert!(facts.bindings["VALUE"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::List(_)))));
        assert!(facts.bindings["VALUE"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::Cycle))));
        let text = "VALUE.append('local')";
        let span =
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len());
        assert!(
            facts
                .mutations
                .iter()
                .any(|mutation| mutation.origin.file == file && mutation.origin.span == span)
        );
    }

    #[test]
    fn dynamic_all_shapes_preserve_imported_and_prior_possibilities_and_open_namespace() {
        for all in [
            "['VALUE', dynamic()]",
            "['VALUE', *EXTRA]",
            "'VALUE'",
            "True",
        ] {
            let source = "VALUE = 'local'\nfrom plugin import *\n";
            let plugin = format!("VALUE = 'imported'\n__all__ = {all}\n");
            let (file, facts, _) = evaluate_imports(&[("/project/plugin.py", &plugin)], source);
            assert_eq!(
                binding_strings(&facts, "VALUE"),
                ["imported", "local"].into_iter().collect()
            );
            assert!(facts.bindings["VALUE"].alternatives().any(|state| matches!(state, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(_)))));
            let text = "from plugin import *";
            let origin = Origin::new(
                file,
                Span::saturating_from_parts_usize(
                    source.find(text).expect("text exists"),
                    text.len(),
                ),
            );
            let causes = facts
                .namespace_remainder
                .as_ref()
                .expect("namespace should remain open")
                .as_slice();
            assert!(
                matches!(causes, [cause] if cause.unknown.cause == PythonUnknownCause::UnsupportedExpression && cause.unknown.origins().collect::<Vec<_>>() == [origin])
            );
        }
    }

    #[test]
    fn star_import_without_all_selects_only_intrinsic_public_names_and_never_scans_children() {
        let (_, facts, trace) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "PUBLIC = 'public'\n_PRIVATE = 'private'\n",
                ),
                ("/project/pkg/child.py", "VALUE = 'child'\n"),
            ],
            "PUBLIC = 'stale public'\n_PRIVATE = 'stale private'\nfrom pkg import *\nimport pkg\nCHILD = pkg.child\n",
        );
        assert_eq!(
            binding_strings(&facts, "PUBLIC"),
            ["public"].into_iter().collect()
        );
        assert_eq!(
            binding_strings(&facts, "_PRIVATE"),
            ["stale private"].into_iter().collect()
        );
        assert!(
            matches!(&bound(&facts, "CHILD").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "child"))
        );
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg"]);
    }

    #[test]
    fn exact_all_selects_private_names_in_order_dedupes_and_loads_listed_children() {
        let (_, facts, trace) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "__all__ = ['_PRIVATE', 'second', 'first', 'second']\n_PRIVATE = 'private'\nUNLISTED = 'hidden'\n",
                ),
                ("/project/pkg/first.py", "VALUE = 'first'\n"),
                (
                    "/project/pkg/second/__init__.py",
                    "VALUE = 'second package'\n",
                ),
                ("/project/pkg/unlisted.py", "VALUE = 'unlisted'\n"),
            ],
            "from pkg import *\n",
        );
        assert_eq!(
            binding_strings(&facts, "_PRIVATE"),
            ["private"].into_iter().collect()
        );
        assert_eq!(module_name(&facts, "second"), ("pkg.second", false));
        assert_eq!(module_name(&facts, "first"), ("pkg.first", false));
        assert!(!facts.bindings.contains_key("UNLISTED"));
        assert!(!facts.bindings.contains_key("unlisted"));
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "resolved:pkg.second", "resolved:pkg.first"]
        );
    }

    #[test]
    fn exact_all_branch_alternatives_keep_common_exports_exact_and_omitted_stale_values() {
        let (_, facts, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "COMMON = 'imported common'\nLEFT = 'imported left'\nRIGHT = 'imported right'\nif FLAG:\n    __all__ = ['COMMON', 'LEFT']\nelse:\n    __all__ = ['COMMON', 'RIGHT']\n",
            )],
            "COMMON = 'stale common'\nLEFT = 'stale left'\nRIGHT = 'stale right'\nUNLISTED = 'stale unlisted'\nfrom plugin import *\n",
        );
        assert_eq!(
            binding_strings(&facts, "COMMON"),
            ["imported common"].into_iter().collect()
        );
        assert_eq!(
            binding_strings(&facts, "LEFT"),
            ["imported left", "stale left"].into_iter().collect()
        );
        assert_eq!(
            binding_strings(&facts, "RIGHT"),
            ["imported right", "stale right"].into_iter().collect()
        );
        assert_eq!(
            binding_strings(&facts, "UNLISTED"),
            ["stale unlisted"].into_iter().collect()
        );
        assert!(
            !facts.bindings["COMMON"]
                .alternatives()
                .any(|state| matches!(state, PythonBindingState::Unbound))
        );
    }

    #[test]
    fn named_from_import_existing_private_member_shadows_child() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "_child = 'member'\n"),
                ("/project/pkg/_child.py", "VALUE = 'child'\n"),
            ],
            "from pkg import _child\n",
        );
        assert!(
            matches!(&bound(&facts, "_child").value.kind, PythonValueKind::Str(value) if value == "member")
        );
        assert_eq!(import_names(&trace), ["resolved:pkg"]);
    }

    #[test]
    fn named_from_import_loads_exact_package_child_with_alias_origins() {
        let source = "from pkg import child as alias\n";
        let (file, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/child.py", "VALUE = 'child'\n"),
            ],
            source,
        );
        assert_eq!(module_name(&facts, "alias"), ("pkg.child", false));
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg.child"]);
        let alias = "child as alias";
        assert_eq!(
            bound(&facts, "alias").binding_origins().collect::<Vec<_>>(),
            [Origin::new(
                file,
                Span::saturating_from_parts_usize(
                    source.find(alias).expect("text exists"),
                    alias.len()
                )
            )]
        );
        let statement = "from pkg import child as alias";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(
                source.find(statement).expect("text exists"),
                statement.len(),
            ),
        );
        assert!(
            matches!(trace.imports().nth(1), Some(PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved }) if edge.origin == origin && edge.imported.name().as_str() == "pkg.child")
        );
        assert_eq!(trace.files().len(), 3);
    }

    #[test]
    fn from_dotted_import_syntax_leaf_records_syntax_edge_and_selects_member() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                (
                    "/project/pkg/sub.py",
                    "CHILD = 'child'\nif FLAG:\n    OTHER = 'o'\n    broken(\n",
                ),
            ],
            "from pkg.sub import CHILD\n",
        );
        assert!(trace.imports().any(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.name().as_str() == "pkg")));
        assert!(trace.imports().any(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::SyntaxErrors(_) } if edge.imported.name().as_str() == "pkg.sub")));
        assert!(
            matches!(&bound(&facts, "CHILD").value.kind, PythonValueKind::Str(text) if text == "child")
        );
    }

    #[test]
    fn python_module_evaluation_records_only_path_feasible_imports() {
        let (_, _, trace) = evaluate_imports(
            &[],
            "if False:\n    from unreachable import VALUE\nif condition:\n    from feasible import VALUE\n",
        );
        assert_eq!(trace.imports().len(), 1);
        assert!(
            matches!(trace.imports().next(), Some(PythonImportOutcome::NotFound { module, .. }) if module.as_str() == "feasible")
        );
    }

    #[test]
    fn python_module_evaluation_keeps_failed_star_import_from_loop_body() {
        let source = "for item in ITEMS:\n    from missing_star import *\n";
        let (file, facts, trace) = evaluate_imports(&[], source);
        let text = "from missing_star import *";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        let outcomes = trace.imports().collect::<Vec<_>>();
        assert!(
            matches!(outcomes.as_slice(), [PythonImportOutcome::NotFound { origin: actual, module }] if *actual == origin && module.as_str() == "missing_star")
        );
        let causes = facts
            .namespace_remainder
            .as_ref()
            .expect("namespace should remain open")
            .as_slice();
        assert!(
            matches!(causes, [cause] if matches!(&cause.unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing_star") && cause.unknown.origins().collect::<Vec<_>>() == [origin])
        );
    }

    #[test]
    fn from_dotted_import_namespace_leaf_records_source_prefix_and_not_found() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/ns/mod.py", "X = 'x'\n"),
            ],
            "from pkg.ns import VALUE\n",
        );
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "notfound:pkg.ns.VALUE"]
        );
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member } if module.as_str() == "pkg.ns" && member == "VALUE"))
        );
    }

    #[test]
    fn ordinary_dotted_import_recovers_leaf_syntax_and_attaches_child() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\nbroken(\n"),
            ],
            "import pkg.sub\nCHILD = pkg.sub\n",
        );
        assert!(trace.imports().any(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.name().as_str() == "pkg")));
        assert!(trace.imports().any(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::SyntaxErrors(_) } if edge.imported.name().as_str() == "pkg.sub")));
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(module_name(&facts, "CHILD"), ("pkg.sub", false));
    }

    #[test]
    fn python_module_evaluation_records_mutation_origins_on_values_and_effects() {
        let source = "VALUES = []\nVALUES.append('added')\n";
        let (file, facts) = evaluate_source_with_file(source);
        let [mutation] = facts.mutations.as_slice() else {
            panic!("expected one mutation")
        };
        assert_eq!(mutation.binding, "VALUES");
        assert_eq!(mutation.operation, PythonMutationOperation::Append);
        assert!(mutation.path.as_slice().is_empty());
        let text = "VALUES.append('added')";
        assert_eq!(
            mutation.origin,
            Origin::new(
                file,
                Span::saturating_from_parts_usize(
                    source.find(text).expect("text exists"),
                    text.len()
                )
            )
        );
        assert!(
            bound(&facts, "VALUES")
                .value
                .origins()
                .any(|origin| origin.span == mutation.origin.span)
        );
    }

    #[test]
    fn python_module_evaluation_records_typed_nested_mutation_facts() {
        let source = "TEMPLATES = [{'DIRS': []}]\nTEMPLATES[0]['DIRS'].append('/one')\nTEMPLATES[0]['DIRS'] += ['/two']\n";
        let (file, facts) = evaluate_source_with_file(source);
        let [append, augmented_add] = facts.mutations.as_slice() else {
            panic!("expected two mutations")
        };
        let path = [
            PythonMutationPathSegment::Index(0),
            PythonMutationPathSegment::Key("DIRS".to_string()),
        ];
        for (mutation, operation, text) in [
            (
                append,
                PythonMutationOperation::Append,
                "TEMPLATES[0]['DIRS'].append('/one')",
            ),
            (
                augmented_add,
                PythonMutationOperation::Extend,
                "TEMPLATES[0]['DIRS'] += ['/two']",
            ),
        ] {
            assert_eq!(mutation.binding, "TEMPLATES");
            assert_eq!(mutation.path.as_slice(), path);
            assert_eq!(mutation.operation, operation);
            assert_eq!(
                mutation.origin,
                Origin::new(
                    file,
                    Span::saturating_from_parts_usize(
                        source.find(text).expect("text exists"),
                        text.len()
                    )
                )
            );
        }
    }

    #[test]
    fn python_module_evaluation_discards_facts_after_unsupported_mutation() {
        let source = "VALUES = []\nVALUES.append('stale')\nVALUES.clear()\n";
        let (file, facts) = evaluate_source_with_file(source);
        assert!(facts.mutations.as_slice().is_empty());
        let text = "VALUES.clear()";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(
            matches!(&bound(&facts, "VALUES").value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedMutation && unknown.origins().collect::<Vec<_>>() == [origin])
        );
    }

    #[test]
    fn python_module_evaluation_records_nested_string_augmented_add_as_iterable_extension() {
        let facts = evaluate_source(
            "TEMPLATES = [{'DIRS': []}]\nTEMPLATES[0]['DIRS'].append('/one')\nTEMPLATES[0]['DIRS'] += 'invalid'\n",
        );
        let [append, augmented_add] = facts.mutations.as_slice() else {
            panic!("expected two mutations")
        };
        assert_eq!(append.operation, PythonMutationOperation::Append);
        assert_eq!(augmented_add.operation, PythonMutationOperation::Extend);
        assert_eq!(append.binding, "TEMPLATES");
        assert_eq!(augmented_add.binding, "TEMPLATES");
        assert!(facts.bindings["TEMPLATES"].alternatives().all(|alternative| matches!(alternative, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::List(_)))));
    }

    #[test]
    fn python_module_evaluation_keeps_mutation_from_loop_body() {
        let source = "VALUES = []\nfor item in ITEMS:\n    VALUES.append(item)\n";
        let (file, facts) = evaluate_source_with_file(source);
        let [mutation] = facts.mutations.as_slice() else {
            panic!("expected one mutation")
        };
        assert_eq!(mutation.binding, "VALUES");
        assert_eq!(mutation.operation, PythonMutationOperation::Append);
        assert!(mutation.path.as_slice().is_empty());
        let text = "VALUES.append(item)";
        assert_eq!(
            mutation.origin,
            Origin::new(
                file,
                Span::saturating_from_parts_usize(
                    source.find(text).expect("text exists"),
                    text.len()
                )
            )
        );
    }

    #[test]
    fn from_dotted_import_named_records_parent_effects_and_selects_member() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "CHILD = 'child'\n"),
            ],
            "from pkg.sub import CHILD\n",
        );
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg.sub"]);
        assert!(
            matches!(&bound(&facts, "CHILD").value.kind, PythonValueKind::Str(text) if text == "child")
        );
    }

    #[test]
    fn from_dotted_import_star_records_parent_effects_and_binds_members() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "CHILD = 'child'\n"),
            ],
            "from pkg.sub import *\n",
        );
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg.sub"]);
        assert!(
            matches!(&bound(&facts, "CHILD").value.kind, PythonValueKind::Str(text) if text == "child")
        );
        assert!(!facts.bindings.contains_key("PARENT"));
    }

    #[test]
    fn exact_all_non_child_spellings_stay_missing_without_descendant_lookup() {
        let (_, facts, trace) = evaluate_imports(
            &[
                (
                    "/project/pkg/__init__.py",
                    "__all__ = ['bad-name', 'child.grandchild']\n",
                ),
                ("/project/pkg/child/__init__.py", ""),
                ("/project/pkg/child/grandchild.py", ""),
            ],
            "from pkg import *\n",
        );
        assert_eq!(import_names(&trace), ["resolved:pkg"]);
        for member in ["bad-name", "child.grandchild"] {
            assert!(
                matches!(&bound(&facts, member).value.kind, PythonValueKind::Unknown(unknown)
                if matches!(&unknown.cause, PythonUnknownCause::MissingImportMember { module, member: missing } if module.as_str() == "pkg" && missing == member))
            );
        }
    }

    #[test]
    fn exact_tuple_all_selects_only_its_named_member() {
        let (_, facts, _) = evaluate_imports(
            &[(
                "/project/plugin.py",
                "__all__ = ('SELECTED',)\nSELECTED = 'selected'\nOMITTED = 'omitted'\n",
            )],
            "from plugin import *\n",
        );
        let selected = facts.bindings["SELECTED"]
            .alternatives()
            .filter_map(|state| {
                let PythonBindingState::Bound(bound) = state else {
                    return None;
                };
                let PythonValueKind::Str(value) = &bound.value.kind else {
                    return None;
                };
                Some(value.as_str())
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(selected, ["selected"].into_iter().collect());
        assert!(!facts.bindings.contains_key("OMITTED"));
    }

    #[test]
    fn from_dotted_import_not_found_records_parent_prefix_and_fails_selection() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "PARENT = 'parent'\n")],
            "from pkg.missing import VALUE\n",
        );
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "notfound:pkg.missing"]
        );
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "pkg.missing"))
        );
    }

    #[test]
    fn from_dotted_import_not_found_intermediate_records_root_prefix_and_fails_selection() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "PARENT = 'parent'\n")],
            "from pkg.missing.leaf import VALUE\n",
        );
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "notfound:pkg.missing.leaf"]
        );
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "pkg.missing.leaf"))
        );
    }

    #[test]
    fn from_dotted_import_namespace_parent_selects_source_child_member() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/nspkg/mod.py", "VALUE = 'child'\n")],
            "from nspkg.mod import VALUE\n",
        );
        assert_eq!(import_names(&trace), ["resolved:nspkg.mod"]);
        assert!(
            matches!(&bound(&facts, "VALUE").value.kind, PythonValueKind::Str(text) if text == "child")
        );
    }

    #[test]
    fn ordinary_import_single_alias_binds_module_and_omits_source_name() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/a/__init__.py", "VALUE = 'a'\n")],
            "import a as x\n",
        );
        assert_eq!(module_name(&facts, "x"), ("a", false));
        assert!(!facts.bindings.contains_key("a"));
        assert_eq!(import_names(&trace), ["resolved:a"]);
    }

    #[test]
    fn member_read_of_intrinsic_binding_after_bare_import_selects_value() {
        let (_, facts, _) = evaluate_imports(
            &[("/project/pkg/__init__.py", "VALUE = 'pkg'\n")],
            "import pkg\nATTR = pkg.VALUE\n",
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert!(
            matches!(&bound(&facts, "ATTR").value.kind, PythonValueKind::Str(text) if text == "pkg")
        );
    }

    #[test]
    fn member_read_of_parent_init_binding_after_dotted_root_import_selects_value() {
        let (_, facts, _) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg.sub\nATTR = pkg.PARENT\n",
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert!(
            matches!(&bound(&facts, "ATTR").value.kind, PythonValueKind::Str(text) if text == "parent")
        );
    }

    #[test]
    fn member_read_of_leaf_binding_through_dotted_alias_selects_value() {
        let (_, facts, _) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg.sub as s\nATTR = s.LEAF\n",
        );
        assert_eq!(module_name(&facts, "s"), ("pkg.sub", false));
        assert!(
            matches!(&bound(&facts, "ATTR").value.kind, PythonValueKind::Str(text) if text == "leaf")
        );
    }

    #[test]
    fn ordinary_dotted_import_source_parent_binds_namespace_leaf_child() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
                ("/project/pkg/ns/mod.py", "X = 'x'\n"),
            ],
            "import pkg.ns\nCHILD = pkg.ns\n",
        );
        assert_eq!(import_names(&trace), ["resolved:pkg"]);
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(module_name(&facts, "CHILD"), ("pkg.ns", true));
    }

    #[test]
    fn ordinary_dotted_import_namespace_intermediate_reaches_source_leaf() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/ns/leaf.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg.ns.leaf\nCHILD = pkg.ns.leaf\n",
        );
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "resolved:pkg.ns.leaf"]
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(module_name(&facts, "CHILD"), ("pkg.ns.leaf", false));
    }

    #[test]
    fn ordinary_import_binds_multiple_clauses_in_source_order() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/alpha/__init__.py", ""),
                ("/project/beta/__init__.py", ""),
            ],
            "import alpha, beta\n",
        );
        assert_eq!(import_names(&trace), ["resolved:alpha", "resolved:beta"]);
        assert_eq!(module_name(&facts, "alpha"), ("alpha", false));
        assert_eq!(module_name(&facts, "beta"), ("beta", false));
    }

    #[test]
    fn ordinary_import_mixed_clauses_preserve_source_order_after_failure() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/alpha/__init__.py", "")],
            "import missing, alpha\n",
        );
        assert_eq!(import_names(&trace), ["notfound:missing", "resolved:alpha"]);
        assert!(
            matches!(&bound(&facts, "missing").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing"))
        );
        assert_eq!(module_name(&facts, "alpha"), ("alpha", false));
    }

    #[test]
    fn ordinary_import_binds_source_module_and_records_edge() {
        let (file, facts, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "VALUE = 'pkg'\n")],
            "import pkg\n",
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(import_names(&trace), ["resolved:pkg"]);
        assert!(trace.files().any(|dependency| dependency == file));
    }

    #[test]
    fn ordinary_import_alias_binds_leaf_and_evaluates_parent() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "ROOT = 'root'\n"),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg.sub as s\n",
        );
        assert_eq!(module_name(&facts, "s"), ("pkg.sub", false));
        assert!(!facts.bindings.contains_key("pkg"));
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg.sub"]);
    }

    #[test]
    fn ordinary_dotted_import_binds_root_and_attaches_loaded_child() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg.sub\nCHILD = pkg.sub\n",
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(module_name(&facts, "CHILD"), ("pkg.sub", false));
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg.sub"]);
    }

    #[test]
    fn ordinary_import_of_namespace_parent_binds_namespace_and_loads_child() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/nspkg/mod.py", "LEAF = 'leaf'\n")],
            "import nspkg.mod\nCHILD = nspkg.mod\n",
        );
        assert_eq!(module_name(&facts, "nspkg"), ("nspkg", true));
        assert_eq!(module_name(&facts, "CHILD"), ("nspkg.mod", false));
        assert_eq!(import_names(&trace), ["resolved:nspkg.mod"]);
    }

    #[test]
    fn ordinary_import_preserves_prior_clause_effects_on_later_failure() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/good/__init__.py", "")],
            "import good\nimport bad\n",
        );
        assert_eq!(module_name(&facts, "good"), ("good", false));
        assert!(
            matches!(&bound(&facts, "bad").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "bad"))
        );
        assert_eq!(import_names(&trace), ["resolved:good", "notfound:bad"]);
    }

    #[test]
    fn ordinary_import_in_false_branch_creates_no_effects() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "")],
            "if False:\n    import pkg\nMARK = 'kept'\n",
        );
        assert!(!facts.bindings.contains_key("pkg"));
        assert_eq!(trace.imports().len(), 0);
    }

    #[test]
    fn ordinary_dotted_import_of_package_leaf_attaches_package_child() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/sub/__init__.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg.sub\nCHILD = pkg.sub\n",
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(module_name(&facts, "CHILD"), ("pkg.sub", false));
        assert_eq!(import_names(&trace), ["resolved:pkg", "resolved:pkg.sub"]);
    }

    #[test]
    fn ordinary_import_of_namespace_terminal_binds_namespace_without_edges() {
        let (_, facts, trace) =
            evaluate_imports(&[("/project/ns/placeholder.py", "")], "import ns\n");
        assert_eq!(module_name(&facts, "ns"), ("ns", true));
        assert_eq!(trace.imports().len(), 0);
    }

    #[test]
    fn ordinary_dotted_import_missing_leaf_preserves_parent_edge() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "VALUE = 'pkg'\n")],
            "import pkg.missing\n",
        );
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "notfound:pkg.missing"]
        );
        assert!(
            matches!(&bound(&facts, "pkg").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ImportNotFound(module) if module.as_str() == "pkg.missing"))
        );
    }

    #[test]
    fn ordinary_dotted_import_missing_intermediate_preserves_root_edge() {
        let (_, _, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "")],
            "import pkg.missing.leaf\n",
        );
        assert_eq!(
            import_names(&trace),
            ["resolved:pkg", "notfound:pkg.missing.leaf"]
        );
    }

    #[test]
    fn ordinary_dotted_import_recovers_parent_syntax_and_continues() {
        let (_, facts, trace) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", "VALUE = 'ok'\nbroken(\n"),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg.sub\nCHILD = pkg.sub\n",
        );
        assert!(trace.imports().any(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::SyntaxErrors(_) } if edge.imported.name().as_str() == "pkg")));
        assert!(trace.imports().any(|outcome| matches!(outcome, PythonImportOutcome::Evaluated { edge, status: PythonImportEvaluationStatus::Resolved } if edge.imported.name().as_str() == "pkg.sub")));
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(module_name(&facts, "CHILD"), ("pkg.sub", false));
    }

    #[test]
    fn member_read_of_unimported_child_is_module_attribute_unknown() {
        let (_, facts, _) = evaluate_imports(
            &[
                ("/project/pkg/__init__.py", ""),
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
            ],
            "import pkg\nCHILD = pkg.sub\n",
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert!(
            matches!(&bound(&facts, "CHILD").value.kind, PythonValueKind::Unknown(unknown) if matches!(&unknown.cause, PythonUnknownCause::ModuleAttribute { module, member } if module.as_str() == "pkg" && member == "sub"))
        );
    }

    #[test]
    fn ordinary_dotted_import_records_root_to_leaf_order_under_reverse_registration() {
        let (_, _, trace) = evaluate_imports(
            &[
                ("/project/pkg/sub.py", "LEAF = 'leaf'\n"),
                ("/project/pkg/__init__.py", "PARENT = 'parent'\n"),
            ],
            "import pkg.sub\n",
        );
        let resolved = trace
            .imports()
            .map(|outcome| {
                let PythonImportOutcome::Evaluated {
                    edge,
                    status: PythonImportEvaluationStatus::Resolved,
                } = outcome
                else {
                    panic!("unexpected outcome: {outcome:?}")
                };
                (
                    edge.importer.name().as_str().to_owned(),
                    edge.imported.name().as_str().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            resolved,
            [
                ("settings".to_owned(), "pkg".to_owned()),
                ("settings".to_owned(), "pkg.sub".to_owned())
            ]
        );
    }

    #[test]
    fn ambiguous_dotted_import_preserves_module_and_unbound_alternatives() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "")],
            "if condition:\n    import pkg\nMARK = 'kept'\n",
        );
        let alternatives = facts.bindings["pkg"].alternatives().collect::<Vec<_>>();
        assert!(alternatives.iter().any(|alternative| matches!(alternative, PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Module(PythonModule::Source(module)) if module.name().as_str() == "pkg"))));
        assert!(
            alternatives
                .iter()
                .any(|alternative| matches!(alternative, PythonBindingState::Unbound))
        );
        assert_eq!(import_names(&trace), ["resolved:pkg"]);
    }

    #[test]
    fn deterministically_true_dotted_import_binds_module_unconditionally() {
        let (_, facts, trace) = evaluate_imports(
            &[("/project/pkg/__init__.py", "")],
            "if True:\n    import pkg\n",
        );
        assert_eq!(module_name(&facts, "pkg"), ("pkg", false));
        assert_eq!(import_names(&trace), ["resolved:pkg"]);
    }

    #[test]
    fn ambiguous_branch_omits_annotation_and_dead_write() {
        for body in [
            "if FLAG:\n    VALUE: str\n",
            "if FLAG:\n    if False:\n        VALUE = 'dead'\n",
        ] {
            let facts = evaluate_source(&format!("CONTROL = 'present'\n{body}"));
            assert!(!facts.bindings.contains_key("VALUE"), "{body}");
            assert!(facts.bindings.contains_key("CONTROL"), "{body}");
        }
    }

    #[test]
    fn python_binding_alternative_limit_has_exact_boundary_and_unknown_remainder() {
        for (count, expected_len, overflow) in [(63, 64, false), (64, 65, true)] {
            let mut source = String::new();
            for index in 0..count {
                if index == 0 {
                    writeln!(source, "if condition_0:").expect("write string");
                } else {
                    writeln!(source, "elif condition_{index}:").expect("write string");
                }
                writeln!(source, "    VALUE = '{index:02}'").expect("write string");
            }
            let facts = evaluate_source(&source);
            let alternatives = facts.bindings["VALUE"].alternatives().collect::<Vec<_>>();
            assert_eq!(alternatives.len(), expected_len);
            assert!(
                alternatives
                    .iter()
                    .any(|alternative| matches!(alternative, PythonBindingState::Unbound))
            );
            assert_eq!(alternatives.iter().any(|alternative| matches!(alternative,
                PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::AlternativeLimitExceeded))), overflow);
        }
    }

    #[test]
    fn python_evaluator_produces_unbound_for_a_path_without_assignment() {
        let source = "if condition:\n    VALUE = 'set'\n";
        let (file, facts) = evaluate_source_with_file(source);
        let alternatives = facts.bindings["VALUE"].alternatives().collect::<Vec<_>>();
        assert_eq!(alternatives.len(), 2);
        assert!(
            alternatives
                .iter()
                .any(|alternative| matches!(alternative, PythonBindingState::Unbound))
        );
        let text = "'set'";
        let origin = Origin::new(
            file,
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len()),
        );
        assert!(alternatives.iter().any(|alternative| matches!(alternative,
            PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Str(value) if value == "set")
                && bound.value.origins().collect::<Vec<_>>() == [origin]
                && bound.binding_origins().collect::<Vec<_>>() == [origin])));
    }

    #[test]
    fn ambiguous_branch_same_value_reassignment_preserves_origins() {
        let source = "VALUE = 'same'\nif FLAG:\n    VALUE = 'same'\n";
        let (file, facts) = evaluate_source_with_file(source);
        let alternatives = facts.bindings["VALUE"].alternatives().collect::<Vec<_>>();
        let [PythonBindingState::Bound(bound)] = alternatives.as_slice() else {
            panic!("one bound alternative expected")
        };
        let expected = source
            .match_indices("'same'")
            .map(|(start, value)| {
                Origin::new(file, Span::saturating_from_parts_usize(start, value.len()))
            })
            .collect::<Vec<_>>();
        assert_eq!(bound.value.origins().collect::<Vec<_>>(), expected);
        assert_eq!(bound.binding_origins().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn ambiguous_branch_restoration_preserves_only_base_and_restoration_origins() {
        let source = "VALUE = 'base'\nif FLAG:\n    VALUE = 'temporary'\n    VALUE = 'base'\n";
        let (file, facts) = evaluate_source_with_file(source);
        let alternatives = facts.bindings["VALUE"].alternatives().collect::<Vec<_>>();
        let [PythonBindingState::Bound(bound)] = alternatives.as_slice() else {
            panic!("one bound alternative expected")
        };
        let expected = source
            .match_indices("'base'")
            .map(|(start, value)| {
                Origin::new(file, Span::saturating_from_parts_usize(start, value.len()))
            })
            .collect::<Vec<_>>();
        assert_eq!(bound.value.origins().collect::<Vec<_>>(), expected);
        assert_eq!(bound.binding_origins().collect::<Vec<_>>(), expected);
        let temporary = Span::saturating_from_parts_usize(
            source.find("'temporary'").expect("text exists"),
            "'temporary'".len(),
        );
        assert!(bound.value.origins().all(|origin| origin.span != temporary));
    }

    #[test]
    fn ambiguous_branch_append_remove_retains_mutation_evidence() {
        let source =
            "VALUES = []\nif FLAG:\n    VALUES.append('item')\n    VALUES.remove('item')\n";
        let (_, facts) = evaluate_source_with_file(source);
        let alternatives = facts.bindings["VALUES"].alternatives().collect::<Vec<_>>();
        let [PythonBindingState::Bound(bound)] = alternatives.as_slice() else {
            panic!("one bound alternative expected")
        };
        let PythonValueKind::List(items) = &bound.value.kind else {
            panic!("list expected")
        };
        assert!(items.semantic_items().is_empty());
        assert_eq!(bound.value.origins().len(), 3);
        assert_eq!(facts.mutations.as_slice().len(), 2);
        for (operation, text) in [
            (PythonMutationOperation::Append, "VALUES.append('item')"),
            (PythonMutationOperation::Remove, "VALUES.remove('item')"),
        ] {
            let span = Span::saturating_from_parts_usize(
                source.find(text).expect("text exists"),
                text.len(),
            );
            assert!(
                facts
                    .mutations
                    .iter()
                    .any(|mutation| mutation.binding == "VALUES"
                        && mutation.operation == operation
                        && mutation.origin.span == span)
            );
        }
    }

    #[test]
    fn ambiguous_branch_reassignment_clears_prior_mutation_evidence() {
        let source = "VALUES = []\nif FLAG:\n    VALUES.append('stale')\n    VALUES = []\n";
        let facts = evaluate_source(source);
        let alternatives = facts.bindings["VALUES"].alternatives().collect::<Vec<_>>();
        let [PythonBindingState::Bound(bound)] = alternatives.as_slice() else {
            panic!("one bound alternative expected")
        };
        assert!(facts.mutations.as_slice().is_empty());
        assert_eq!(bound.value.origins().len(), 2);
        let text = "VALUES.append('stale')";
        let span =
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len());
        assert!(bound.value.origins().all(|origin| origin.span != span));
    }

    #[test]
    fn repeated_tuple_alias_invalidation_retains_augmented_add_fact() {
        let facts = evaluate_source("INNER = []\nROOT = (INNER, INNER)\nROOT[0] += ['x']\n");
        assert!(matches!(facts.mutations.as_slice(), [mutation]
            if mutation.binding == "ROOT" && mutation.operation == PythonMutationOperation::Extend
                && mutation.path.as_slice() == [PythonMutationPathSegment::Index(0)]));
        let alternatives = facts.bindings["ROOT"].alternatives().collect::<Vec<_>>();
        assert!(
            matches!(alternatives.as_slice(), [PythonBindingState::Bound(bound)]
            if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression))
        );
    }

    #[test]
    fn successful_augmented_add_preserves_rhs_only_binding() {
        let facts = evaluate_source("SRC = ['x']\nVALUES = []\nVALUES += SRC\n");
        for name in ["SRC", "VALUES"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            let [PythonBindingState::Bound(bound)] = alternatives.as_slice() else {
                panic!("one bound alternative expected")
            };
            let PythonValueKind::List(list) = &bound.value.kind else {
                panic!("list expected")
            };
            assert!(
                matches!(list.semantic_items(), [PythonSequenceItem::Value(value)] if matches!(&value.kind, PythonValueKind::Str(text) if text == "x"))
            );
        }
    }

    #[test]
    fn ambiguous_branch_noop_extend_retains_mutation_evidence() {
        let source = "VALUES = []\nif FLAG:\n    VALUES.extend([])\n";
        let facts = evaluate_source(source);
        let alternatives = facts.bindings["VALUES"].alternatives().collect::<Vec<_>>();
        let [PythonBindingState::Bound(bound)] = alternatives.as_slice() else {
            panic!("one bound alternative expected")
        };
        assert_eq!(bound.value.origins().len(), 2);
        let text = "VALUES.extend([])";
        let span =
            Span::saturating_from_parts_usize(source.find(text).expect("text exists"), text.len());
        assert!(matches!(facts.mutations.as_slice(), [mutation]
            if mutation.binding == "VALUES" && mutation.operation == PythonMutationOperation::Extend && mutation.origin.span == span));
    }

    #[test]
    fn chained_immutable_assignment_remains_exact() {
        let facts = evaluate_source("VALUE = ALIAS = '/static/'");
        for name in ["VALUE", "ALIAS"] {
            let binding = facts.bindings.get(name).expect("binding should exist");
            let alternatives = binding.alternatives().collect::<Vec<_>>();
            assert!(matches!(alternatives.as_slice(),
                [PythonBindingState::Bound(bound)]
                    if matches!(&bound.value.kind, PythonValueKind::Str(value) if value == "/static/")
            ));
        }
    }

    #[test]
    fn binary_string_plus_string_is_exact() {
        let facts = evaluate_source("VALUE = '/sta' + 'tic/'");
        let binding = facts.bindings.get("VALUE").expect("binding should exist");
        let alternatives = binding.alternatives().collect::<Vec<_>>();
        assert!(matches!(alternatives.as_slice(),
            [PythonBindingState::Bound(bound)]
                if matches!(&bound.value.kind, PythonValueKind::Str(value) if value == "/static/")
        ));
    }

    #[test]
    fn name_target_string_augmented_add_is_exact_or_degrades() {
        let exact = evaluate_source("VALUE = '/sta'\nVALUE += 'tic/'");
        let binding = exact.bindings.get("VALUE").expect("binding should exist");
        let alternatives = binding.alternatives().collect::<Vec<_>>();
        assert!(matches!(alternatives.as_slice(),
            [PythonBindingState::Bound(bound)]
                if matches!(&bound.value.kind, PythonValueKind::Str(value) if value == "/static/")
        ));

        for source in [
            "VALUE = '/static/'\nVALUE += EXTRA",
            "from pathlib import Path\nVALUE = '/static/'\nVALUE += Path(__file__)",
        ] {
            let degraded = evaluate_source(source);
            let binding = degraded
                .bindings
                .get("VALUE")
                .expect("binding should exist");
            assert!(binding.alternatives().any(|alternative| matches!(
                alternative,
                PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
            )), "{degraded:#?}");
        }
    }

    #[test]
    fn rejected_nested_dictionary_assignment_invalidates_possible_aliases() {
        for source in [
            "OPTIONS = {}\nALIAS = OPTIONS\nWRAPPER = {'OPTIONS': OPTIONS, **UNKNOWN}\nWRAPPER['OPTIONS']['loaders'] = ('loader',)\n",
            "OPTIONS = {}\nALIAS = OPTIONS\nif FLAG:\n    LOADERS = []\nelse:\n    LOADERS = ['cached']\nOPTIONS['loaders'] = LOADERS\n",
            "LEFT = {}\nRIGHT = {}\nALIAS = LEFT\nif FLAG:\n    ROOT = LEFT\nelse:\n    ROOT = RIGHT\nROOT['loaders'] = []\n",
        ] {
            let facts = evaluate_source(source);
            let binding = facts.bindings.get("ALIAS").expect("ALIAS should be bound");
            assert!(binding.alternatives().any(|alternative| matches!(
                alternative,
                PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
            )), "{source}");
        }
    }

    #[test]
    fn recursive_mutations_degrade_unrepresentable_values() {
        for (source, names) in [
            ("ROOT = {}\nROOT['self'] = ROOT\n", &["ROOT"][..]),
            (
                "CHILD = {}\nROOT = {'child': CHILD}\nROOT['child']['parent'] = ROOT\n",
                &["CHILD", "ROOT"],
            ),
            (
                "ROOT = {'child': []}\nROOT['child'].append(ROOT)\n",
                &["ROOT"],
            ),
            (
                "ROOT = {'child': []}\nROOT['child'].insert(0, ROOT)\n",
                &["ROOT"],
            ),
            (
                "ROOT = {'child': []}\nROOT['child'].extend([ROOT])\n",
                &["ROOT"],
            ),
            ("ROOT = []\nROOT += [ROOT]\n", &["ROOT"]),
        ] {
            let facts = evaluate_source(source);
            for name in names {
                let binding = facts.bindings.get(*name).expect("binding should exist");
                assert!(binding.alternatives().any(|alternative| matches!(
                    alternative,
                    PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
                )), "{source}: {name}");
            }
        }
    }

    #[test]
    fn name_target_list_augmented_add_failure_rebinds_and_clears_prior_facts() {
        let facts = evaluate_source("VALUES = []\nVALUES.append('a')\nVALUES += True\n");
        assert!(facts.bindings["VALUES"].alternatives().any(|alternative| matches!(
            alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
        )));
        let operations = facts
            .mutations
            .iter()
            .filter(|mutation| mutation.binding == "VALUES")
            .map(|mutation| mutation.operation)
            .collect::<Vec<_>>();
        assert_eq!(operations, vec![PythonMutationOperation::Extend]);
    }

    #[test]
    fn augmented_add_and_extend_bool_failures_differ_on_mutation_facts() {
        let augmented = evaluate_source("VALUES = []\nVALUES += True\n");
        assert!(augmented.bindings["VALUES"].alternatives().any(|alternative| matches!(
            alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
        )));
        assert!(
            augmented
                .mutations
                .iter()
                .any(|mutation| mutation.binding == "VALUES"
                    && mutation.operation == PythonMutationOperation::Extend)
        );

        let extended = evaluate_source("FLAG = True\nVALUES = []\nVALUES.extend(FLAG)\n");
        assert!(extended.bindings["VALUES"].alternatives().any(|alternative| matches!(
            alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
        )));
        assert!(extended.mutations.as_slice().is_empty());
        assert!(extended.bindings["FLAG"].alternatives().any(|alternative| matches!(
            alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
        )));
    }

    #[test]
    fn failed_extend_replaces_root_but_only_joins_rhs_degradation() {
        let facts = evaluate_source("FLAG = True\nVALUES = []\nVALUES.extend(FLAG)\n");
        let values = facts.bindings["VALUES"].alternatives().collect::<Vec<_>>();
        assert!(
            matches!(values.as_slice(), [PythonBindingState::Bound(bound)]
            if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedMutation))
        );
        let flag = facts.bindings["FLAG"].alternatives().collect::<Vec<_>>();
        assert_eq!(flag.len(), 2);
        assert!(flag.iter().any(|alternative| matches!(alternative,
            PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Bool(true)))));
        assert!(flag.iter().any(|alternative| matches!(alternative,
            PythonBindingState::Bound(bound) if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedMutation))));
        assert!(facts.mutations.as_slice().is_empty());
    }

    #[test]
    fn failed_self_aliasing_name_augmented_add_keeps_direct_target_cause() {
        let facts = evaluate_source("VALUES = []\nVALUES.append(VALUES)\nVALUES += True\n");
        let values = facts.bindings["VALUES"].alternatives().collect::<Vec<_>>();
        assert!(
            matches!(values.as_slice(), [PythonBindingState::Bound(bound)]
            if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression))
        );
        assert!(
            matches!(facts.mutations.as_slice(), [mutation] if mutation.operation == PythonMutationOperation::Extend)
        );
    }

    #[test]
    fn failed_name_augmented_add_preserves_rhs_only_binding_exactly() {
        let facts = evaluate_source("FLAG = True\nVALUES = []\nVALUES += FLAG\n");
        let values = facts.bindings["VALUES"].alternatives().collect::<Vec<_>>();
        assert!(
            matches!(values.as_slice(), [PythonBindingState::Bound(bound)]
            if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedExpression))
        );
        let flag = facts.bindings["FLAG"].alternatives().collect::<Vec<_>>();
        assert!(
            matches!(flag.as_slice(), [PythonBindingState::Bound(bound)] if matches!(bound.value.kind, PythonValueKind::Bool(true)))
        );
        assert!(
            matches!(facts.mutations.as_slice(), [mutation] if mutation.operation == PythonMutationOperation::Extend)
        );
    }

    #[test]
    fn failed_extend_alias_rhs_degradation_takes_precedence_over_preservation() {
        let facts = evaluate_source("ROOT = {}\nALIAS = ROOT\nROOT.extend(ALIAS)\n");
        for name in ["ROOT", "ALIAS"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(
                matches!(alternatives.as_slice(), [PythonBindingState::Bound(bound)]
                if matches!(&bound.value.kind, PythonValueKind::Unknown(unknown) if unknown.cause == PythonUnknownCause::UnsupportedMutation))
            );
        }
        assert!(facts.mutations.as_slice().is_empty());
    }

    #[test]
    fn name_target_augmented_add_unsupported_receiver_categories_fallback() {
        for receiver in ["{}", "Path('/x')", "True", "MISSING"] {
            let source =
                format!("from pathlib import Path\nTARGET = {receiver}\nTARGET += ['a']\n");
            let facts = evaluate_source(&source);
            assert!(facts.bindings["TARGET"].alternatives().any(|alternative| matches!(
                alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
            )), "receiver `{receiver}`");
            assert!(
                facts
                    .mutations
                    .iter()
                    .any(|mutation| mutation.binding == "TARGET"
                        && mutation.operation == PythonMutationOperation::Extend),
                "receiver `{receiver}`"
            );
        }
        let facts = evaluate_source("D = {}\nALIAS = D\nD += ['a']\n");
        for name in ["D", "ALIAS"] {
            assert!(facts.bindings[name].alternatives().any(|alternative| matches!(
                alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
            )), "{name}");
        }
    }

    #[test]
    fn nested_augmented_add_non_list_target_categories_fallback() {
        for source in [
            "ROOT = ['x']\nROOT[0] += ['a']\n",
            "ROOT = ((),)\nROOT[0] += ['a']\n",
            "ROOT = ({},)\nROOT[0] += ['a']\n",
            "from pathlib import Path\nROOT = (Path('/x'),)\nROOT[0] += ['a']\n",
            "ROOT = (True,)\nROOT[0] += ['a']\n",
            "ROOT = (MISSING,)\nROOT[0] += ['a']\n",
        ] {
            let facts = evaluate_source(source);
            assert!(facts.bindings["ROOT"].alternatives().any(|alternative| matches!(
                alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
            )), "{source}");
            assert!(
                facts
                    .mutations
                    .iter()
                    .any(|mutation| mutation.binding == "ROOT"
                        && mutation.operation == PythonMutationOperation::Extend),
                "{source}"
            );
        }
    }

    #[test]
    fn extend_non_list_receiver_categories_fallback() {
        for receiver in ["()", "'text'", "{}", "Path('/x')", "True", "MISSING"] {
            let source = format!(
                "from pathlib import Path\nRECEIVER = {receiver}\nRECEIVER.extend(['a'])\n"
            );
            let facts = evaluate_source(&source);
            assert!(facts.bindings["RECEIVER"].alternatives().any(|alternative| matches!(
                alternative, PythonBindingState::Bound(bound) if matches!(bound.value.kind, PythonValueKind::Unknown(_))
            )), "receiver `{receiver}`");
            assert!(
                facts.mutations.as_slice().is_empty(),
                "receiver `{receiver}`"
            );
        }
    }

    #[test]
    fn ambiguous_branch_mutations_remain_uncorrelated_may_have_evidence() {
        let facts = evaluate_source(
            "VALUES = []\nif FLAG:\n    VALUES.append('item')\nelse:\n    VALUES.extend([])\n",
        );
        let mut lengths = facts.bindings["VALUES"]
            .alternatives()
            .map(|alternative| {
                let PythonBindingState::Bound(bound) = alternative else {
                    panic!("both branches should retain exact list values")
                };
                let PythonValueKind::List(items) = &bound.value.kind else {
                    panic!("both branches should retain exact list values")
                };
                items.semantic_items().len()
            })
            .collect::<Vec<_>>();
        lengths.sort_unstable();
        assert_eq!(lengths, [0, 1]);
        assert_eq!(facts.mutations.as_slice().len(), 2);
        assert!(
            facts
                .mutations
                .iter()
                .any(|mutation| mutation.operation == PythonMutationOperation::Append)
        );
        assert!(
            facts
                .mutations
                .iter()
                .any(|mutation| mutation.operation == PythonMutationOperation::Extend)
        );
    }

    #[test]
    fn ambiguous_match_joins_only_evaluated_pattern_and_body_names() {
        let facts = evaluate_source(
            "match SUBJECT:\n    case {'item': ITEM} if GUARD:\n        VALUE = 'matched'\n        DEAD: str\n    case _:\n        FALLBACK = 'fallback'\n",
        );
        for name in ["ITEM", "VALUE", "FALLBACK"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert_eq!(alternatives.len(), 2, "{name}");
            assert!(
                alternatives
                    .iter()
                    .any(|alternative| matches!(alternative, PythonBindingState::Unbound)),
                "{name}"
            );
            assert!(
                alternatives
                    .iter()
                    .any(|alternative| matches!(alternative, PythonBindingState::Bound(_))),
                "{name}"
            );
        }
        assert!(!facts.bindings.contains_key("DEAD"));
    }

    #[test]
    fn python_module_evaluation_canonicalizes_branch_mutation_order() {
        let source = "A_VALUES = []\nZ_VALUES = []\nif FLAG:\n    Z_VALUES.append('z')\nelse:\n    A_VALUES.append('a')\n";
        let db = TestDatabase::new();
        let project =
            crate::project::Project::initial(&db, Utf8Path::new("/project"), &Settings::default());
        db.add_file("/project/settings.py", source)
            .expect("fixture should be writable");
        let module = crate::python::file_to_module(&db, project, "/project/settings.py".into())
            .expect("module should resolve");
        let facts = crate::python::evaluation::python_module_facts(
            &db,
            project,
            module,
            crate::python::evaluation::EvaluationDemand::Full,
        )
        .as_ref()
        .expect("module should evaluate");
        let mutations = facts.mutations.as_slice();
        assert_eq!(
            mutations
                .iter()
                .map(|mutation| mutation.binding.as_str())
                .collect::<Vec<_>>(),
            ["A_VALUES", "Z_VALUES"]
        );
        for (mutation, text) in mutations
            .iter()
            .zip(["A_VALUES.append('a')", "Z_VALUES.append('z')"])
        {
            assert_eq!(
                mutation.origin.span,
                Span::saturating_from_parts_usize(
                    source.find(text).expect("text exists"),
                    text.len()
                )
            );
        }
    }

    #[test]
    fn closed_unsupported_literals_are_concrete_unsupported_values() {
        let facts = evaluate_source(
            "NONE = None\nNUMBER = 1\nNEGATIVE = -1\nBYTES = b'x'\nELLIPSIS = ...\nSET = {1}\nVALUES = [None, 1, b'x', ..., {1}]\n",
        );
        for name in ["NONE", "NUMBER", "NEGATIVE", "BYTES", "ELLIPSIS", "SET"] {
            let alternatives = facts.bindings[name].alternatives().collect::<Vec<_>>();
            assert!(
                matches!(alternatives.as_slice(), [PythonBindingState::Bound(bound)] if matches!(bound.value.kind, PythonValueKind::UnsupportedLiteral))
            );
        }
        let alternatives = facts.bindings["VALUES"].alternatives().collect::<Vec<_>>();
        let [PythonBindingState::Bound(bound)] = alternatives.as_slice() else {
            panic!("VALUES should be bound")
        };
        let PythonValueKind::List(items) = &bound.value.kind else {
            panic!("VALUES should be a list")
        };
        assert_eq!(items.semantic_items().len(), 5);
        assert!(items.semantic_items().iter().all(|item| matches!(item, PythonSequenceItem::Value(value) if matches!(value.kind, PythonValueKind::UnsupportedLiteral))));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "keep multiline Python fixtures inline"
    )]
    fn settings_slice_preserves_effects_and_alias_certificates() {
        for (prefix, omitted, contaminated) in [
            (
                r"import pathlib
UNUSED = {'result': opaque(pathlib)}
",
                true,
                true,
            ),
            // The early scalar passes inspection, but the later alias must keep the binding.
            (
                r"import pathlib
UNUSED = {'scalar': 'first', 'alias': pathlib.Path}
unknown().attr = 1
",
                false,
                true,
            ),
            (
                r"import pathlib as UNUSED
UNUSED = {'scalar': unknown.attr}
unknown().attr = 1
",
                true,
                true,
            ),
            (
                r"A = ['other']
UNUSED = ['first', A, INSTALLED_APPS]
for item in unknown:
    A.append('changed')
",
                false,
                false,
            ),
            (
                r"UNUSED = [INSTALLED_APPS]
UNUSED = {'scalar': unknown.attr}
for item in unknown:
    INSTALLED_APPS.append('changed')
",
                true,
                false,
            ),
            (
                r"if FLAG:
    A = 'scalar'
else:
    A = INSTALLED_APPS
UNUSED = {'value': A}
opaque(INSTALLED_APPS)
",
                false,
                false,
            ),
        ] {
            let db = TestDatabase::new();
            let project =
                crate::project::Project::initial(&db, Utf8Path::new("/proj"), &Settings::default());
            let source = format!(
                r"INSTALLED_APPS = ['blog']
{prefix}TEMPLATES = []
"
            );
            db.add_file("/proj/settings.py", &source)
                .expect("fixture should be writable");
            let module = crate::python::file_to_module(&db, project, "/proj/settings.py".into())
                .expect("module should resolve");
            let parsed = ruff_python_parser::parse_module(&source)
                .expect("fixture should parse")
                .into_syntax();
            let (full, full_trace, full_effects) = PythonModuleEvaluator::new(
                &db,
                project,
                module.clone(),
                super::IntrinsicContamination::default(),
            )
            .evaluate(&parsed.body, Vec::new(), Vec::new());
            let selected = crate::python::evaluation::slice::selected_statements(
                &parsed.body,
                crate::python::evaluation::EvaluationDemand::Settings,
                false,
            );
            let evaluator = PythonModuleEvaluator::new(
                &db,
                project,
                module,
                super::IntrinsicContamination::default(),
            );
            let stats = Rc::clone(&evaluator.stats);
            let (sliced, trace, effects) = evaluator.evaluate(selected, Vec::new(), Vec::new());
            assert_eq!(!sliced.bindings.contains_key("UNUSED"), omitted, "{source}");
            if omitted {
                assert_eq!(stats.materialized_dictionaries.get(), 0, "{source}");
                assert!(
                    sliced
                        .mutations
                        .iter()
                        .all(|mutation| mutation.binding != "UNUSED")
                );
            }
            assert_eq!(
                effects.namespace_is_contaminated(crate::python::PythonIntrinsicNamespace::Pathlib),
                contaminated,
                "{source}"
            );
            for name in ["INSTALLED_APPS", "TEMPLATES"] {
                assert_eq!(
                    sliced.bindings.get(name),
                    full.bindings.get(name),
                    "{source}"
                );
            }
            assert_eq!(trace, full_trace);
            assert_eq!(effects, full_effects);
        }
    }

    #[test]
    fn settings_slice_avoids_unobserved_dictionary_materialization() {
        for demanded in [false, true] {
            let db = TestDatabase::new();
            let project =
                crate::project::Project::initial(&db, Utf8Path::new("/proj"), &Settings::default());
            let source = format!(
                r"import constants as messages
MESSAGE_TAGS = {{messages.INFO: 'info', messages.ERROR: 'error', messages.WARNING: 'warning', messages.SUCCESS: 'success'}}
INSTALLED_APPS = ['blog']
TEMPLATES = {}
",
                if demanded { "[MESSAGE_TAGS]" } else { "[]" }
            );
            db.add_file(
                "/proj/constants.py",
                r"if A:
    INFO = 'i'
else:
    INFO = 'j'
if B:
    ERROR = 'e'
else:
    ERROR = 'f'
if C:
    WARNING = 'w'
else:
    WARNING = 'x'
if D:
    SUCCESS = 's'
else:
    SUCCESS = 't'
",
            )
            .expect("fixture should be writable");
            db.add_file("/proj/settings.py", &source)
                .expect("fixture should be writable");
            let module = crate::python::file_to_module(&db, project, "/proj/settings.py".into())
                .expect("module should resolve");
            let parsed = ruff_python_parser::parse_module(&source)
                .expect("fixture should parse")
                .into_syntax();
            let evaluator = PythonModuleEvaluator::new(
                &db,
                project,
                module.clone(),
                super::IntrinsicContamination::default(),
            );
            let full_stats = Rc::clone(&evaluator.stats);
            let (full, full_trace, full_effects) =
                evaluator.evaluate(&parsed.body, Vec::new(), Vec::new());
            let selected = crate::python::evaluation::slice::selected_statements(
                &parsed.body,
                crate::python::evaluation::EvaluationDemand::Settings,
                false,
            );
            let evaluator = PythonModuleEvaluator::new(
                &db,
                project,
                module,
                super::IntrinsicContamination::default(),
            );
            let sliced_stats = Rc::clone(&evaluator.stats);
            let (sliced, trace, effects) = evaluator.evaluate(selected, Vec::new(), Vec::new());
            assert_eq!(full_stats.materialized_dictionaries.get(), 1);
            assert_eq!(
                sliced_stats.materialized_dictionaries.get(),
                usize::from(demanded)
            );
            assert_eq!(sliced.bindings.contains_key("MESSAGE_TAGS"), demanded);
            for name in ["INSTALLED_APPS", "TEMPLATES"] {
                assert_eq!(sliced.bindings.get(name), full.bindings.get(name));
            }
            assert_eq!(trace, full_trace);
            assert_eq!(effects, full_effects);
            assert_eq!(full.bindings["MESSAGE_TAGS"].alternatives().count(), 16);
        }
    }

    #[test]
    fn settings_slice_does_not_evaluate_irrelevant_guards_or_values() {
        let db = TestDatabase::new();
        let project =
            crate::project::Project::initial(&db, Utf8Path::new("/proj"), &Settings::default());
        let mut source = String::new();
        for index in 0..20 {
            writeln!(
                source,
                r"if UNKNOWN_{index}:
    UNUSED_{index} = ['a', 'b', 'c']
else:
    UNUSED_{index} = ['d', 'e', 'f']"
            )
            .expect("writing fixture source should succeed");
        }
        source.push_str(
            r"INSTALLED_APPS = ['blog']
TEMPLATES = []
",
        );
        db.add_file("/proj/settings.py", &source)
            .expect("fixture should be writable");
        let module = crate::python::file_to_module(&db, project, "/proj/settings.py".into())
            .expect("fixture module should resolve");
        let parsed = ruff_python_parser::parse_module(&source)
            .expect("fixture should parse")
            .into_syntax();
        let evaluator = PythonModuleEvaluator::new(
            &db,
            project,
            module.clone(),
            super::IntrinsicContamination::default(),
        );
        let full_stats = Rc::clone(&evaluator.stats);
        let (full, _, _) = evaluator.evaluate(&parsed.body, Vec::new(), Vec::new());
        let selected = crate::python::evaluation::slice::selected_statements(
            &parsed.body,
            crate::python::evaluation::EvaluationDemand::Settings,
            false,
        );
        assert_eq!(selected.len(), 2);
        let evaluator = PythonModuleEvaluator::new(
            &db,
            project,
            module,
            super::IntrinsicContamination::default(),
        );
        let sliced_stats = Rc::clone(&evaluator.stats);
        let (sliced, _, _) = evaluator.evaluate(selected, Vec::new(), Vec::new());
        assert_eq!(sliced_stats.evaluated_expressions.get(), 3);
        assert_eq!(full_stats.evaluated_expressions.get(), 183);
        for name in ["INSTALLED_APPS", "TEMPLATES"] {
            assert_eq!(sliced.bindings.get(name), full.bindings.get(name));
        }
        assert_eq!(sliced.bindings.len(), 2);
    }

    #[test]
    fn settings_slice_preserves_private_predicate_constraints_and_origins() {
        let db = TestDatabase::new();
        let project =
            crate::project::Project::initial(&db, Utf8Path::new("/proj"), &Settings::default());
        let source = r"FLAG = unknown_flag
UNUSED = ['skip']
APPS = ['initial']
if FLAG:
    APPS = ['left']
else:
    APPS = ['right', 'extra']
INSTALLED_APPS = APPS
APPS = ['too_late']
if not FLAG:
    TEMPLATES = []
else:
    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates'}]
";
        db.add_file("/proj/settings.py", source)
            .expect("fixture should be writable");
        let module = crate::python::file_to_module(&db, project, "/proj/settings.py".into())
            .expect("fixture module should resolve");
        let parsed = ruff_python_parser::parse_module(source)
            .expect("fixture should parse")
            .into_syntax();
        let (full, _, _) = PythonModuleEvaluator::new(
            &db,
            project,
            module.clone(),
            super::IntrinsicContamination::default(),
        )
        .evaluate(&parsed.body, Vec::new(), Vec::new());
        let selected = crate::python::evaluation::slice::selected_statements(
            &parsed.body,
            crate::python::evaluation::EvaluationDemand::Settings,
            false,
        );
        assert_eq!(selected.len(), 5);
        let (sliced, _, _) = PythonModuleEvaluator::new(
            &db,
            project,
            module,
            super::IntrinsicContamination::default(),
        )
        .evaluate(selected, Vec::new(), Vec::new());
        for name in ["INSTALLED_APPS", "TEMPLATES"] {
            assert_eq!(sliced.bindings.get(name), full.bindings.get(name));
            assert_eq!(sliced.bindings[name].alternatives().count(), 2);
        }
        assert!(!sliced.bindings.contains_key("UNUSED"));
    }

    fn test_file(index: u64) -> File {
        File::from_id(Id::from_bits(index + 1))
    }

    fn origin(start: usize) -> Origin {
        Origin::new(test_file(0), Span::saturating_from_parts_usize(start, 1))
    }

    fn state_with_binding() -> PythonEvaluationState {
        let mut state = PythonEvaluationState::new(test_file(0));
        let binding_origin = origin(1);
        state.bindings.insert(
            "VALUE".to_string(),
            PythonBinding::bound(
                PythonValue::string("value".to_string(), binding_origin),
                binding_origin,
            ),
        );
        state
    }

    fn mutation(operation: PythonMutationOperation, start: usize) -> PythonMutation {
        PythonMutation {
            binding: "VALUE".to_string(),
            path: PythonMutationPath::default(),
            operation,
            origin: origin(start),
        }
    }

    fn namespace_module(name: &str) -> PythonModule {
        PythonModule::Namespace(PythonNamespacePackage::new(
            PythonModuleName::parse(name).expect("test Python module name should be valid"),
            Vec::new(),
        ))
    }

    #[test]
    fn module_valued_bindings_are_uniformly_truthy() {
        let mut state = PythonEvaluationState::new(test_file(0));
        state.bindings.insert(
            "MOD".to_string(),
            PythonBinding::bound(
                PythonValue::module(namespace_module("pkg"), origin(1)),
                origin(1),
            ),
        );
        assert_eq!(
            state.known_truthiness("MOD"),
            Some(Truthiness::Truthy),
            "a uniformly module-valued binding is always truthy",
        );

        let mixed = PythonBinding::bound(
            PythonValue::module(namespace_module("pkg"), origin(1)),
            origin(1),
        )
        .join(
            PythonBinding::unknown(&PythonUnknownCause::Cycle, origin(2)),
            origin(3),
        );
        state.bindings.insert("MIXED".to_string(), mixed);
        assert_eq!(
            state.known_truthiness("MIXED"),
            None,
            "a module mixed with a non-module alternative is not uniformly truthy",
        );

        state.bindings.insert(
            "FLAG".to_string(),
            PythonBinding::bound(PythonValue::bool(true, origin(1)), origin(1)),
        );
        assert_eq!(state.known_truthiness("FLAG"), Some(Truthiness::Truthy));
    }

    #[test]
    fn changed_names_include_rooted_mutation_evidence() {
        let mut base = state_with_binding();
        base.mutations
            .insert(mutation(PythonMutationOperation::Append, 2));
        let mut changed = base.clone();
        changed.mutations = vec![mutation(PythonMutationOperation::Extend, 2)].into();

        assert_eq!(
            changed.changed_names_from(&base),
            ["VALUE".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn changed_names_treat_rooted_mutation_order_as_semantic() {
        let mut base = state_with_binding();
        base.mutations = vec![
            mutation(PythonMutationOperation::Append, 2),
            mutation(PythonMutationOperation::Extend, 3),
        ]
        .into();
        let mut changed = base.clone();
        let mut reversed = changed.mutations.into_iter().collect::<Vec<_>>();
        reversed.reverse();
        changed.mutations = reversed.into();

        assert_eq!(
            changed.changed_names_from(&base),
            ["VALUE".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn changed_names_ignore_fully_equal_states() {
        let mut base = state_with_binding();
        base.mutations
            .insert(mutation(PythonMutationOperation::Append, 2));

        assert!(base.clone().changed_names_from(&base).is_empty());
    }

    #[test]
    fn changed_names_include_constraint_only_binding_changes() {
        let base = state_with_binding();
        let mut changed = base.clone();
        changed
            .bindings
            .get_mut("VALUE")
            .expect("the fixture binding should exist")
            .select_branch(origin(2), 0);

        assert_eq!(
            changed.changed_names_from(&base),
            ["VALUE".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn changed_names_ignore_namespace_dependency_and_import_only_changes() {
        let base = state_with_binding();
        let mut changed = base.clone();
        changed
            .namespace_causes
            .push(PythonNamespaceCause::unconstrained(PythonUnknown::new(
                PythonUnknownCause::UnsupportedExpression,
                [origin(2)],
            )));
        changed.import_trace.record_component(
            test_file(1),
            PythonImportOutcome::NotFound {
                origin: origin(3),
                module: PythonModuleName::parse("missing")
                    .expect("test Python module name should be valid"),
            },
            None,
        );

        assert!(changed.changed_names_from(&base).is_empty());
    }

    #[test]
    fn loop_effect_degradation_aggregates_effects_and_degrades_changed_names() {
        let base = state_with_binding();
        let loop_origin = origin(6);
        let mut loop_unknown =
            PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, loop_origin);
        loop_unknown.discriminate_predicates("VALUE", loop_origin, None);
        let expected_binding = base
            .binding("VALUE")
            .expect("the fixture binding should exist")
            .clone()
            .join(loop_unknown, loop_origin);
        let mut first_mutation = mutation(PythonMutationOperation::Extend, 2);
        first_mutation.binding = "OTHER".to_string();
        let mut later_mutation = mutation(PythonMutationOperation::Append, 3);
        later_mutation.binding = "OTHER".to_string();
        let first_cause = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedExpression,
            [origin(4)],
        ));
        let second_cause = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedMutation,
            [origin(5)],
        ));

        let mut first_body = base.clone();
        first_body.assign_value(
            "VALUE",
            PythonValue::string("changed".to_string(), origin(2)),
            origin(2),
        );
        first_body.mutations.insert(first_mutation.clone());
        first_body.import_trace.record_file(test_file(1));
        first_body.namespace_causes.push(first_cause.clone());

        let mut second_body = base.clone();
        second_body.assign_value(
            "VALUE",
            PythonValue::string("changed".to_string(), origin(2)),
            origin(2),
        );
        second_body
            .mutations
            .extend([later_mutation.clone(), first_mutation.clone()]);
        second_body.import_trace.record_file(test_file(2));
        second_body.import_trace.record_file(test_file(1));
        second_body.namespace_causes.push(second_cause.clone());

        let degraded = base.degrade_loop_effects(vec![first_body, second_body], loop_origin);

        assert_eq!(degraded.binding("VALUE"), Some(&expected_binding));
        assert_eq!(
            degraded.import_trace.files().collect::<Vec<_>>(),
            [test_file(0), test_file(1), test_file(2)]
        );
        assert_eq!(
            degraded.mutations.as_slice(),
            [first_mutation, later_mutation]
        );
        assert_eq!(degraded.namespace_causes, [first_cause, second_cause]);
    }

    #[test]
    fn branch_join_preserves_first_seen_mutation_order_and_deduplicates() {
        let base = PythonEvaluationState::new(test_file(0));
        let first_seen = mutation(PythonMutationOperation::Extend, 2);
        let later = mutation(PythonMutationOperation::Append, 3);
        let mut first_branch = base.clone();
        first_branch.mutations.insert(first_seen.clone());
        let mut second_branch = base.clone();
        second_branch
            .mutations
            .extend([later.clone(), first_seen.clone()]);

        let joined =
            PythonEvaluationState::join_branches(base, &[first_branch, second_branch], origin(4));

        assert_eq!(joined.mutations.as_slice(), [first_seen, later]);
    }
}
