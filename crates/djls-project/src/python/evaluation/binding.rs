use djls_source::Origin;

use super::BranchConstraints;
use super::MAX_EXACT_PYTHON_ALTERNATIVES;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;

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
                binding_origins: vec![binding_origin],
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
                binding_origins: Vec::new(),
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

    pub(super) fn alternatives_mut(&mut self) -> impl Iterator<Item = &mut PythonBindingState> {
        self.cases.iter_mut().map(|case| &mut case.state)
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

    pub(super) fn rebase_binding_origin(mut self, origin: Origin) -> Self {
        for state in self.alternatives_mut() {
            if let PythonBindingState::Bound(bound) = state {
                bound.binding_origins = vec![origin];
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
            let mut overflow_origins = vec![overflow_origin];
            let mut retained = Vec::with_capacity(MAX_EXACT_PYTHON_ALTERNATIVES);
            for case in joined.cases.drain(..) {
                if case.state.is_limit_remainder()
                    || retained.len() == MAX_EXACT_PYTHON_ALTERNATIVES
                {
                    if let PythonBindingState::Bound(bound) = &case.state {
                        overflow_origins.extend(bound.binding_origins.iter().copied());
                        overflow_origins.extend(bound.value.origins());
                    }
                } else {
                    retained.push(case);
                }
            }
            deduplicate_origins(&mut overflow_origins);
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
        overflow_origins: Vec<Origin>,
    ) -> PythonBindingCase {
        PythonBindingCase {
            state: PythonBindingState::Bound(PythonBoundValue {
                value: PythonValue::with_evidence(
                    PythonValueKind::Unknown(PythonUnknown {
                        cause: PythonUnknownCause::AlternativeLimitExceeded,
                        origin: Some(overflow_origin),
                    }),
                    overflow_origins.clone(),
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
                        merge_origins(&mut existing.binding_origins, incoming.binding_origins);
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
        normalized.sort_by_key(|case| {
            format!(
                "{}:{:?}",
                binding_state_sort_key(&case.state),
                case.constraints
            )
        });
        self.cases = normalized;
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

fn binding_state_sort_key(state: &PythonBindingState) -> String {
    match state {
        PythonBindingState::Unbound => "0".to_string(),
        PythonBindingState::Bound(bound) => format!(
            "1:{:?}:{:?}:{:?}",
            bound.value.kind,
            bound.value.origins().collect::<Vec<_>>(),
            bound.binding_origins
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBoundValue {
    pub(crate) value: PythonValue,
    pub(crate) binding_origins: Vec<Origin>,
}

impl PythonBoundValue {
    fn normalize_origins(&mut self) {
        deduplicate_origins(&mut self.binding_origins);
        self.value.normalize();
    }
}

fn deduplicate_origins(origins: &mut Vec<Origin>) {
    origins.sort_by_key(|origin| {
        (
            format!("{:?}", origin.file),
            origin.span.start(),
            origin.span.length(),
        )
    });
    origins.dedup();
}

fn merge_origins(target: &mut Vec<Origin>, incoming: Vec<Origin>) {
    target.extend(incoming);
    deduplicate_origins(target);
}
