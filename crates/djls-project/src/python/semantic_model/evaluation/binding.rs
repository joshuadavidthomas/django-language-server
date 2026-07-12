use std::collections::BTreeMap;

use djls_source::Origin;

use super::BranchConstraints;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;

const MAX_PYTHON_ALTERNATIVES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PythonBindings(pub(crate) BTreeMap<String, PythonBinding>);

impl PythonBindings {
    pub(super) fn get(&self, name: &str) -> Option<&PythonBinding> {
        self.0.get(name)
    }

    pub(super) fn insert(&mut self, name: String, binding: PythonBinding) {
        self.0.insert(name, binding);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonBindingCase {
    alternative: PythonBindingAlternative,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBinding {
    cases: Vec<PythonBindingCase>,
}

impl PythonBinding {
    pub(super) fn new(alternatives: Vec<PythonBindingAlternative>) -> Self {
        assert!(
            !alternatives.is_empty(),
            "a Python binding must have an alternative"
        );
        let mut binding = Self {
            cases: alternatives
                .into_iter()
                .map(|alternative| PythonBindingCase {
                    alternative,
                    constraints: BranchConstraints::unconstrained(),
                })
                .collect(),
        };
        binding.normalize(None);
        binding
    }

    pub(crate) fn alternatives(&self) -> impl ExactSizeIterator<Item = &PythonBindingAlternative> {
        self.cases.iter().map(|case| &case.alternative)
    }

    pub(crate) fn alternatives_with_constraints(
        &self,
    ) -> impl Iterator<Item = (&PythonBindingAlternative, &BranchConstraints)> {
        self.cases
            .iter()
            .map(|case| (&case.alternative, &case.constraints))
    }

    pub(super) fn alternatives_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut PythonBindingAlternative> {
        self.cases.iter_mut().map(|case| &mut case.alternative)
    }

    pub(super) fn correlated(
        alternative: PythonBindingAlternative,
        constraints: BranchConstraints,
    ) -> Self {
        let mut binding = Self {
            cases: vec![PythonBindingCase {
                alternative,
                constraints,
            }],
        };
        binding.normalize(None);
        binding
    }

    pub(super) fn single_bound(&self) -> Option<&PythonBoundValue> {
        let mut alternatives = self.alternatives();
        let PythonBindingAlternative::Bound(bound) = alternatives.next()? else {
            return None;
        };
        alternatives.next().is_none().then_some(bound)
    }

    pub(super) fn single_bound_mut(&mut self) -> Option<&mut PythonBoundValue> {
        if self.cases.len() != 1 {
            return None;
        }
        let PythonBindingAlternative::Bound(bound) = &mut self.cases[0].alternative else {
            return None;
        };
        Some(bound)
    }

    pub(super) fn select_branch(&mut self, join: Origin, arm: usize) {
        for case in &mut self.cases {
            case.constraints.select(join, arm);
        }
    }

    pub(super) fn replace_unbound_with(self, prior: Option<Self>, overflow_origin: Origin) -> Self {
        if !self
            .alternatives()
            .any(|alternative| *alternative == PythonBindingAlternative::Unbound)
        {
            return self;
        }
        let Some(prior) = prior else {
            return self;
        };

        let mut imported = Vec::new();
        let mut unbound_constraints = None;
        for case in self.cases {
            if case.alternative == PythonBindingAlternative::Unbound {
                unbound_constraints = Some(case.constraints);
            } else {
                imported.push(case);
            }
        }
        let fallback = prior.intersect_constraints(
            unbound_constraints
                .as_ref()
                .expect("an unbound alternative has constraints"),
        );
        let imported = (!imported.is_empty()).then_some(Self { cases: imported });
        match (imported, fallback) {
            (Some(imported), Some(fallback)) => imported.join(fallback, overflow_origin),
            (Some(imported), None) => imported,
            (None, Some(fallback)) => fallback,
            (None, None) => unreachable!("an imported fallback must have a feasible branch"),
        }
    }

    pub(super) fn intersect_constraints(mut self, constraints: &BranchConstraints) -> Option<Self> {
        self.cases = self
            .cases
            .into_iter()
            .filter_map(|mut case| {
                let intersection = case.constraints.intersection(constraints);
                if intersection.alternatives.is_empty() {
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

        if joined.exact_alternative_count() > MAX_PYTHON_ALTERNATIVES {
            let mut overflow_origins = vec![overflow_origin];
            let mut retained = Vec::with_capacity(MAX_PYTHON_ALTERNATIVES);
            for case in joined.cases.drain(..) {
                if is_alternative_limit_unknown(&case.alternative)
                    || retained.len() == MAX_PYTHON_ALTERNATIVES
                {
                    collect_alternative_origins(&case.alternative, &mut overflow_origins);
                } else {
                    retained.push(case);
                }
            }
            joined.cases = retained;
            deduplicate_origins(&mut overflow_origins);
            joined.cases.push(PythonBindingCase {
                alternative: PythonBindingAlternative::Bound(PythonBoundValue {
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
            });
            joined.normalize(Some(overflow_origin));
        }
        joined
    }

    fn exact_alternative_count(&self) -> usize {
        self.alternatives()
            .filter(|alternative| !is_alternative_limit_unknown(alternative))
            .count()
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        let mut normalized = Vec::<PythonBindingCase>::new();
        for mut incoming_case in std::mem::take(&mut self.cases) {
            match incoming_case.alternative {
                PythonBindingAlternative::Unbound => {
                    if let Some(existing) = normalized.iter_mut().find(|candidate| {
                        candidate.alternative == PythonBindingAlternative::Unbound
                    }) {
                        existing.constraints.merge(incoming_case.constraints);
                    } else {
                        normalized.push(incoming_case);
                    }
                }
                PythonBindingAlternative::Bound(mut incoming) => {
                    incoming.normalize_origins();
                    incoming
                        .value
                        .constrain_value_evidence(&incoming_case.constraints);
                    if let Some(existing_case) = normalized.iter_mut().find(|candidate| {
                        matches!(&candidate.alternative, PythonBindingAlternative::Bound(bound) if bound.value.same_semantic_value(&incoming.value))
                    }) {
                        let PythonBindingAlternative::Bound(existing) =
                            &mut existing_case.alternative
                        else {
                            unreachable!()
                        };
                        merge_origins(&mut existing.binding_origins, incoming.binding_origins);
                        existing
                            .value
                            .merge_semantically_equal(incoming.value, operation_origin);
                        existing_case.constraints.merge(incoming_case.constraints);
                    } else {
                        incoming_case.alternative = PythonBindingAlternative::Bound(incoming);
                        normalized.push(incoming_case);
                    }
                }
            }
        }
        normalized.sort_by_key(|case| {
            format!(
                "{}:{:?}",
                alternative_sort_key(&case.alternative),
                case.constraints
            )
        });
        self.cases = normalized;
    }
}

fn collect_alternative_origins(alternative: &PythonBindingAlternative, origins: &mut Vec<Origin>) {
    if let PythonBindingAlternative::Bound(bound) = alternative {
        origins.extend(bound.binding_origins.iter().copied());
        origins.extend(bound.value.origins());
    }
}

pub(super) fn is_alternative_limit_unknown(alternative: &PythonBindingAlternative) -> bool {
    matches!(
        alternative,
        PythonBindingAlternative::Bound(PythonBoundValue {
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

fn alternative_sort_key(alternative: &PythonBindingAlternative) -> String {
    match alternative {
        PythonBindingAlternative::Unbound => "0".to_string(),
        PythonBindingAlternative::Bound(bound) => format!(
            "1:{:?}:{:?}:{:?}",
            bound.value.kind,
            bound.value.origins().collect::<Vec<_>>(),
            bound.binding_origins
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonBindingAlternative {
    Bound(PythonBoundValue),
    Unbound,
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
