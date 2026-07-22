pub(super) mod expression;
mod imports;
mod statement;

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;
use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;

use super::BranchConstraints;
use super::BranchJoin;
use super::ChildImportFallback;
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
use super::module_object::PathIntrinsicContamination;
use super::name_analysis::reachable_expr_calls;
use super::truthiness::Truthiness;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonPath;
use crate::python::PythonPathIntrinsic;
use crate::python::PythonSourceModule;
use crate::python::PythonSyntaxError;

pub(super) fn evaluate_body(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonSourceModule,
    body: &[ast::Stmt],
    syntax_errors: Vec<PythonSyntaxError>,
    syntax_impacts: Vec<PythonSyntaxErrorImpact>,
    path_intrinsic_contamination: PathIntrinsicContamination,
) -> (PythonModuleFacts, PythonImportTrace, PythonModuleEffects) {
    let state = PythonEvaluationState::with_path_intrinsic_contamination(
        module.file(),
        path_intrinsic_contamination,
    );
    let mut evaluator = PythonModuleEvaluator {
        db,
        project,
        module,
        state,
    };
    evaluator.evaluate_body(body);
    evaluator.state.finish(syntax_errors, syntax_impacts)
}

/// Context-bearing abstract evaluator that evaluates Python syntax into a forkable state.
pub(super) struct PythonModuleEvaluator<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    module: PythonSourceModule,
    pub(super) state: PythonEvaluationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsupportedCallEffect {
    None,
    Arguments,
    ReceiverAndArguments,
}

impl PythonModuleEvaluator<'_> {
    fn fork(&self) -> Self {
        Self {
            db: self.db,
            project: self.project,
            module: self.module.clone(),
            state: self.state.clone(),
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

    pub(super) fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.module.file(), ranged.span())
    }

    fn origin_at(&self, span: Span) -> Origin {
        Origin::new(self.module.file(), span)
    }

    pub(super) fn record_unsupported_call_effects(&mut self, expression: &ast::Expr) {
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
                .degrade_path_intrinsic_values(&mutation_candidates, self.origin(&call));
        }
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
                    matches!(
                        &bound.value.kind,
                        PythonValueKind::Path(PythonPath::Object(_))
                    )
                }
                PythonBindingState::Bound(bound) => {
                    let PythonValueKind::Path(PythonPath::Intrinsic(intrinsic)) = &bound.value.kind
                    else {
                        has_unsupported = true;
                        continue;
                    };
                    matches!(
                        intrinsic,
                        PythonPathIntrinsic::BuiltinStrType
                            | PythonPathIntrinsic::PathlibPathType
                            | PythonPathIntrinsic::OsPathJoinFunction
                            | PythonPathIntrinsic::OsPathDirnameFunction
                            | PythonPathIntrinsic::OsPathAbspathFunction
                    ) && !self
                        .state
                        .module_effects
                        .path_intrinsic_is_contaminated(*intrinsic)
                }
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
}

impl PythonEvaluationState {
    #[cfg(test)]
    fn new(file: File) -> Self {
        Self::with_path_intrinsic_contamination(file, PathIntrinsicContamination::default())
    }

    fn with_path_intrinsic_contamination(
        file: File,
        path_intrinsic_contamination: PathIntrinsicContamination,
    ) -> Self {
        Self {
            bindings: BTreeMap::new(),
            namespace_causes: Vec::new(),
            mutations: UniqueVec::new(),
            import_trace: PythonImportTrace::rooted(file),
            module_effects: PythonModuleEffects::with_path_intrinsic_contamination(
                path_intrinsic_contamination,
            ),
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
        intrinsic: PythonPathIntrinsic,
        origin: Origin,
    ) -> PythonBinding {
        let value = if self
            .module_effects
            .path_intrinsic_is_contaminated(intrinsic)
        {
            PythonValue::unknown(PythonUnknownCause::UnsupportedMutation, Some(origin))
        } else {
            PythonValue::path_intrinsic(intrinsic, origin)
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

    fn assign_path_intrinsic(
        &mut self,
        name: &str,
        intrinsic: PythonPathIntrinsic,
        origin: Origin,
    ) {
        if self
            .module_effects
            .path_intrinsic_is_contaminated(intrinsic)
        {
            self.bind_unknown(name, &PythonUnknownCause::UnsupportedMutation, origin);
        } else {
            self.assign_value(name, PythonValue::path_intrinsic(intrinsic, origin), origin);
        }
    }

    /// Update a name's single bound value after a successful in-place mutation.
    /// This preserves the binding's assignment origins, branch constraints, and
    /// prior mutation facts; only rebinding operations replace that state.
    ///
    /// If the binding no longer has one bound value, retain that loss of
    /// precision as an unknown instead of treating it as an evaluator invariant.
    fn update_bound_value(&mut self, name: &str, value: PythonValue, origin: Origin) {
        if let Some(bound) = self
            .bindings
            .get_mut(name)
            .and_then(PythonBinding::single_bound_mut)
        {
            bound.value = value;
        } else {
            self.assign_value(
                name,
                PythonValue::unknown(PythonUnknownCause::UnsupportedMutation, Some(origin)),
                origin,
            );
        }
    }

    fn assign_binding(&mut self, name: &str, binding: PythonBinding, origin: Origin) {
        self.mutations.retain(|mutation| mutation.binding != name);
        self.bindings
            .insert(name.to_string(), binding.rebase_binding_origin(origin));
    }

    fn assign_from_name(
        &mut self,
        name: &str,
        source: &str,
        binding: PythonBinding,
        origin: Origin,
    ) {
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

    fn path_intrinsic_write_names(&mut self, name: &str) -> Vec<String> {
        let Some(binding) = self.binding(name) else {
            return vec![name.to_string()];
        };
        let intrinsics = binding
            .alternatives()
            .filter_map(|alternative| {
                let PythonBindingState::Bound(bound) = alternative else {
                    return None;
                };
                let PythonValueKind::Path(PythonPath::Intrinsic(intrinsic)) = bound.value.kind
                else {
                    return None;
                };
                Some(intrinsic)
            })
            .collect::<Vec<_>>();
        if intrinsics.is_empty() {
            return vec![name.to_string()];
        }
        for intrinsic in &intrinsics {
            self.module_effects.contaminate_path_intrinsic(*intrinsic);
        }
        self.bindings
            .iter()
            .filter(|&(_candidate, binding)| {
                binding.alternatives().any(|alternative| {
                    let PythonBindingState::Bound(bound) = alternative else {
                        return false;
                    };
                    let PythonValueKind::Path(PythonPath::Intrinsic(candidate_symbol)) =
                        bound.value.kind
                    else {
                        return false;
                    };
                    intrinsics
                        .iter()
                        .any(|intrinsic| intrinsic.shares_mutable_namespace(candidate_symbol))
                })
            })
            .map(|(candidate, _binding)| candidate.clone())
            .collect()
    }

    fn all_path_intrinsic_write_names(&mut self) -> Vec<String> {
        let names = self
            .bindings
            .iter()
            .filter(|&(_name, binding)| {
                binding.alternatives().any(|alternative| {
                    matches!(
                        alternative,
                        PythonBindingState::Bound(bound)
                            if matches!(bound.value.kind, PythonValueKind::Path(PythonPath::Intrinsic(_)))
                    )
                })
            })
            .map(|(name, _binding)| name.clone())
            .collect::<Vec<_>>();
        for intrinsic in [
            PythonPathIntrinsic::BuiltinsModule,
            PythonPathIntrinsic::PathlibModule,
            PythonPathIntrinsic::OsModule,
        ] {
            self.module_effects.contaminate_path_intrinsic(intrinsic);
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
    ) -> Vec<String> {
        let mut wanted = ReachableAllocationSites::default();
        let Some(binding) = self.binding(name) else {
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
                let occurrences = candidate.allocation_site_occurrences(&wanted);
                occurrences > usize::from(candidate_name.as_str() == name)
            })
            .map(|(name, _binding)| name.clone())
            .collect()
    }

    fn value_for_name(&self, name: &str) -> Option<PythonValue> {
        let binding = self.binding(name)?;
        Some(binding.single_bound()?.value.clone())
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
    pub(super) fn known_truthiness(&self, name: &str) -> Option<bool> {
        if let Some(value) = self.bool_value(name) {
            return Some(value);
        }
        let binding = self.binding(name)?;
        let is_module = |state: &PythonBindingState| {
            matches!(state, PythonBindingState::Bound(bound)
                if matches!(bound.value.kind, PythonValueKind::Module(_)))
        };
        let mut alternatives = binding.alternatives();
        let first = alternatives.next()?;
        (is_module(first) && alternatives.all(is_module)).then_some(true)
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
        for binding in self.bindings.values_mut() {
            *binding = binding.clone().join(unknown.clone(), origin);
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

    fn degrade_path_intrinsic_values(&mut self, values: &[PythonBinding], origin: Origin) {
        let intrinsics = values
            .iter()
            .flat_map(PythonBinding::alternatives)
            .filter_map(|alternative| {
                let PythonBindingState::Bound(bound) = alternative else {
                    return None;
                };
                let PythonValueKind::Path(PythonPath::Intrinsic(intrinsic)) = bound.value.kind
                else {
                    return None;
                };
                Some(intrinsic)
            })
            .collect::<Vec<_>>();
        for intrinsic in &intrinsics {
            self.module_effects.contaminate_path_intrinsic(*intrinsic);
        }
        let aliases = self
            .bindings
            .iter()
            .filter(|&(_candidate, binding)| {
                binding.alternatives().any(|alternative| {
                    let PythonBindingState::Bound(bound) = alternative else {
                        return false;
                    };
                    let PythonValueKind::Path(PythonPath::Intrinsic(candidate_symbol)) =
                        bound.value.kind
                    else {
                        return false;
                    };
                    intrinsics
                        .iter()
                        .any(|intrinsic| intrinsic.shares_mutable_namespace(candidate_symbol))
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
            names.extend(self.path_intrinsic_write_names(&name));
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
            let unknown = PythonBinding::unknown(cause, origin);
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
            let mut joined: Option<PythonBinding> = None;
            for (arm, branch) in branches {
                let mut candidate = branch
                    .binding(&name)
                    .cloned()
                    .unwrap_or_else(PythonBinding::unbound);
                candidate.select_branch(join.to_owned(), *arm);
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
            .map(|(arm, branch)| (*arm, branch.module_effects.clone()))
            .collect::<Vec<_>>();
        base.module_effects = PythonModuleEffects::join_indexed_branches(&branch_effects, join);
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
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::Origin;
    use super::PythonBinding;
    use super::PythonEvaluationState;
    use super::PythonImportOutcome;
    use super::PythonMutation;
    use super::PythonMutationOperation;
    use super::PythonMutationPath;
    use super::PythonNamespaceCause;
    use super::PythonUnknown;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use crate::python::PythonModule;
    use crate::python::PythonModuleName;
    use crate::python::PythonNamespacePackage;

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
            Some(true),
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
        assert_eq!(state.known_truthiness("FLAG"), Some(true));
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
        let expected_binding = base
            .binding("VALUE")
            .expect("the fixture binding should exist")
            .clone()
            .join(
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, loop_origin),
                loop_origin,
            );
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
