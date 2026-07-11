use std::collections::BTreeMap;
use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileReadError;
use djls_source::Origin;

use super::mutations::PythonMutationAccess;
use crate::python::PythonModuleName;
use crate::python::PythonSyntaxError;
use crate::python::module::PythonImportError;

const MAX_PYTHON_ALTERNATIVES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonModuleValuesOutcome {
    Readable(PythonModuleValues),
    Unreadable(FileReadError),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PythonModuleValues {
    pub(crate) bindings: PythonBindings,
    pub(crate) namespace_remainder: Option<PythonNamespaceRemainder>,
    pub(crate) syntax_errors: Vec<PythonSyntaxError>,
    pub(crate) syntax_impacts: Vec<PythonSyntaxImpact>,
    pub(crate) mutations: Vec<PythonMutation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonSyntaxImpact {
    pub(crate) error: PythonSyntaxError,
    pub(crate) names: BTreeSet<String>,
    pub(crate) namespace_open: bool,
    pub(crate) excluded_names: BTreeSet<String>,
}

impl PythonSyntaxImpact {
    pub(crate) fn affects(&self, name: &str) -> bool {
        self.names.contains(name) || (self.namespace_open && !self.excluded_names.contains(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonMutation {
    pub(crate) root: String,
    pub(crate) access: Vec<PythonMutationAccess>,
    pub(crate) method: String,
    pub(crate) origin: Origin,
}

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
pub(crate) struct BranchConstraints {
    // Disjunction of conjunctions. Each conjunction records one arm selected at a control-flow
    // join. Origins identify joins without retaining AST nodes across query boundaries.
    alternatives: Vec<Vec<(Origin, usize)>>,
}

impl BranchConstraints {
    pub(crate) fn unconstrained() -> Self {
        Self {
            alternatives: vec![Vec::new()],
        }
    }

    fn select(&mut self, join: Origin, arm: usize) {
        for alternative in &mut self.alternatives {
            alternative.retain(|(existing, _)| *existing != join);
            alternative.push((join, arm));
            alternative.sort_by_key(|choice| format!("{choice:?}"));
        }
        self.normalize();
    }

    fn merge(&mut self, other: Self) {
        self.alternatives.extend(other.alternatives);
        self.normalize();
    }

    fn normalize(&mut self) {
        self.alternatives
            .sort_by_key(|choices| format!("{choices:?}"));
        self.alternatives.dedup();
    }

    pub(crate) fn normalized_alternatives(&self) -> &[Vec<(Origin, usize)>] {
        &self.alternatives
    }

    pub(crate) fn intersection(&self, other: &Self) -> Self {
        let mut alternatives = Vec::new();
        for left in &self.alternatives {
            for right in &other.alternatives {
                let mut choices = left.clone();
                let mut compatible = true;
                for &(join, arm) in right {
                    if let Some((_, existing_arm)) = choices
                        .iter()
                        .find(|(existing_join, _)| *existing_join == join)
                    {
                        if *existing_arm != arm {
                            compatible = false;
                            break;
                        }
                    } else {
                        choices.push((join, arm));
                    }
                }
                if compatible {
                    choices.sort_by_key(|choice| format!("{choice:?}"));
                    alternatives.push(choices);
                }
            }
        }
        let mut constraints = Self { alternatives };
        constraints.normalize();
        constraints
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

fn is_alternative_limit_unknown(alternative: &PythonBindingAlternative) -> bool {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonValueEvidence {
    origin: Origin,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonValue {
    pub(crate) kind: PythonValueKind,
    evidence: Vec<PythonValueEvidence>,
}

impl PythonValue {
    pub(super) fn unknown(cause: PythonUnknownCause, origin: Option<Origin>) -> Self {
        Self {
            kind: PythonValueKind::Unknown(PythonUnknown { cause, origin }),
            evidence: origin
                .into_iter()
                .map(|origin| PythonValueEvidence {
                    origin,
                    constraints: BranchConstraints::unconstrained(),
                })
                .collect(),
        }
    }

    pub(super) fn known(kind: PythonValueKind, origin: Origin) -> Self {
        Self {
            kind,
            evidence: vec![PythonValueEvidence {
                origin,
                constraints: BranchConstraints::unconstrained(),
            }],
        }
    }

    fn with_evidence(kind: PythonValueKind, origins: Vec<Origin>) -> Self {
        Self {
            kind,
            evidence: origins
                .into_iter()
                .map(|origin| PythonValueEvidence {
                    origin,
                    constraints: BranchConstraints::unconstrained(),
                })
                .collect(),
        }
    }

    pub(crate) fn origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.evidence.iter().map(|evidence| evidence.origin)
    }

    pub(crate) fn origins_with_constraints(
        &self,
    ) -> impl Iterator<Item = (Origin, &BranchConstraints)> {
        self.evidence
            .iter()
            .map(|evidence| (evidence.origin, &evidence.constraints))
    }

    pub(super) fn rebase_origin(&mut self, origin: Origin) {
        self.evidence = vec![PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
        }];
    }

    pub(super) fn record_origin(&mut self, origin: Origin) {
        self.evidence.push(PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
        });
    }

    pub(super) fn normalize(&mut self) {
        normalize_value_evidence(&mut self.evidence);
        self.kind.normalize();
    }

    fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        for evidence in &mut self.evidence {
            evidence.constraints = evidence.constraints.intersection(constraints);
        }
        match &mut self.kind {
            PythonValueKind::List(list) => {
                for variant in &mut list.variants {
                    variant.constraints = variant.constraints.intersection(constraints);
                    for item in &mut variant.items {
                        if let PythonListItem::Value(value) = item {
                            value.constrain_value_evidence(constraints);
                        }
                    }
                }
            }
            PythonValueKind::Dict(dict) => {
                for item in &mut dict.items {
                    if let PythonDictItem::Entry { key, value } = item {
                        key.constrain_value_evidence(constraints);
                        value.constrain_value_evidence(constraints);
                    }
                }
            }
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Unknown(_) => {}
        }
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        self.kind.same_semantic_value(&other.kind)
    }

    fn merge_semantically_equal(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        debug_assert!(self.same_semantic_value(&incoming));
        self.evidence.extend(incoming.evidence);
        self.kind
            .merge_semantically_equal(incoming.kind, operation_origin);
        self.normalize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonValueKind {
    Str(String),
    Bool(bool),
    Path(Utf8PathBuf),
    List(PythonList),
    Dict(PythonDict),
    Unknown(PythonUnknown),
}

impl PythonValueKind {
    fn normalize(&mut self) {
        match self {
            Self::List(list) => list.normalize(None),
            Self::Dict(dict) => dict.normalize(),
            Self::Str(_) | Self::Bool(_) | Self::Path(_) | Self::Unknown(_) => {}
        }
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Path(left), Self::Path(right)) => left == right,
            (Self::List(left), Self::List(right)) => left.same_semantic_value(right),
            (Self::Dict(left), Self::Dict(right)) => left.same_semantic_value(right),
            (Self::Unknown(left), Self::Unknown(right)) => left.cause == right.cause,
            (
                Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::List(_)
                | Self::Dict(_)
                | Self::Unknown(_),
                _,
            ) => false,
        }
    }

    fn merge_semantically_equal(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        debug_assert!(self.same_semantic_value(&incoming));
        match (self, incoming) {
            (Self::List(existing), Self::List(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin);
            }
            (Self::Dict(existing), Self::Dict(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin);
            }
            (Self::Unknown(existing), Self::Unknown(incoming)) => {
                existing.origin = earliest_origin(existing.origin, incoming.origin);
            }
            (Self::Str(_), Self::Str(_))
            | (Self::Bool(_), Self::Bool(_))
            | (Self::Path(_), Self::Path(_)) => {}
            (
                Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::List(_)
                | Self::Dict(_)
                | Self::Unknown(_),
                _,
            ) => unreachable!("semantic equality requires matching value variants"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonListVariant {
    pub(crate) items: Vec<PythonListItem>,
    pub(crate) constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonList {
    /// Canonical items used for semantic equality and ordinary value consumers.
    pub(crate) items: Vec<PythonListItem>,
    /// Complete correlated item sequences retained before equal lists merge their evidence.
    pub(crate) variants: Vec<PythonListVariant>,
}

impl PythonList {
    pub(super) fn new(items: Vec<PythonListItem>) -> Self {
        Self {
            variants: vec![PythonListVariant {
                items: items.clone(),
                constraints: BranchConstraints::unconstrained(),
            }],
            items,
        }
    }

    pub(super) fn append(&mut self, item: &PythonListItem) {
        self.items.push(item.clone());
        for variant in &mut self.variants {
            if !is_list_variant_limit_unknown(&variant.items) {
                variant.items.push(item.clone());
            }
        }
    }

    pub(super) fn extend(&mut self, extension: &Self, operation_origin: Origin) {
        self.items.extend(extension.items.clone());
        let mut variants = Vec::with_capacity(MAX_PYTHON_ALTERNATIVES + 1);
        let mut overflowed = self
            .variants
            .iter()
            .chain(&extension.variants)
            .any(|variant| is_list_variant_limit_unknown(&variant.items));

        'products: for left in &self.variants {
            if is_list_variant_limit_unknown(&left.items) {
                continue;
            }
            for right in &extension.variants {
                if is_list_variant_limit_unknown(&right.items) {
                    continue;
                }
                let constraints = left.constraints.intersection(&right.constraints);
                if constraints.alternatives.is_empty() {
                    continue;
                }
                let mut items = left.items.clone();
                items.extend(right.items.clone());
                variants.push(PythonListVariant { items, constraints });
                if variants.len() > MAX_PYTHON_ALTERNATIVES {
                    overflowed = true;
                    break 'products;
                }
            }
        }
        self.variants = variants;
        if overflowed {
            self.variants
                .push(list_variant_limit_unknown(operation_origin));
        }
        self.normalize(Some(operation_origin));
    }

    pub(super) fn insert(&mut self, index: usize, item: &PythonListItem) {
        self.items.insert(index, item.clone());
        for variant in &mut self.variants {
            if !is_list_variant_limit_unknown(&variant.items) {
                variant.items.insert(index, item.clone());
            }
        }
    }

    pub(super) fn remove(&mut self, index: usize) {
        self.items.remove(index);
        for variant in &mut self.variants {
            if !is_list_variant_limit_unknown(&variant.items) {
                variant.items.remove(index);
            }
        }
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        normalize_list_items(&mut self.items);
        for variant in &mut self.variants {
            normalize_list_items(&mut variant.items);
        }
        self.variants
            .sort_by_cached_key(|variant| format!("{variant:?}"));
        self.variants.dedup();

        let had_remainder = self
            .variants
            .iter()
            .any(|variant| is_list_variant_limit_unknown(&variant.items));
        let existing_remainder_origin = self
            .variants
            .iter()
            .find_map(|variant| list_variant_limit_origin(&variant.items));
        let mut exact_count = 0;
        self.variants.retain(|variant| {
            if is_list_variant_limit_unknown(&variant.items) {
                return false;
            }
            exact_count += 1;
            exact_count <= MAX_PYTHON_ALTERNATIVES
        });
        let overflowed = exact_count > MAX_PYTHON_ALTERNATIVES || had_remainder;
        if overflowed {
            let origin = operation_origin
                .or(existing_remainder_origin)
                .expect("a list alternative remainder must have an origin");
            self.variants.push(list_variant_limit_unknown(origin));
        }
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        list_items_same_semantic_value(&self.items, &other.items)
    }

    fn merge_semantically_equal(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        debug_assert!(self.same_semantic_value(&incoming));
        merge_semantically_equal_list_items(&mut self.items, incoming.items, operation_origin);
        self.variants.extend(incoming.variants);
        self.normalize(operation_origin);
    }
}

pub(super) fn is_list_variant_limit_unknown(variant: &[PythonListItem]) -> bool {
    matches!(
        variant,
        [PythonListItem::UnknownUnpack(PythonUnknown {
            cause: PythonUnknownCause::AlternativeLimitExceeded,
            ..
        })]
    )
}

fn list_variant_limit_origin(variant: &[PythonListItem]) -> Option<Origin> {
    let [PythonListItem::UnknownUnpack(unknown)] = variant else {
        return None;
    };
    (unknown.cause == PythonUnknownCause::AlternativeLimitExceeded)
        .then_some(unknown.origin)
        .flatten()
}

fn list_variant_limit_unknown(origin: Origin) -> PythonListVariant {
    PythonListVariant {
        items: vec![PythonListItem::UnknownUnpack(PythonUnknown {
            cause: PythonUnknownCause::AlternativeLimitExceeded,
            origin: Some(origin),
        })],
        constraints: BranchConstraints::unconstrained(),
    }
}

fn normalize_list_items(items: &mut [PythonListItem]) {
    for item in items {
        if let PythonListItem::Value(value) = item {
            value.normalize();
        }
    }
}

fn list_items_same_semantic_value(left: &[PythonListItem], right: &[PythonListItem]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| match (left, right) {
                (PythonListItem::Value(left), PythonListItem::Value(right)) => {
                    left.same_semantic_value(right)
                }
                (PythonListItem::UnknownElement(left), PythonListItem::UnknownElement(right))
                | (PythonListItem::UnknownUnpack(left), PythonListItem::UnknownUnpack(right)) => {
                    left.cause == right.cause
                }
                (
                    PythonListItem::Value(_)
                    | PythonListItem::UnknownElement(_)
                    | PythonListItem::UnknownUnpack(_),
                    _,
                ) => false,
            })
}

fn merge_semantically_equal_list_items(
    existing: &mut [PythonListItem],
    incoming: Vec<PythonListItem>,
    operation_origin: Option<Origin>,
) {
    for (existing, incoming) in existing.iter_mut().zip(incoming) {
        match (existing, incoming) {
            (PythonListItem::Value(existing), PythonListItem::Value(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin);
            }
            (
                PythonListItem::UnknownElement(existing),
                PythonListItem::UnknownElement(incoming),
            )
            | (PythonListItem::UnknownUnpack(existing), PythonListItem::UnknownUnpack(incoming)) => {
                existing.origin = earliest_origin(existing.origin, incoming.origin);
            }
            _ => unreachable!("semantic equality requires matching list item variants"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonListItem {
    Value(PythonValue),
    UnknownElement(PythonUnknown),
    UnknownUnpack(PythonUnknown),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonDict {
    pub(crate) items: Vec<PythonDictItem>,
}

impl PythonDict {
    #[cfg(test)]
    fn string_key(&self, key: &str) -> PythonDictLookup<'_> {
        let mut uncertain = false;
        for item in self.items.iter().rev() {
            match item {
                PythonDictItem::Entry {
                    key: candidate,
                    value,
                } if matches!(&candidate.kind, PythonValueKind::Str(candidate) if candidate == key) =>
                {
                    return PythonDictLookup {
                        value: Some(value),
                        uncertain,
                    };
                }
                PythonDictItem::UnknownUnpack(_) => uncertain = true,
                PythonDictItem::Entry { .. } => {}
            }
        }
        PythonDictLookup {
            value: None,
            uncertain,
        }
    }

    fn normalize(&mut self) {
        for item in &mut self.items {
            if let PythonDictItem::Entry { key, value } = item {
                key.normalize();
                value.normalize();
            }
        }
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        self.items.len() == other.items.len()
            && self
                .items
                .iter()
                .zip(&other.items)
                .all(|(left, right)| match (left, right) {
                    (
                        PythonDictItem::Entry {
                            key: left_key,
                            value: left_value,
                        },
                        PythonDictItem::Entry {
                            key: right_key,
                            value: right_value,
                        },
                    ) => {
                        left_key.same_semantic_value(right_key)
                            && left_value.same_semantic_value(right_value)
                    }
                    (PythonDictItem::UnknownUnpack(left), PythonDictItem::UnknownUnpack(right)) => {
                        left.cause == right.cause
                    }
                    (PythonDictItem::Entry { .. } | PythonDictItem::UnknownUnpack(_), _) => false,
                })
    }

    fn merge_semantically_equal(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        debug_assert!(self.same_semantic_value(&incoming));
        for (existing, incoming) in self.items.iter_mut().zip(incoming.items) {
            match (existing, incoming) {
                (
                    PythonDictItem::Entry {
                        key: existing_key,
                        value: existing_value,
                    },
                    PythonDictItem::Entry {
                        key: incoming_key,
                        value: incoming_value,
                    },
                ) => {
                    existing_key.merge_semantically_equal(incoming_key, operation_origin);
                    existing_value.merge_semantically_equal(incoming_value, operation_origin);
                }
                (
                    PythonDictItem::UnknownUnpack(existing),
                    PythonDictItem::UnknownUnpack(incoming),
                ) => {
                    existing.origin = earliest_origin(existing.origin, incoming.origin);
                }
                _ => unreachable!("semantic equality requires matching dictionary item variants"),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonDictItem {
    Entry {
        key: PythonValue,
        value: PythonValue,
    },
    UnknownUnpack(PythonUnknown),
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PythonDictLookup<'a> {
    value: Option<&'a PythonValue>,
    uncertain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonUnknown {
    pub(crate) cause: PythonUnknownCause,
    pub(crate) origin: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonUnknownCause {
    UnsupportedExpression,
    UnsupportedMutation,
    InvalidImport(PythonImportError),
    ImportNotFound(PythonModuleName),
    SkippedExternal(PythonModuleName),
    Unreadable(FileReadError),
    SyntaxErrors(Vec<PythonSyntaxError>),
    Cycle,
    AlternativeLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonNamespaceCause {
    pub(crate) unknown: PythonUnknown,
    pub(crate) constraints: BranchConstraints,
}

impl PythonNamespaceCause {
    pub(super) fn unconstrained(unknown: PythonUnknown) -> Self {
        Self {
            unknown,
            constraints: BranchConstraints::unconstrained(),
        }
    }

    pub(super) fn select_branch(&mut self, join: Origin, arm: usize) {
        self.constraints.select(join, arm);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonNamespaceRemainder {
    pub(crate) causes: Vec<PythonNamespaceCause>,
}

impl PythonNamespaceRemainder {
    pub(super) fn new(mut causes: Vec<PythonNamespaceCause>) -> Self {
        causes.sort_by_key(|cause| {
            (
                format!("{:?}", cause.unknown.cause),
                cause
                    .unknown
                    .origin
                    .map(|origin| format!("{:?}", origin.file)),
                cause.unknown.origin.map(|origin| origin.span.start()),
                cause.unknown.origin.map(|origin| origin.span.length()),
                format!("{:?}", cause.constraints),
            )
        });
        let mut normalized: Vec<PythonNamespaceCause> = Vec::new();
        for cause in causes {
            if let Some(existing) = normalized
                .iter_mut()
                .find(|existing| existing.unknown == cause.unknown)
            {
                existing.constraints.merge(cause.constraints);
            } else {
                normalized.push(cause);
            }
        }
        Self { causes: normalized }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonModuleEvaluation {
    pub(super) values: PythonModuleValuesOutcome,
    pub(super) dependencies: PythonModuleDependencies,
    cycle_seed: bool,
}

impl PythonModuleEvaluation {
    pub(super) fn evaluated(
        values: PythonModuleValuesOutcome,
        dependencies: PythonModuleDependencies,
    ) -> Self {
        Self {
            values,
            dependencies,
            cycle_seed: false,
        }
    }

    pub(super) fn cycle_seed() -> Self {
        Self {
            values: PythonModuleValuesOutcome::Readable(PythonModuleValues::default()),
            dependencies: PythonModuleDependencies::default(),
            cycle_seed: true,
        }
    }

    pub(super) const fn is_cycle_seed(&self) -> bool {
        self.cycle_seed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PythonModuleDependencies {
    pub(crate) files: Vec<File>,
    pub(crate) imports: Vec<PythonImportOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonImportOutcome {
    Resolved {
        origin: Origin,
        file: File,
    },
    InvalidImport {
        origin: Origin,
        reason: PythonImportError,
    },
    NotFound {
        origin: Origin,
        module: PythonModuleName,
    },
    SkippedExternal {
        origin: Origin,
        module: PythonModuleName,
    },
    Unreadable {
        origin: Origin,
        file: File,
        error: FileReadError,
    },
    SyntaxErrors {
        origin: Origin,
        file: File,
        errors: Vec<PythonSyntaxError>,
    },
    Cycle {
        origin: Origin,
        file: File,
    },
}

fn normalize_value_evidence(evidence: &mut Vec<PythonValueEvidence>) {
    evidence.sort_by_key(|evidence| {
        (
            origin_sort_key(&evidence.origin),
            format!("{:?}", evidence.constraints),
        )
    });
    let mut normalized: Vec<PythonValueEvidence> = Vec::new();
    for evidence in std::mem::take(evidence) {
        if let Some(existing) = normalized
            .iter_mut()
            .find(|existing| existing.origin == evidence.origin)
        {
            existing.constraints.merge(evidence.constraints);
        } else {
            normalized.push(evidence);
        }
    }
    *evidence = normalized;
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

fn earliest_origin(left: Option<Origin>, right: Option<Origin>) -> Option<Origin> {
    match (left, right) {
        (Some(left), Some(right)) => Some(
            [left, right]
                .into_iter()
                .min_by_key(origin_sort_key)
                .expect("two origins have a minimum"),
        ),
        (Some(origin), None) | (None, Some(origin)) => Some(origin),
        (None, None) => None,
    }
}

fn origin_sort_key(origin: &Origin) -> (String, u32, u32) {
    (
        format!("{:?}", origin.file),
        origin.span.start(),
        origin.span.length(),
    )
}

#[cfg(test)]
mod tests {
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::*;

    #[derive(Clone)]
    enum BindingValue {
        Exact(String),
        TopLevelUnknown,
        NestedUnknownElement,
        NestedUnknownUnpack,
    }

    fn test_file(index: u32) -> File {
        // SAFETY: Test indexes are strictly below `salsa::Id::MAX_U32`; synthetic
        // files are only compared as opaque origin IDs and are never read from a database.
        File::from_id(unsafe { salsa::Id::from_index(index) })
    }

    fn origin(start: u32) -> Origin {
        Origin::new(test_file(0), Span::new(start, 1))
    }

    fn binding(value: BindingValue, start: u32) -> PythonBinding {
        let origin = origin(start);
        let kind = match value {
            BindingValue::Exact(value) => PythonValueKind::Str(value),
            BindingValue::TopLevelUnknown => PythonValueKind::Unknown(PythonUnknown {
                cause: PythonUnknownCause::UnsupportedExpression,
                origin: Some(origin),
            }),
            BindingValue::NestedUnknownElement => {
                PythonValueKind::List(PythonList::new(vec![PythonListItem::UnknownElement(
                    PythonUnknown {
                        cause: PythonUnknownCause::UnsupportedExpression,
                        origin: Some(origin),
                    },
                )]))
            }
            BindingValue::NestedUnknownUnpack => {
                PythonValueKind::List(PythonList::new(vec![PythonListItem::UnknownUnpack(
                    PythonUnknown {
                        cause: PythonUnknownCause::UnsupportedExpression,
                        origin: Some(origin),
                    },
                )]))
            }
        };
        PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
            value: PythonValue::known(kind, origin),
            binding_origins: vec![origin],
        })])
    }

    fn list_binding(item_starts: [u32; 2], list_start: u32) -> PythonBinding {
        let item = |value: &str, start| {
            PythonListItem::Value(PythonValue::known(
                PythonValueKind::Str(value.to_string()),
                origin(start),
            ))
        };
        let list_origin = origin(list_start);
        PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
            value: PythonValue::known(
                PythonValueKind::List(PythonList::new(vec![
                    item("first", item_starts[0]),
                    item("second", item_starts[1]),
                ])),
                list_origin,
            ),
            binding_origins: vec![list_origin],
        })])
    }

    fn correlated_list(starts: impl IntoIterator<Item = u32>) -> PythonList {
        let item = |start| {
            PythonListItem::Value(PythonValue::known(
                PythonValueKind::Str("same".to_string()),
                origin(start),
            ))
        };
        PythonList {
            items: vec![item(0)],
            variants: starts
                .into_iter()
                .map(|start| PythonListVariant {
                    items: vec![item(start)],
                    constraints: BranchConstraints::unconstrained(),
                })
                .collect(),
        }
    }

    fn joined(bindings: Vec<PythonBinding>, right_grouped: bool) -> PythonBinding {
        let overflow_origin = origin(1_000);
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
            let Some(bound) = joined.single_bound() else {
                panic!("equal values should normalize to one bound alternative");
            };
            assert_eq!(
                bound
                    .binding_origins
                    .iter()
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
        let from_a = Origin::new(test_file(0), Span::new(10, 1));
        let from_b = Origin::new(test_file(1), Span::new(20, 1));
        let exact = |origins: Vec<Origin>| {
            PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
                value: PythonValue::with_evidence(
                    PythonValueKind::Str("same".to_string()),
                    origins.clone(),
                ),
                binding_origins: origins,
            })])
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
    fn equal_list_variant_join_obeys_laws_and_retains_correlated_sequences() {
        let joined = assert_join_laws(vec![
            list_binding([10, 21], 100),
            list_binding([20, 11], 200),
            list_binding([30, 31], 300),
        ]);
        let Some(bound) = joined.single_bound() else {
            panic!("equal lists should normalize into one alternative");
        };
        let PythonValueKind::List(list) = &bound.value.kind else {
            panic!("joined value should remain a list");
        };
        assert_eq!(list.variants.len(), 3);
        let starts = list
            .variants
            .iter()
            .map(|variant| {
                variant
                    .items
                    .iter()
                    .map(|item| match item {
                        PythonListItem::Value(value) => value
                            .origins()
                            .next()
                            .expect("test values have origins")
                            .span
                            .start(),
                        PythonListItem::UnknownElement(_) | PythonListItem::UnknownUnpack(_) => {
                            panic!("test variants contain only exact values")
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            starts,
            [vec![10, 21], vec![20, 11], vec![30, 31]]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn list_extension_has_exact_boundary_and_unknown_remainder() {
        let mut at_limit = correlated_list(0..8);
        at_limit.extend(&correlated_list(100..108), origin(1_000));
        assert_eq!(at_limit.variants.len(), 64);
        assert!(
            !at_limit
                .variants
                .iter()
                .any(|variant| is_list_variant_limit_unknown(&variant.items))
        );

        let mut overflowed = correlated_list(0..8);
        overflowed.extend(&correlated_list(100..109), origin(2_000));
        assert_eq!(overflowed.variants.len(), 65);
        let remainder = overflowed
            .variants
            .iter()
            .filter_map(|variant| list_variant_limit_origin(&variant.items))
            .collect::<Vec<_>>();
        assert_eq!(remainder, [origin(2_000)]);
    }

    #[test]
    fn list_variant_merge_is_capped_at_the_exact_boundary() {
        let mut at_limit = correlated_list(0..32);
        at_limit.merge_semantically_equal(correlated_list(32..64), Some(origin(1_000)));
        assert_eq!(at_limit.variants.len(), 64);
        assert!(
            !at_limit
                .variants
                .iter()
                .any(|variant| is_list_variant_limit_unknown(&variant.items))
        );

        let mut overflowed = correlated_list(0..32);
        overflowed.merge_semantically_equal(correlated_list(32..65), Some(origin(2_000)));
        assert_eq!(overflowed.variants.len(), 65);
        assert_eq!(
            overflowed
                .variants
                .iter()
                .filter_map(|variant| list_variant_limit_origin(&variant.items))
                .collect::<Vec<_>>(),
            [origin(2_000)]
        );
    }

    #[test]
    fn repeated_list_self_extension_stays_bounded_and_uses_each_operation_origin() {
        let mut list = correlated_list(0..2);
        for operation_start in [100, 200, 300, 400] {
            let extension = list.clone();
            list.extend(&extension, origin(operation_start));
            assert!(list.variants.len() <= MAX_PYTHON_ALTERNATIVES + 1);
        }

        assert_eq!(list.variants.len(), 65);
        let remainder = list
            .variants
            .iter()
            .filter_map(|variant| list_variant_limit_origin(&variant.items))
            .collect::<Vec<_>>();
        assert_eq!(remainder, [origin(400)]);
    }

    #[test]
    fn binding_join_obeys_laws_at_and_over_the_alternative_limit() {
        let alternatives = |count: u32| {
            (0..count)
                .map(|index| binding(BindingValue::Exact(format!("{index:03}")), index))
                .collect::<Vec<_>>()
        };

        let at_limit = assert_join_laws(alternatives(64));
        assert_eq!(at_limit.alternatives().len(), 64);
        assert!(!at_limit.alternatives().any(is_alternative_limit_unknown));

        let overflowed = assert_join_laws(alternatives(65));
        assert_eq!(overflowed.alternatives().len(), 65);
        let PythonBindingAlternative::Bound(overflow) = overflowed
            .alternatives()
            .find(|alternative| is_alternative_limit_unknown(alternative))
            .expect("overflow should add a typed unknown remainder")
        else {
            unreachable!();
        };
        let PythonValueKind::Unknown(unknown) = &overflow.value.kind else {
            unreachable!();
        };
        assert_eq!(
            unknown.origin.expect("join origin should be retained").span,
            Span::new(1_000, 1),
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
    fn dictionary_lookup_respects_ordered_unknown_unpacks() {
        let string = |value: &str| {
            PythonValue::with_evidence(PythonValueKind::Str(value.to_string()), Vec::new())
        };
        let unknown = PythonUnknown {
            cause: PythonUnknownCause::UnsupportedExpression,
            origin: None,
        };
        let dict = PythonDict {
            items: vec![
                PythonDictItem::Entry {
                    key: string("key"),
                    value: string("old"),
                },
                PythonDictItem::UnknownUnpack(unknown),
                PythonDictItem::Entry {
                    key: string("later"),
                    value: string("exact"),
                },
            ],
        };

        assert_eq!(dict.string_key("key").value, Some(&string("old")));
        assert!(dict.string_key("key").uncertain);
        assert_eq!(dict.string_key("later").value, Some(&string("exact")));
        assert!(!dict.string_key("later").uncertain);
        assert!(dict.string_key("missing").uncertain);
    }
}
