use std::cmp::Ordering;
use std::mem;

use djls_source::Origin;

use super::BranchConstraints;
use super::MAX_EXACT_PYTHON_ALTERNATIVES;
use super::OriginSet;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ReachableAllocationSites;
use super::StructuralOrd;
use crate::python::PythonModule;

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingCase {
    state: PythonBindingState,
    constraints: BranchConstraints,
}

/// Feasible bound and unbound states for one Python name.
///
/// Each case retains the branch constraints under which it is reachable; normalization merges
/// equivalent values without flattening mutually exclusive worlds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBinding {
    cases: Vec<BindingCase>,
}

impl PythonBinding {
    pub(super) fn bound(value: PythonValue, binding_origin: Origin) -> Self {
        Self::from_case(BindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value,
                binding_origins: [binding_origin].into_iter().collect(),
            }),
            constraints: BranchConstraints::unconstrained(),
        })
    }

    pub(super) fn constrained_bound(
        value: PythonValue,
        binding_origin: Origin,
        constraints: &BranchConstraints,
    ) -> Option<Self> {
        Self::bound(value, binding_origin).intersect_constraints(constraints)
    }

    pub(super) fn unknown(cause: &PythonUnknownCause, origin: Origin) -> Self {
        Self::bound(PythonValue::unknown(cause.clone(), Some(origin)), origin)
    }

    pub(super) fn constrained_unknown(
        cause: &PythonUnknownCause,
        origin: Origin,
        constraints: &BranchConstraints,
    ) -> Option<Self> {
        Self::constrained_bound(
            PythonValue::unknown(cause.clone(), Some(origin)),
            origin,
            constraints,
        )
    }

    pub(super) fn unbound() -> Self {
        Self::from_case(BindingCase {
            state: PythonBindingState::Unbound,
            constraints: BranchConstraints::unconstrained(),
        })
    }

    pub(super) fn originless_cycle_unknown() -> Self {
        Self::from_case(BindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value: PythonValue::unknown(PythonUnknownCause::Cycle, None),
                binding_origins: OriginSet::default(),
            }),
            constraints: BranchConstraints::unconstrained(),
        })
    }

    fn from_case(case: BindingCase) -> Self {
        let mut binding = Self { cases: vec![case] };
        binding.normalize(None);
        binding
    }

    pub(crate) fn alternatives(&self) -> impl ExactSizeIterator<Item = &PythonBindingState> {
        self.cases.iter().map(|case| &case.state)
    }

    pub(crate) fn alternatives_with_constraints(
        &self,
    ) -> impl Iterator<Item = (&PythonBindingState, &BranchConstraints)> {
        self.cases
            .iter()
            .map(|case| (&case.state, &case.constraints))
    }

    fn alternatives_mut(&mut self) -> impl Iterator<Item = &mut PythonBindingState> {
        self.cases.iter_mut().map(|case| &mut case.state)
    }

    pub(super) fn contains_feasible_origin(&self, wanted: Origin) -> bool {
        self.cases.iter().any(|case| match &case.state {
            PythonBindingState::Bound(bound) => bound
                .value
                .has_origin_feasible_under(wanted, &case.constraints),
            PythonBindingState::Unbound => false,
        })
    }

    pub(super) fn reachable_allocation_sites(&self) -> ReachableAllocationSites {
        let mut origins = ReachableAllocationSites::default();
        for state in self.alternatives() {
            if let PythonBindingState::Bound(bound) = state {
                origins.absorb(bound.value.reachable_allocation_sites());
            }
        }
        origins
    }

    pub(super) fn allocation_site_occurrences(&self, wanted: &ReachableAllocationSites) -> usize {
        self.alternatives()
            .filter_map(|state| match state {
                PythonBindingState::Bound(bound) => {
                    Some(bound.value.allocation_site_occurrences(wanted))
                }
                PythonBindingState::Unbound => None,
            })
            .sum()
    }

    pub(super) fn single_bound(&self) -> Option<&PythonBoundValue> {
        let [
            BindingCase {
                state: PythonBindingState::Bound(bound),
                ..
            },
        ] = self.cases.as_slice()
        else {
            return None;
        };
        Some(bound)
    }

    pub(super) fn single_bound_mut(&mut self) -> Option<&mut PythonBoundValue> {
        let [
            BindingCase {
                state: PythonBindingState::Bound(bound),
                ..
            },
        ] = self.cases.as_mut_slice()
        else {
            return None;
        };
        Some(bound)
    }

    pub(super) fn rebase_cycle_unknowns(&mut self, origin: Origin) {
        for state in self.alternatives_mut() {
            let PythonBindingState::Bound(bound) = state else {
                continue;
            };
            let PythonValueKind::Unknown(unknown) = &mut bound.value.kind else {
                continue;
            };
            if unknown.cause == PythonUnknownCause::Cycle {
                bound.rebase_binding_origin(origin);
                bound.value.rebase_origin(origin);
            }
        }
    }

    pub(super) fn rebase_binding_origin(mut self, origin: Origin) -> Self {
        for state in self.alternatives_mut() {
            if let PythonBindingState::Bound(bound) = state {
                bound.binding_origins.replace([origin]);
            }
        }
        self
    }

    pub(super) fn select_branch(&mut self, join: Origin, arm: usize) {
        for case in &mut self.cases {
            case.constraints.select(join, arm);
        }
    }

    pub(super) fn replace_unbound_with(self, prior: Option<Self>, overflow_origin: Origin) -> Self {
        if !self
            .alternatives()
            .any(|state| *state == PythonBindingState::Unbound)
        {
            return self;
        }
        let Some(prior) = prior else {
            return self;
        };

        let mut imported = Vec::new();
        let mut unbound_case = None;
        for case in self.cases {
            if case.state == PythonBindingState::Unbound {
                unbound_case = Some(case);
            } else {
                imported.push(case);
            }
        }
        let Some(unbound_case) = unbound_case else {
            return Self { cases: imported };
        };
        let fallback = prior.intersect_constraints(&unbound_case.constraints);
        let imported = (!imported.is_empty()).then_some(Self { cases: imported });
        let retained_unbound = Self {
            cases: vec![unbound_case],
        };
        match (imported, fallback) {
            (Some(imported), Some(fallback)) => imported.join(fallback, overflow_origin),
            (Some(imported), None) => imported.join(retained_unbound, overflow_origin),
            (None, Some(fallback)) => fallback,
            (None, None) => retained_unbound,
        }
    }

    /// Constraints where an exact child import remains feasible: definite
    /// absence, or a cycle seed whose partially initialized namespace cannot
    /// establish either presence or absence yet.
    pub(super) fn import_fallback_constraints(&self) -> Option<BranchConstraints> {
        self.cases
            .iter()
            .filter(|case| {
                case.state == PythonBindingState::Unbound || case.state.is_cycle_unknown()
            })
            .map(|case| case.constraints.clone())
            .reduce(|mut constraints, incoming| {
                constraints.merge(incoming);
                constraints
            })
    }

    fn cycle_constraints(&self) -> Option<BranchConstraints> {
        self.cases
            .iter()
            .filter(|case| case.state.is_cycle_unknown())
            .map(|case| case.constraints.clone())
            .reduce(|mut constraints, incoming| {
                constraints.merge(incoming);
                constraints
            })
    }

    /// Add a successful/typed fallback result on cycle-seed alternatives without
    /// turning cycle uncertainty into definite missing-member evidence.
    pub(super) fn join_fallback_on_cycle(mut self, fallback: &Self, origin: Origin) -> Self {
        let Some(constraints) = self.cycle_constraints() else {
            return self;
        };
        if let Some(fallback) = fallback.clone().intersect_constraints(&constraints) {
            self = self.join(fallback, origin);
        }
        self
    }

    /// Build a loaded-child coordinate from a complete member projection.
    /// Residual `Unbound` cases become the exact child module. Exact intrinsic
    /// members preserve the prior child coordinate so they still win lookup.
    /// Cycle uncertainty retains both the cycle and loaded-child possibilities.
    pub(super) fn attach_module_for_import_fallback(
        &self,
        prior: &Self,
        child: &PythonModule,
        origin: Origin,
        preserved: Option<&BranchConstraints>,
    ) -> Self {
        let mut contributions = Vec::new();
        if let Some(preserved) = preserved
            && let Some(prior) = prior.clone().intersect_constraints(preserved)
        {
            contributions.push(prior);
        }
        for case in &self.cases {
            let contribution = match &case.state {
                PythonBindingState::Unbound => Self::constrained_bound(
                    PythonValue::module(child.clone(), origin),
                    origin,
                    &case.constraints,
                ),
                state if state.is_cycle_unknown() => Self::constrained_bound(
                    PythonValue::module(child.clone(), origin),
                    origin,
                    &case.constraints,
                )
                .map(|module| {
                    Self {
                        cases: vec![case.clone()],
                    }
                    .join(module, origin)
                }),
                PythonBindingState::Bound(bound)
                    if matches!(bound.value.kind, PythonValueKind::Unknown(_)) =>
                {
                    Some(Self {
                        cases: vec![case.clone()],
                    })
                }
                PythonBindingState::Bound(_) => prior
                    .clone()
                    .intersect_constraints(&case.constraints)
                    .or_else(|| Self::unbound().intersect_constraints(&case.constraints)),
            };
            if let Some(contribution) = contribution {
                contributions.push(contribution);
            }
        }
        contributions
            .into_iter()
            .reduce(|left, right| left.join(right, origin))
            .unwrap_or_else(Self::unbound)
    }

    /// Apply one fallback module's object effect only where member projection
    /// permits the child import. Other member alternatives preserve the prior
    /// coordinate; cycle alternatives retain both possibilities.
    pub(super) fn merge_effect_for_import_fallback(
        &self,
        prior: &Self,
        incoming: &Self,
        origin: Origin,
        preserved: Option<&BranchConstraints>,
    ) -> Self {
        let mut contributions = Vec::new();
        if let Some(preserved) = preserved
            && let Some(prior) = prior.clone().intersect_constraints(preserved)
        {
            contributions.push(prior);
        }
        for case in &self.cases {
            let contribution = match &case.state {
                PythonBindingState::Unbound => {
                    incoming.clone().intersect_constraints(&case.constraints)
                }
                state if state.is_cycle_unknown() => {
                    let incoming = incoming.clone().intersect_constraints(&case.constraints);
                    let prior = prior.clone().intersect_constraints(&case.constraints);
                    match (prior, incoming) {
                        (Some(prior), Some(incoming)) => Some(prior.join(incoming, origin)),
                        (Some(prior), None) => Some(prior),
                        (None, Some(incoming)) => Some(incoming),
                        (None, None) => None,
                    }
                }
                PythonBindingState::Bound(_) => prior
                    .clone()
                    .intersect_constraints(&case.constraints)
                    .or_else(|| Self::unbound().intersect_constraints(&case.constraints)),
            };
            if let Some(contribution) = contribution {
                contributions.push(contribution);
            }
        }
        contributions
            .into_iter()
            .reduce(|left, right| left.join(right, origin))
            .unwrap_or_else(Self::unbound)
    }

    /// Merge an incoming imported coordinate onto `prior`, distributing per
    /// incoming constrained case: an `Unbound` case preserves `prior` on its
    /// constraints, an exact module attachment assigns (dropping `prior` there),
    /// and an unknown case joins `prior` conservatively on its constraints.
    pub(super) fn merge_imported_onto(&self, prior: &Self, origin: Origin) -> Self {
        let mut contributions: Vec<Self> = Vec::new();
        for case in &self.cases {
            let contribution = match &case.state {
                PythonBindingState::Unbound => {
                    prior.clone().intersect_constraints(&case.constraints)
                }
                PythonBindingState::Bound(bound)
                    if matches!(bound.value.kind, PythonValueKind::Module(_)) =>
                {
                    Some(Self {
                        cases: vec![case.clone()],
                    })
                }
                PythonBindingState::Bound(_) => {
                    let incoming = Self {
                        cases: vec![case.clone()],
                    };
                    Some(
                        match prior.clone().intersect_constraints(&case.constraints) {
                            Some(kept) => kept.join(incoming, origin),
                            None => incoming,
                        },
                    )
                }
            };
            if let Some(contribution) = contribution {
                contributions.push(contribution);
            }
        }
        contributions
            .into_iter()
            .reduce(|left, right| left.join(right, origin))
            .unwrap_or_else(Self::unbound)
    }

    pub(super) fn intersect_constraints(mut self, constraints: &BranchConstraints) -> Option<Self> {
        self.cases = self
            .cases
            .into_iter()
            .filter_map(|mut case| {
                let intersection = case.constraints.intersection(constraints);
                if intersection.is_impossible() {
                    None
                } else {
                    case.constraints = intersection;
                    Some(case)
                }
            })
            .collect();
        if self.cases.is_empty() {
            return None;
        }
        self.normalize(None);
        Some(self)
    }

    pub(super) fn join(self, other: Self, overflow_origin: Origin) -> Self {
        let mut cases = self.cases;
        cases.extend(other.cases);
        let mut joined = Self { cases };
        joined.normalize(Some(overflow_origin));

        let exact_alternative_count = joined
            .cases
            .iter()
            .filter(|case| !case.is_limit_remainder())
            .count();
        if exact_alternative_count > MAX_EXACT_PYTHON_ALTERNATIVES {
            let mut overflow_origins: OriginSet = [overflow_origin].into_iter().collect();
            let mut retained = Vec::with_capacity(MAX_EXACT_PYTHON_ALTERNATIVES);
            for case in joined.cases.drain(..) {
                if case.is_limit_remainder() || retained.len() == MAX_EXACT_PYTHON_ALTERNATIVES {
                    if let PythonBindingState::Bound(bound) = &case.state {
                        overflow_origins.extend(bound.binding_origins.iter());
                        overflow_origins.extend(bound.value.origins());
                    }
                } else {
                    retained.push(case);
                }
            }
            retained.push(BindingCase::alternative_limit_remainder(overflow_origins));
            joined.cases = retained;
            joined.normalize(Some(overflow_origin));
        }
        joined
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        let mut normalized = Vec::<BindingCase>::new();
        for mut incoming_case in mem::take(&mut self.cases) {
            match incoming_case.state {
                PythonBindingState::Unbound => {
                    if let Some(existing) = normalized
                        .iter_mut()
                        .find(|candidate| candidate.state == PythonBindingState::Unbound)
                    {
                        existing.constraints.merge(incoming_case.constraints);
                    } else {
                        normalized.push(incoming_case);
                    }
                }
                PythonBindingState::Bound(mut incoming) => {
                    incoming.value.normalize();
                    incoming
                        .value
                        .constrain_value_evidence(&incoming_case.constraints);
                    if let Some(existing_case) = normalized.iter_mut().find(|candidate| {
                        matches!(&candidate.state, PythonBindingState::Bound(bound) if bound.value.same_semantic_value(&incoming.value))
                    }) {
                        match &mut existing_case.state {
                            PythonBindingState::Bound(existing) => {
                                existing.merge_semantically_equal(incoming, operation_origin);
                                existing_case.constraints.merge(incoming_case.constraints);
                            }
                            PythonBindingState::Unbound => {
                                incoming_case.state = PythonBindingState::Bound(incoming);
                                normalized.push(incoming_case);
                            }
                        }
                    } else {
                        incoming_case.state = PythonBindingState::Bound(incoming);
                        normalized.push(incoming_case);
                    }
                }
            }
        }
        normalized.sort_by(BindingCase::structural_cmp);
        self.cases = normalized;
    }
}

impl BindingCase {
    fn alternative_limit_remainder(overflow_origins: OriginSet) -> Self {
        Self {
            state: PythonBindingState::Bound(PythonBoundValue {
                value: PythonValue::unknown(
                    PythonUnknownCause::AlternativeLimitExceeded,
                    overflow_origins.iter(),
                ),
                binding_origins: overflow_origins,
            }),
            // The remainder represents alternatives discarded from potentially different
            // arms, so leaving it unconstrained is conservative and preserves the cap.
            constraints: BranchConstraints::unconstrained(),
        }
    }

    fn is_limit_remainder(&self) -> bool {
        let PythonBindingState::Bound(bound) = &self.state else {
            return false;
        };
        bound
            .value
            .unknown_value()
            .is_some_and(|unknown| unknown.cause == PythonUnknownCause::AlternativeLimitExceeded)
    }
}

impl StructuralOrd for BindingCase {
    /// Unbound precedes Bound so cap retention remains stable. Bound cases then
    /// compare complete value evidence, binding provenance, and constraints.
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.state
            .structural_cmp(&other.state)
            .then_with(|| self.constraints.structural_cmp(&other.constraints))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonBindingState {
    Bound(PythonBoundValue),
    Unbound,
}

impl PythonBindingState {
    fn is_cycle_unknown(&self) -> bool {
        let Self::Bound(bound) = self else {
            return false;
        };
        bound
            .value
            .unknown_value()
            .is_some_and(|unknown| unknown.cause == PythonUnknownCause::Cycle)
    }
}

impl StructuralOrd for PythonBindingState {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Unbound, Self::Unbound) => Ordering::Equal,
            (Self::Unbound, Self::Bound(_)) => Ordering::Less,
            (Self::Bound(_), Self::Unbound) => Ordering::Greater,
            (Self::Bound(left), Self::Bound(right)) => left
                .value
                .structural_cmp(&right.value)
                .then_with(|| left.binding_origins.structural_cmp(&right.binding_origins)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBoundValue {
    pub(crate) value: PythonValue,
    binding_origins: OriginSet,
}

impl PythonBoundValue {
    pub(crate) fn binding_origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.binding_origins.iter()
    }

    pub(crate) fn representative_binding_origin(&self) -> Option<Origin> {
        self.binding_origins.first()
    }

    fn rebase_binding_origin(&mut self, origin: Origin) {
        self.binding_origins.replace([origin]);
    }

    fn merge_semantically_equal(
        &mut self,
        incoming: Self,
        operation_origin: Option<Origin>,
    ) -> bool {
        if !self.value.same_semantic_value(&incoming.value) {
            return false;
        }
        self.binding_origins.extend(incoming.binding_origins.iter());
        self.value
            .merge_semantically_equal(incoming.value, operation_origin)
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use djls_source::File;
    use djls_source::Span;
    use salsa::Id;
    use salsa::plumbing::FromId as _;

    use super::super::BranchConstraints;
    use super::super::OriginSet;
    use super::super::PythonSequenceItem;
    use super::super::PythonUnknown;
    use super::BindingCase;
    use super::MAX_EXACT_PYTHON_ALTERNATIVES;
    use super::Origin;
    use super::PythonBinding;
    use super::PythonBindingState;
    use super::PythonBoundValue;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use super::PythonValueKind;
    use super::StructuralOrd;
    use crate::python::PythonModule;
    use crate::python::PythonNamespacePackage;

    fn origin(file: u32, start: u32) -> Origin {
        // SAFETY: Test indexes are below `salsa::Id::MAX_U32`; these synthetic
        // files are compared only as opaque IDs and are never read.
        let file = File::from_id(unsafe { Id::from_index(file) });
        Origin::new(file, Span::new(start, 1))
    }

    #[derive(Clone)]
    enum BindingValue {
        Exact(String),
        TopLevelUnknown,
        NestedUnknownElement,
        NestedUnknownUnpack,
    }

    fn nested_unknown(origin: Origin) -> PythonUnknown {
        PythonUnknown::new(PythonUnknownCause::UnsupportedExpression, [origin])
    }

    fn binding(value: BindingValue, start: u32) -> PythonBinding {
        let origin = origin(0, start);
        let value = match value {
            BindingValue::Exact(value) => PythonValue::string(value, origin),
            BindingValue::TopLevelUnknown => {
                PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin))
            }
            BindingValue::NestedUnknownElement => PythonValue::list(
                vec![PythonSequenceItem::UnknownElement(nested_unknown(origin))],
                origin,
            ),
            BindingValue::NestedUnknownUnpack => PythonValue::list(
                vec![PythonSequenceItem::UnknownUnpack(nested_unknown(origin))],
                origin,
            ),
        };
        PythonBinding::bound(value, origin)
    }

    fn joined(bindings: Vec<PythonBinding>, right_grouped: bool) -> PythonBinding {
        let overflow_origin = origin(0, 1_000);
        if right_grouped {
            let mut bindings = bindings;
            let mut result = bindings.pop();
            while let Some(left) = bindings.pop() {
                result = Some(left.join(
                    result.expect("a right-grouped join has a right operand"),
                    overflow_origin,
                ));
            }
            result.expect("a binding join needs at least one alternative")
        } else {
            bindings
                .into_iter()
                .reduce(|left, right| left.join(right, overflow_origin))
                .expect("a binding join needs at least one alternative")
        }
    }

    fn assert_join_laws(bindings: Vec<PythonBinding>) -> PythonBinding {
        let left = joined(bindings.clone(), false);
        assert_eq!(
            left,
            joined(bindings.clone(), true),
            "join must be associative"
        );

        let mut reversed = bindings.clone();
        reversed.reverse();
        assert_eq!(left, joined(reversed, false), "join must be commutative");

        let mut duplicated = bindings.clone();
        duplicated.extend(bindings);
        assert_eq!(left, joined(duplicated, false), "join must be idempotent");
        left
    }

    fn namespace_module(name: &str) -> PythonModule {
        PythonModule::Namespace(PythonNamespacePackage::new(
            crate::python::PythonModuleName::parse(name)
                .expect("test Python module name should be valid"),
            Vec::new(),
        ))
    }

    fn module_binding(name: &str, start: u32) -> PythonBinding {
        let origin = origin(0, start);
        PythonBinding::bound(PythonValue::module(namespace_module(name), origin), origin)
    }

    fn contains_str(binding: &PythonBinding, wanted: &str) -> bool {
        binding.alternatives().any(|state| {
            let PythonBindingState::Bound(bound) = state else {
                return false;
            };
            matches!(&bound.value.kind, PythonValueKind::Str(text) if text.as_str() == wanted)
        })
    }

    #[test]
    fn infeasible_unbound_replacement_stays_conservatively_unbound() {
        let join = origin(0, 100);
        let mut imported_constraints = BranchConstraints::unconstrained();
        imported_constraints.select(join, 0);
        let imported = PythonBinding {
            cases: vec![BindingCase {
                state: PythonBindingState::Unbound,
                constraints: imported_constraints.clone(),
            }],
        };
        let mut prior_constraints = BranchConstraints::unconstrained();
        prior_constraints.select(join, 1);
        let prior = PythonBinding::constrained_bound(
            PythonValue::string("prior".to_string(), origin(0, 1)),
            origin(0, 1),
            &prior_constraints,
        )
        .expect("the prior branch constraints should be feasible");

        let replaced = imported.replace_unbound_with(Some(prior), origin(0, 2));

        assert_eq!(
            replaced.alternatives_with_constraints().collect::<Vec<_>>(),
            vec![(&PythonBindingState::Unbound, &imported_constraints)],
        );
    }

    #[test]
    fn infeasible_unbound_replacement_preserves_mixed_bound_and_unbound_cases() {
        let join = origin(0, 100);
        let mut imported_bound = binding(BindingValue::Exact("imported".to_string()), 1);
        imported_bound.select_branch(join, 0);
        let mut imported_unbound = PythonBinding::unbound();
        imported_unbound.select_branch(join, 1);
        let imported = imported_bound.join(imported_unbound, origin(0, 2));

        let mut prior_constraints = BranchConstraints::unconstrained();
        prior_constraints.select(join, 0);
        let prior = PythonBinding::constrained_bound(
            PythonValue::string("prior".to_string(), origin(0, 3)),
            origin(0, 3),
            &prior_constraints,
        )
        .expect("the prior branch constraints should be feasible");
        let replaced = imported
            .clone()
            .replace_unbound_with(Some(prior), origin(0, 4));

        assert_eq!(replaced, imported);
        let mut expected_unbound_constraints = BranchConstraints::unconstrained();
        expected_unbound_constraints.select(join, 1);
        assert!(contains_str(&replaced, "imported"));
        assert!(!contains_str(&replaced, "prior"));
        assert!(
            replaced
                .alternatives_with_constraints()
                .any(|(state, constraints)| {
                    state == &PythonBindingState::Unbound
                        && constraints == &expected_unbound_constraints
                })
        );
    }

    #[test]
    fn replace_unbound_with_composes_conditional_child_and_source_fallback() {
        let join = origin(0, 100);
        let mut child = module_binding("pkg", 1);
        child.select_branch(join, 0);
        let mut missing = PythonBinding::unbound();
        missing.select_branch(join, 1);
        let base = child.join(missing, origin(0, 3));
        assert!(
            base.alternatives()
                .any(|state| *state == PythonBindingState::Unbound),
            "the branch without a child contributes residual Unbound",
        );

        let fallback = PythonBinding::bound(
            PythonValue::string("src".to_string(), origin(0, 4)),
            origin(0, 4),
        );
        let composed = base.replace_unbound_with(Some(fallback), origin(0, 5));

        assert!(
            !composed
                .alternatives()
                .any(|state| *state == PythonBindingState::Unbound),
            "the source fallback covers the residual Unbound",
        );
        assert!(
            contains_str(&composed, "src"),
            "intrinsic source fallback applies"
        );
        assert_eq!(
            composed
                .alternatives()
                .filter(|state| matches!(
                    state,
                    PythonBindingState::Bound(bound)
                        if matches!(bound.value.kind, PythonValueKind::Module(_))
                ))
                .count(),
            1,
            "the loaded child survives on its own branch",
        );
    }

    #[test]
    fn merge_imported_onto_distributes_per_incoming_case() {
        let prior = module_binding("pkg.a", 1);
        let incoming = module_binding("pkg.b", 2).join(
            PythonBinding::unknown(&PythonUnknownCause::Cycle, origin(0, 3)),
            origin(0, 3),
        );

        let merged = incoming.merge_imported_onto(&prior, origin(0, 10));

        let module_count = merged
            .alternatives()
            .filter(|state| {
                matches!(
                    state,
                    PythonBindingState::Bound(bound)
                        if matches!(bound.value.kind, PythonValueKind::Module(_))
                )
            })
            .count();
        assert_eq!(
            module_count, 2,
            "exact attachment assigns pkg.b while the unknown case keeps prior pkg.a",
        );
        assert!(
            merged.alternatives().any(|state| matches!(
                state,
                PythonBindingState::Bound(bound)
                    if bound.value.unknown_value().is_some_and(|u| u.cause == PythonUnknownCause::Cycle)
            )),
            "the unknown case is retained",
        );
    }

    #[test]
    fn typed_value_order_binding_cases_is_total_before_semantic_merge() {
        let join = origin(0, 100);
        let mut first_constraints = BranchConstraints::unconstrained();
        first_constraints.select(join, 0);
        let mut second_constraints = BranchConstraints::unconstrained();
        second_constraints.select(join, 1);

        let first = BindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value: PythonValue::string("same".to_string(), origin(0, 10)),
                binding_origins: [origin(0, 10)].into_iter().collect(),
            }),
            constraints: first_constraints,
        };
        let second = BindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value: PythonValue::string("same".to_string(), origin(0, 20)),
                binding_origins: [origin(0, 20)].into_iter().collect(),
            }),
            constraints: second_constraints,
        };
        let unbound = BindingCase {
            state: PythonBindingState::Unbound,
            constraints: BranchConstraints::unconstrained(),
        };
        assert_eq!(unbound.structural_cmp(&first), Ordering::Less);
        assert_ne!(first, second);
        assert_ne!(first.structural_cmp(&second), Ordering::Equal);
        assert_eq!(
            first.structural_cmp(&second),
            second.structural_cmp(&first).reverse()
        );

        let merged = PythonBinding { cases: vec![first] }.join(
            PythonBinding {
                cases: vec![second],
            },
            origin(0, 1_000),
        );
        let bound = merged
            .single_bound()
            .expect("semantically equal values should merge to one bound case");
        assert_eq!(
            bound.binding_origins().collect::<Vec<_>>(),
            [origin(0, 10), origin(0, 20)]
        );
    }

    #[test]
    fn binding_join_obeys_laws_and_orders_origins_for_all_value_shapes() {
        let cases = [
            BindingValue::Exact("same".to_string()),
            BindingValue::TopLevelUnknown,
            BindingValue::NestedUnknownElement,
            BindingValue::NestedUnknownUnpack,
        ];
        for value in cases {
            let joined = assert_join_laws(vec![
                binding(value.clone(), 30),
                binding(value.clone(), 10),
                binding(value, 20),
            ]);
            let bound = joined
                .single_bound()
                .expect("equal values should normalize to one bound alternative");
            assert_eq!(
                bound
                    .binding_origins()
                    .map(|origin| origin.span.start())
                    .collect::<Vec<_>>(),
                [10, 20, 30],
                "binding origins must be retained in order",
            );
            assert_eq!(
                bound
                    .value
                    .origins()
                    .map(|origin| origin.span.start())
                    .collect::<Vec<_>>(),
                [10, 20, 30],
                "value origins must be retained in order",
            );
        }

        assert_join_laws(vec![
            binding(BindingValue::Exact("c".to_string()), 30),
            binding(BindingValue::Exact("a".to_string()), 10),
            binding(BindingValue::Exact("b".to_string()), 20),
        ]);
    }

    #[test]
    fn cross_file_origin_sets_do_not_change_semantic_equality() {
        let from_a = origin(0, 10);
        let from_b = origin(1, 20);
        let exact = |origins: Vec<Origin>| {
            origins
                .into_iter()
                .map(|origin| {
                    PythonBinding::bound(PythonValue::string("same".to_string(), origin), origin)
                })
                .reduce(|binding, incoming| binding.join(incoming, from_a))
                .expect("test bindings have at least one origin")
        };
        let a = exact(vec![from_a]);
        let ab = exact(vec![from_a, from_b]);
        let b = exact(vec![from_b]);

        assert_eq!(
            joined(vec![a.clone(), ab.clone(), b.clone()], false),
            joined(vec![a, ab, b], true),
        );
    }

    #[test]
    fn typed_value_order_binding_cap_retains_the_same_subset_for_reversed_input() {
        let alternatives = |count: u32| {
            (0..count)
                .map(|index| binding(BindingValue::Exact(format!("{index:03}")), index))
                .collect::<Vec<_>>()
        };

        let at_limit = assert_join_laws(alternatives(64));
        assert_eq!(at_limit.alternatives().len(), 64);
        assert!(!at_limit.cases.iter().any(BindingCase::is_limit_remainder));
        assert_eq!(MAX_EXACT_PYTHON_ALTERNATIVES, 64);

        let overflowed = assert_join_laws(alternatives(65));
        assert_eq!(overflowed.alternatives().len(), 65);
        assert_eq!(
            overflowed
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
                .collect::<Vec<_>>(),
            (0..64)
                .map(|index| format!("{index:03}"))
                .collect::<Vec<_>>(),
            "the typed order retains the same exact 64-value subset"
        );
        let overflow = overflowed
            .cases
            .iter()
            .find(|case| case.is_limit_remainder())
            .and_then(|case| match &case.state {
                PythonBindingState::Bound(bound) => Some(bound),
                PythonBindingState::Unbound => None,
            })
            .expect("overflow should add one bound typed unknown remainder");
        let unknown = overflow
            .value
            .unknown_value()
            .expect("overflow remainder should remain an unknown value");
        assert!(
            unknown
                .origins()
                .any(|origin| origin.span == Span::new(1_000, 1)),
            "join origin should be retained",
        );
        assert_eq!(
            overflow
                .value
                .origins()
                .map(|origin| origin.span.start())
                .collect::<Vec<_>>(),
            [64, 1_000],
            "overflow evidence should include the dropped alternative and primary join",
        );
    }

    #[test]
    fn origin_set_is_independent_of_insertion_order() {
        let first = origin(0, 20);
        let second = origin(0, 10);
        let third = origin(1, 5);

        let forward = [first, second, third, first]
            .into_iter()
            .collect::<OriginSet>();
        let reverse = [third, second, first].into_iter().collect::<OriginSet>();

        assert_eq!(forward, reverse);
        assert_eq!(forward.iter().collect::<Vec<_>>(), [second, first, third]);
    }
}
