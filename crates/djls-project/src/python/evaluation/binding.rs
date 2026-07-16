use djls_source::Origin;

use super::BranchConstraints;
use super::MAX_EXACT_PYTHON_ALTERNATIVES;
use super::MutableOrigins;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::origin_sort_key;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CanonicalOrigins(Vec<Origin>);

impl CanonicalOrigins {
    fn one(origin: Origin) -> Self {
        Self(vec![origin])
    }

    fn insert(&mut self, origin: Origin) {
        if self.0.contains(&origin) {
            return;
        }
        self.0.push(origin);
        self.0.sort_by_key(origin_sort_key);
    }

    fn extend(&mut self, origins: impl IntoIterator<Item = Origin>) {
        for origin in origins {
            self.insert(origin);
        }
    }

    fn rebase(&mut self, origin: Origin) {
        self.0.clear();
        self.0.push(origin);
    }

    fn first(&self) -> Option<Origin> {
        self.0.first().copied()
    }

    fn iter(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.0.iter().copied()
    }
}

impl FromIterator<Origin> for CanonicalOrigins {
    fn from_iter<T: IntoIterator<Item = Origin>>(iter: T) -> Self {
        let mut origins = Self::default();
        origins.extend(iter);
        origins
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonBindingCase {
    state: PythonBindingState,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBinding {
    cases: Vec<PythonBindingCase>,
}

impl PythonBinding {
    pub(super) fn bound(value: PythonValue, binding_origin: Origin) -> Self {
        Self::from_case(PythonBindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value,
                binding_origins: CanonicalOrigins::one(binding_origin),
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
        Self::from_case(PythonBindingCase {
            state: PythonBindingState::Unbound,
            constraints: BranchConstraints::unconstrained(),
        })
    }

    pub(super) fn originless_cycle_unknown() -> Self {
        Self::from_case(PythonBindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value: PythonValue::unknown(PythonUnknownCause::Cycle, None),
                binding_origins: CanonicalOrigins::default(),
            }),
            constraints: BranchConstraints::unconstrained(),
        })
    }

    fn from_case(case: PythonBindingCase) -> Self {
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

    pub(super) fn contains_mutable_value(&self) -> bool {
        !self.reachable_mutable_origins().is_empty()
    }

    pub(super) fn reachable_mutable_origins(&self) -> MutableOrigins {
        let mut origins = MutableOrigins::default();
        for state in self.alternatives() {
            if let PythonBindingState::Bound(bound) = state {
                origins.extend(bound.value.reachable_mutable_origins().iter());
            }
        }
        origins
    }

    pub(super) fn mutable_origin_occurrences(&self, wanted: &MutableOrigins) -> usize {
        self.alternatives()
            .filter_map(|state| match state {
                PythonBindingState::Bound(bound) => {
                    Some(bound.value.mutable_origin_occurrences(wanted))
                }
                PythonBindingState::Unbound => None,
            })
            .sum()
    }

    pub(super) fn single_bound(&self) -> Option<&PythonBoundValue> {
        let mut states = self.alternatives();
        let PythonBindingState::Bound(bound) = states.next()? else {
            return None;
        };
        states.next().is_none().then_some(bound)
    }

    pub(super) fn single_bound_mut(&mut self) -> Option<&mut PythonBoundValue> {
        if self.cases.len() != 1 {
            return None;
        }
        let PythonBindingState::Bound(bound) = &mut self.cases[0].state else {
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
                unknown.origin = Some(origin);
                bound.rebase_binding_origin(origin);
                bound.value.rebase_origin(origin);
            }
        }
    }

    pub(super) fn rebase_binding_origin(mut self, origin: Origin) -> Self {
        for state in self.alternatives_mut() {
            if let PythonBindingState::Bound(bound) = state {
                bound.binding_origins.rebase(origin);
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
        let mut unbound_constraints = None;
        for case in self.cases {
            if case.state == PythonBindingState::Unbound {
                unbound_constraints = Some(case.constraints);
            } else {
                imported.push(case);
            }
        }
        let fallback = prior.intersect_constraints(
            unbound_constraints
                .as_ref()
                .expect("an unbound state has constraints"),
        );
        let imported = (!imported.is_empty()).then_some(Self { cases: imported });
        match (imported, fallback) {
            (Some(imported), Some(fallback)) => imported.join(fallback, overflow_origin),
            (Some(imported), None) => imported,
            (None, Some(fallback)) => fallback,
            (None, None) => unreachable!("an imported fallback must have a feasible branch"),
        }
    }

    fn intersect_constraints(mut self, constraints: &BranchConstraints) -> Option<Self> {
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

        if joined.exact_alternative_count() > MAX_EXACT_PYTHON_ALTERNATIVES {
            let mut overflow_origins = CanonicalOrigins::one(overflow_origin);
            let mut retained = Vec::with_capacity(MAX_EXACT_PYTHON_ALTERNATIVES);
            for case in joined.cases.drain(..) {
                if case.state.is_limit_remainder()
                    || retained.len() == MAX_EXACT_PYTHON_ALTERNATIVES
                {
                    if let PythonBindingState::Bound(bound) = &case.state {
                        overflow_origins.extend(bound.binding_origins.iter());
                        overflow_origins.extend(bound.value.origins());
                    }
                } else {
                    retained.push(case);
                }
            }
            retained.push(Self::alternative_limit_case(
                overflow_origin,
                overflow_origins,
            ));
            joined.cases = retained;
            joined.normalize(Some(overflow_origin));
        }
        joined
    }

    fn alternative_limit_case(
        overflow_origin: Origin,
        overflow_origins: CanonicalOrigins,
    ) -> PythonBindingCase {
        PythonBindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value: PythonValue::with_evidence(
                    PythonValueKind::Unknown(PythonUnknown {
                        cause: PythonUnknownCause::AlternativeLimitExceeded,
                        origin: Some(overflow_origin),
                    }),
                    overflow_origins.iter(),
                ),
                binding_origins: overflow_origins,
            }),
            // The remainder represents alternatives discarded from potentially different
            // arms, so leaving it unconstrained is conservative and preserves the cap.
            constraints: BranchConstraints::unconstrained(),
        }
    }

    fn exact_alternative_count(&self) -> usize {
        self.alternatives()
            .filter(|state| !state.is_limit_remainder())
            .count()
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        let mut normalized = Vec::<PythonBindingCase>::new();
        for mut incoming_case in std::mem::take(&mut self.cases) {
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
                    incoming.normalize_origins();
                    incoming
                        .value
                        .constrain_value_evidence(&incoming_case.constraints);
                    if let Some(existing_case) = normalized.iter_mut().find(|candidate| {
                        matches!(&candidate.state, PythonBindingState::Bound(bound) if bound.value.same_semantic_value(&incoming.value))
                    }) {
                        let PythonBindingState::Bound(existing) = &mut existing_case.state else {
                            unreachable!()
                        };
                        existing
                            .binding_origins
                            .extend(incoming.binding_origins.iter());
                        existing
                            .value
                            .merge_semantically_equal(incoming.value, operation_origin);
                        existing_case.constraints.merge(incoming_case.constraints);
                    } else {
                        incoming_case.state = PythonBindingState::Bound(incoming);
                        normalized.push(incoming_case);
                    }
                }
            }
        }
        normalized.sort_by_key(PythonBindingCase::canonical_sort_key);
        self.cases = normalized;
    }
}

impl PythonBindingCase {
    fn canonical_sort_key(&self) -> String {
        match &self.state {
            PythonBindingState::Unbound => format!("0:{:?}", self.constraints),
            PythonBindingState::Bound(bound) => format!(
                "1:{:?}:{:?}:{:?}:{:?}",
                bound.value.kind,
                bound.value.origins().collect::<Vec<_>>(),
                bound.binding_origins.iter().collect::<Vec<_>>(),
                self.constraints,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonBindingState {
    Bound(PythonBoundValue),
    Unbound,
}

impl PythonBindingState {
    pub(super) fn is_limit_remainder(&self) -> bool {
        matches!(
            self,
            Self::Bound(PythonBoundValue {
                value: PythonValue {
                    kind: PythonValueKind::Unknown(PythonUnknown {
                        cause: PythonUnknownCause::AlternativeLimitExceeded,
                        ..
                    }),
                    ..
                },
                ..
            })
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBoundValue {
    pub(crate) value: PythonValue,
    binding_origins: CanonicalOrigins,
}

impl PythonBoundValue {
    pub(crate) fn binding_origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.binding_origins.iter()
    }

    pub(crate) fn first_binding_origin(&self) -> Option<Origin> {
        self.binding_origins.first()
    }

    fn rebase_binding_origin(&mut self, origin: Origin) {
        self.binding_origins.rebase(origin);
    }

    fn normalize_origins(&mut self) {
        self.value.normalize();
    }
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::CanonicalOrigins;
    use super::Origin;

    fn origin(file: u32, start: u32) -> Origin {
        // SAFETY: Test indexes are below `salsa::Id::MAX_U32`; these synthetic
        // files are compared only as opaque IDs and are never read.
        let file = File::from_id(unsafe { salsa::Id::from_index(file) });
        Origin::new(file, Span::new(start, 1))
    }

    #[test]
    fn canonical_origins_are_independent_of_insertion_order() {
        let first = origin(0, 20);
        let second = origin(0, 10);
        let third = origin(1, 5);

        let forward = [first, second, third, first]
            .into_iter()
            .collect::<CanonicalOrigins>();
        let reverse = [third, second, first]
            .into_iter()
            .collect::<CanonicalOrigins>();

        assert_eq!(forward, reverse);
        assert_eq!(forward.iter().collect::<Vec<_>>(), [second, first, third]);
    }
}
