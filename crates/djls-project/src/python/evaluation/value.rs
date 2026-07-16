use camino::Utf8PathBuf;
use djls_source::FileReadError;
use djls_source::Origin;

use super::BranchConstraints;
use super::MAX_EXACT_PYTHON_ALTERNATIVES;
use super::MutableOrigins;
use super::origin_sort_key;
use crate::python::PythonModuleName;
use crate::python::PythonSyntaxError;
use crate::python::module::PythonImportError;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonValueEvidence {
    origin: Origin,
    constraints: BranchConstraints,
    is_mutable_identity: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PythonValueEvidenceSet(Vec<PythonValueEvidence>);

impl PythonValueEvidenceSet {
    fn one(origin: Origin, is_mutable_identity: bool) -> Self {
        Self(vec![PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
            is_mutable_identity,
        }])
    }

    fn from_origins(origins: impl IntoIterator<Item = Origin>, is_mutable_identity: bool) -> Self {
        let mut evidence = Self::default();
        evidence.extend(origins.into_iter().map(|origin| PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
            is_mutable_identity,
        }));
        evidence
    }

    fn insert(&mut self, evidence: PythonValueEvidence) {
        self.0.push(evidence);
        self.normalize();
    }

    fn extend(&mut self, evidence: impl IntoIterator<Item = PythonValueEvidence>) {
        self.0.extend(evidence);
        self.normalize();
    }

    fn merge(&mut self, incoming: Self) {
        self.extend(incoming.0);
    }

    fn origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.0.iter().map(|evidence| evidence.origin)
    }

    fn mutable_origins(&self) -> impl Iterator<Item = Origin> + '_ {
        self.0
            .iter()
            .filter(|evidence| evidence.is_mutable_identity)
            .map(|evidence| evidence.origin)
    }

    fn origins_with_constraints(&self) -> impl Iterator<Item = (Origin, &BranchConstraints)> {
        self.0
            .iter()
            .map(|evidence| (evidence.origin, &evidence.constraints))
    }

    fn rebase(&mut self, origin: Origin, is_mutable_identity: bool) {
        self.0.clear();
        self.0.push(PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
            is_mutable_identity,
        });
    }

    fn record(&mut self, origin: Origin) {
        self.insert(PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
            is_mutable_identity: false,
        });
    }

    fn constrain(&mut self, constraints: &BranchConstraints) {
        for evidence in &mut self.0 {
            evidence.constraints = evidence.constraints.intersection(constraints);
        }
    }

    fn normalize(&mut self) {
        self.0.sort_by_key(|evidence| {
            (
                origin_sort_key(&evidence.origin),
                format!("{:?}", evidence.constraints),
            )
        });
        let mut normalized: Vec<PythonValueEvidence> = Vec::with_capacity(self.0.len());
        for evidence in std::mem::take(&mut self.0) {
            if let Some(existing) = normalized
                .iter_mut()
                .find(|existing| existing.origin == evidence.origin)
            {
                existing.constraints.merge(evidence.constraints);
                existing.is_mutable_identity |= evidence.is_mutable_identity;
            } else {
                normalized.push(evidence);
            }
        }
        self.0 = normalized;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonValue {
    pub(crate) kind: PythonValueKind,
    evidence: PythonValueEvidenceSet,
}

impl PythonValue {
    pub(super) fn unknown(cause: PythonUnknownCause, origin: Option<Origin>) -> Self {
        Self {
            kind: PythonValueKind::Unknown(PythonUnknown { cause, origin }),
            evidence: origin.map_or_else(PythonValueEvidenceSet::default, |origin| {
                PythonValueEvidenceSet::one(origin, false)
            }),
        }
    }

    pub(super) fn known(kind: PythonValueKind, origin: Origin) -> Self {
        let is_mutable_identity =
            matches!(&kind, PythonValueKind::List(_) | PythonValueKind::Dict(_));
        Self {
            kind,
            evidence: PythonValueEvidenceSet::one(origin, is_mutable_identity),
        }
    }

    pub(super) fn known_tuple(list: PythonList, origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::List(list),
            evidence: PythonValueEvidenceSet::one(origin, false),
        }
    }

    pub(super) fn with_evidence(
        kind: PythonValueKind,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Self {
        let is_mutable_identity =
            matches!(&kind, PythonValueKind::List(_) | PythonValueKind::Dict(_));
        Self {
            kind,
            evidence: PythonValueEvidenceSet::from_origins(origins, is_mutable_identity),
        }
    }

    pub(crate) fn origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.evidence.origins()
    }

    pub(super) fn mutable_origins(&self) -> impl Iterator<Item = Origin> + '_ {
        self.evidence.mutable_origins()
    }

    pub(super) fn is_mutable_container(&self) -> bool {
        self.mutable_origins().next().is_some()
    }

    pub(super) fn reachable_mutable_origins(&self) -> MutableOrigins {
        let mut origins = MutableOrigins::default();
        for origin in self.mutable_origins() {
            origins.insert(origin);
        }
        match &self.kind {
            PythonValueKind::List(list) => {
                for item in list.semantic_items() {
                    if let PythonListItem::Value(value) = item {
                        origins.extend(value.reachable_mutable_origins().iter());
                    }
                }
            }
            PythonValueKind::Dict(dict) => {
                for item in &dict.items {
                    if let PythonDictItem::Entry { key, value } = item {
                        origins.extend(key.reachable_mutable_origins().iter());
                        origins.extend(value.reachable_mutable_origins().iter());
                    }
                }
            }
            PythonValueKind::Unknown(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_) => {}
        }
        origins
    }

    pub(super) fn mutable_origin_occurrences(&self, wanted: &MutableOrigins) -> usize {
        let own = usize::from(
            self.mutable_origins()
                .any(|origin| wanted.contains(&origin)),
        );
        own + match &self.kind {
            PythonValueKind::List(list) => list
                .semantic_items()
                .iter()
                .filter_map(|item| match item {
                    PythonListItem::Value(value) => Some(value.mutable_origin_occurrences(wanted)),
                    PythonListItem::UnknownElement(_) | PythonListItem::UnknownUnpack(_) => None,
                })
                .sum::<usize>(),
            PythonValueKind::Dict(dict) => dict
                .items
                .iter()
                .filter_map(|item| match item {
                    PythonDictItem::Entry { key, value } => Some(
                        key.mutable_origin_occurrences(wanted)
                            + value.mutable_origin_occurrences(wanted),
                    ),
                    PythonDictItem::UnknownUnpack(_) => None,
                })
                .sum(),
            PythonValueKind::Unknown(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_) => 0,
        }
    }

    pub(crate) fn origins_with_constraints(
        &self,
    ) -> impl Iterator<Item = (Origin, &BranchConstraints)> {
        self.evidence.origins_with_constraints()
    }

    pub(super) fn rebase_origin(&mut self, origin: Origin) {
        let is_mutable_identity = self.is_mutable_container();
        self.evidence.rebase(origin, is_mutable_identity);
    }

    pub(super) fn record_origin(&mut self, origin: Origin) {
        self.evidence.record(origin);
    }

    pub(super) fn normalize(&mut self) {
        self.evidence.normalize();
        self.kind.normalize();
    }

    pub(super) fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        self.evidence.constrain(constraints);
        match &mut self.kind {
            PythonValueKind::List(list) => list.constrain_value_evidence(constraints),
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

    pub(super) fn same_semantic_value(&self, other: &Self) -> bool {
        self.kind.same_semantic_value(&other.kind)
    }

    pub(super) fn merge_semantically_equal(
        &mut self,
        incoming: Self,
        operation_origin: Option<Origin>,
    ) {
        debug_assert!(self.same_semantic_value(&incoming));
        self.evidence.merge(incoming.evidence);
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
struct ConstrainedExactListAlternative {
    items: Vec<PythonListItem>,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonListAlternativeRemainder {
    origin: Origin,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonListAlternatives {
    exact: Vec<ConstrainedExactListAlternative>,
    remainder: Option<PythonListAlternativeRemainder>,
}

impl PythonListAlternatives {
    fn one(items: Vec<PythonListItem>) -> Self {
        Self {
            exact: vec![ConstrainedExactListAlternative {
                items,
                constraints: BranchConstraints::unconstrained(),
            }],
            remainder: None,
        }
    }

    fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        for alternative in &mut self.exact {
            alternative.constraints = alternative.constraints.intersection(constraints);
            constrain_list_item_evidence(&mut alternative.items, constraints);
        }
        if let Some(remainder) = &mut self.remainder {
            remainder.constraints = remainder.constraints.intersection(constraints);
        }
        self.normalize(None);
    }

    fn append(&mut self, item: &PythonListItem) {
        for alternative in &mut self.exact {
            alternative.items.push(item.clone());
        }
        self.debug_assert_invariants();
    }

    fn extend(&mut self, extension: &Self, operation_origin: Origin) {
        let mut exact = Vec::with_capacity(self.exact.len().saturating_mul(extension.exact.len()));
        for left in &self.exact {
            for right in &extension.exact {
                let constraints = left.constraints.intersection(&right.constraints);
                if constraints.is_impossible() {
                    continue;
                }
                let mut items = left.items.clone();
                items.extend(right.items.clone());
                exact.push(ConstrainedExactListAlternative { items, constraints });
            }
        }

        let mut remainder_constraints = None;
        if let Some(right_remainder) = &extension.remainder {
            for left in &self.exact {
                merge_feasible_constraints(
                    &mut remainder_constraints,
                    left.constraints.intersection(&right_remainder.constraints),
                );
            }
        }
        if let Some(left_remainder) = &self.remainder {
            for right in &extension.exact {
                merge_feasible_constraints(
                    &mut remainder_constraints,
                    left_remainder.constraints.intersection(&right.constraints),
                );
            }
        }
        if let (Some(left_remainder), Some(right_remainder)) =
            (&self.remainder, &extension.remainder)
        {
            merge_feasible_constraints(
                &mut remainder_constraints,
                left_remainder
                    .constraints
                    .intersection(&right_remainder.constraints),
            );
        }

        *self = Self {
            exact,
            remainder: remainder_constraints.map(|constraints| PythonListAlternativeRemainder {
                origin: operation_origin,
                constraints,
            }),
        };
        self.normalize(Some(operation_origin));
    }

    fn insert(&mut self, index: usize, item: &PythonListItem) {
        for alternative in &mut self.exact {
            alternative.items.insert(index, item.clone());
        }
        self.debug_assert_invariants();
    }

    fn remove(&mut self, index: usize) {
        for alternative in &mut self.exact {
            alternative.items.remove(index);
        }
        self.debug_assert_invariants();
    }

    fn try_mutate_indexed_value(
        &mut self,
        index: usize,
        mutate: &impl Fn(&mut PythonValue) -> bool,
    ) -> bool {
        let mut updated = self.clone();
        for alternative in &mut updated.exact {
            let Some(PythonListItem::Value(next)) = alternative.items.get_mut(index) else {
                return false;
            };
            if !mutate(next) {
                return false;
            }
        }
        updated.debug_assert_invariants();
        *self = updated;
        true
    }

    fn merge(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        self.exact.extend(incoming.exact);
        self.remainder = match (self.remainder.take(), incoming.remainder) {
            (Some(existing), Some(incoming)) => {
                let mut constraints = existing.constraints;
                constraints.merge(incoming.constraints);
                Some(PythonListAlternativeRemainder {
                    origin: earliest_origin(Some(existing.origin), Some(incoming.origin))
                        .expect("two remainder origins have an earliest origin"),
                    constraints,
                })
            }
            (Some(remainder), None) | (None, Some(remainder)) => Some(remainder),
            (None, None) => None,
        };
        self.normalize(operation_origin);
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        for alternative in &mut self.exact {
            normalize_list_items(&mut alternative.items);
        }
        self.exact
            .retain(|alternative| !alternative.constraints.is_impossible());
        if self
            .remainder
            .as_ref()
            .is_some_and(|remainder| remainder.constraints.is_impossible())
        {
            self.remainder = None;
        }
        self.exact
            .sort_by_cached_key(|alternative| format!("{alternative:?}"));
        self.exact.dedup();

        if self.exact.len() > MAX_EXACT_PYTHON_ALTERNATIVES {
            let omitted = self.exact.split_off(MAX_EXACT_PYTHON_ALTERNATIVES);
            let mut constraints = self.remainder.take().map(|remainder| remainder.constraints);
            for alternative in omitted {
                merge_feasible_constraints(&mut constraints, alternative.constraints);
            }
            self.remainder = constraints.map(|constraints| PythonListAlternativeRemainder {
                origin: operation_origin
                    .expect("truncating list alternatives requires an operation origin"),
                constraints,
            });
        }
        self.debug_assert_invariants();
    }

    fn debug_assert_invariants(&self) {
        debug_assert!(self.exact.len() <= MAX_EXACT_PYTHON_ALTERNATIVES);
        debug_assert!(
            self.exact
                .iter()
                .all(|alternative| !alternative.constraints.is_impossible())
        );
        debug_assert!(
            self.remainder
                .as_ref()
                .is_none_or(|remainder| !remainder.constraints.is_impossible())
        );
    }
}

fn merge_feasible_constraints(
    merged: &mut Option<BranchConstraints>,
    constraints: BranchConstraints,
) {
    if constraints.is_impossible() {
        return;
    }
    if let Some(merged) = merged {
        merged.merge(constraints);
    } else {
        *merged = Some(constraints);
    }
}

pub(crate) enum PythonListAlternativeRef<'a> {
    Exact {
        items: &'a [PythonListItem],
        constraints: &'a BranchConstraints,
    },
    Remainder {
        origin: Origin,
        constraints: &'a BranchConstraints,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonList {
    /// Materialized semantic projection used for equality and ordinary value consumers.
    summary: Vec<PythonListItem>,
    /// Bounded correlated exact alternatives plus the possible unmaterialized remainder.
    alternatives: PythonListAlternatives,
}

impl PythonList {
    pub(super) fn new(summary: Vec<PythonListItem>) -> Self {
        let list = Self {
            alternatives: PythonListAlternatives::one(summary.clone()),
            summary,
        };
        list.debug_assert_semantic_equivalence();
        list
    }

    pub(crate) fn semantic_items(&self) -> &[PythonListItem] {
        &self.summary
    }

    pub(crate) fn alternatives(&self) -> impl Iterator<Item = PythonListAlternativeRef<'_>> {
        self.alternatives
            .exact
            .iter()
            .map(|alternative| PythonListAlternativeRef::Exact {
                items: &alternative.items,
                constraints: &alternative.constraints,
            })
            .chain(self.alternatives.remainder.iter().map(|remainder| {
                PythonListAlternativeRef::Remainder {
                    origin: remainder.origin,
                    constraints: &remainder.constraints,
                }
            }))
    }

    fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        constrain_list_item_evidence(&mut self.summary, constraints);
        self.alternatives.constrain_value_evidence(constraints);
        self.debug_assert_semantic_equivalence();
    }

    pub(super) fn append(&mut self, item: &PythonListItem) {
        self.summary.push(item.clone());
        self.alternatives.append(item);
        self.debug_assert_semantic_equivalence();
    }

    pub(super) fn extend(&mut self, extension: &Self, operation_origin: Origin) {
        self.summary.extend(extension.summary.clone());
        self.alternatives
            .extend(&extension.alternatives, operation_origin);
        self.debug_assert_semantic_equivalence();
    }

    pub(super) fn insert(&mut self, index: usize, item: &PythonListItem) {
        self.summary.insert(index, item.clone());
        self.alternatives.insert(index, item);
        self.debug_assert_semantic_equivalence();
    }

    pub(super) fn remove(&mut self, index: usize) {
        self.summary.remove(index);
        self.alternatives.remove(index);
        self.debug_assert_semantic_equivalence();
    }

    pub(super) fn try_mutate_indexed_value(
        &mut self,
        index: usize,
        mutate: impl Fn(&mut PythonValue) -> bool,
    ) -> bool {
        let mut summary = self.summary.clone();
        let Some(PythonListItem::Value(next)) = summary.get_mut(index) else {
            return false;
        };
        if !mutate(next) {
            return false;
        }
        let mut alternatives = self.alternatives.clone();
        if !alternatives.try_mutate_indexed_value(index, &mutate) {
            return false;
        }
        self.summary = summary;
        self.alternatives = alternatives;
        self.debug_assert_semantic_equivalence();
        true
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        normalize_list_items(&mut self.summary);
        self.alternatives.normalize(operation_origin);
        self.debug_assert_semantic_equivalence();
    }

    fn debug_assert_semantic_equivalence(&self) {
        debug_assert!(self.alternatives.exact.iter().all(|alternative| {
            list_items_same_semantic_value(&self.summary, &alternative.items)
        }));
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        list_items_same_semantic_value(&self.summary, &other.summary)
    }

    fn merge_semantically_equal(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        debug_assert!(self.same_semantic_value(&incoming));
        merge_semantically_equal_list_items(&mut self.summary, incoming.summary, operation_origin);
        self.alternatives
            .merge(incoming.alternatives, operation_origin);
        self.debug_assert_semantic_equivalence();
    }
}

fn constrain_list_item_evidence(items: &mut [PythonListItem], constraints: &BranchConstraints) {
    for item in items {
        if let PythonListItem::Value(value) = item {
            value.constrain_value_evidence(constraints);
        }
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

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::super::PythonBinding;
    use super::super::PythonBindingState;
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

    #[test]
    fn evidence_with_the_same_origin_coalesces_constraints_and_identity() {
        let evidence_origin = origin(10);
        let join = origin(20);
        let mut first_constraints = BranchConstraints::unconstrained();
        first_constraints.select(join, 0);
        let mut second_constraints = BranchConstraints::unconstrained();
        second_constraints.select(join, 1);

        let mut evidence = PythonValueEvidenceSet::default();
        evidence.insert(PythonValueEvidence {
            origin: evidence_origin,
            constraints: first_constraints.clone(),
            is_mutable_identity: false,
        });
        evidence.insert(PythonValueEvidence {
            origin: evidence_origin,
            constraints: second_constraints.clone(),
            is_mutable_identity: true,
        });

        let mut expected_constraints = first_constraints;
        expected_constraints.merge(second_constraints);
        assert_eq!(evidence.0.len(), 1);
        assert_eq!(evidence.0[0].constraints, expected_constraints);
        assert_eq!(
            evidence.mutable_origins().collect::<Vec<_>>(),
            [evidence_origin]
        );
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
        PythonBinding::bound(PythonValue::known(kind, origin), origin)
    }

    fn list_binding(item_starts: [u32; 2], list_start: u32) -> PythonBinding {
        let item = |value: &str, start| {
            PythonListItem::Value(PythonValue::known(
                PythonValueKind::Str(value.to_string()),
                origin(start),
            ))
        };
        let list_origin = origin(list_start);
        PythonBinding::bound(
            PythonValue::known(
                PythonValueKind::List(PythonList::new(vec![
                    item("first", item_starts[0]),
                    item("second", item_starts[1]),
                ])),
                list_origin,
            ),
            list_origin,
        )
    }

    fn correlated_list(starts: impl IntoIterator<Item = u32>) -> PythonList {
        let item = |start| {
            PythonListItem::Value(PythonValue::known(
                PythonValueKind::Str("same".to_string()),
                origin(start),
            ))
        };
        PythonList {
            summary: vec![item(0)],
            alternatives: PythonListAlternatives {
                exact: starts
                    .into_iter()
                    .map(|start| ConstrainedExactListAlternative {
                        items: vec![item(start)],
                        constraints: BranchConstraints::unconstrained(),
                    })
                    .collect(),
                remainder: None,
            },
        }
    }

    fn correlated_backend_list(starts: impl IntoIterator<Item = u32>) -> PythonList {
        let backend = |start| {
            let value = |kind| PythonValue::known(kind, origin(start));
            PythonListItem::Value(value(PythonValueKind::Dict(PythonDict {
                items: vec![PythonDictItem::Entry {
                    key: value(PythonValueKind::Str("DIRS".to_string())),
                    value: value(PythonValueKind::List(PythonList::new(Vec::new()))),
                }],
            })))
        };
        starts
            .into_iter()
            .map(|start| PythonList::new(vec![backend(start)]))
            .reduce(|mut correlated, alternative| {
                correlated.merge_semantically_equal(alternative, None);
                correlated
            })
            .expect("a correlated backend list needs at least one alternative")
    }

    fn dirs_value(items: &[PythonListItem]) -> &PythonValue {
        let [
            PythonListItem::Value(PythonValue {
                kind: PythonValueKind::Dict(dict),
                ..
            }),
        ] = items
        else {
            panic!("a test backend projection should contain one dictionary");
        };
        let [PythonDictItem::Entry { value, .. }] = dict.items.as_slice() else {
            panic!("a test backend should contain one DIRS entry");
        };
        value
    }

    fn assert_dirs_mutated(items: &[PythonListItem], mutation_origin: Origin) {
        let PythonListItem::Value(backend) = &items[0] else {
            panic!("a test backend should be an exact value");
        };
        assert!(backend.origins().any(|origin| origin == mutation_origin));

        let dirs = dirs_value(items);
        assert!(dirs.origins().any(|origin| origin == mutation_origin));
        let PythonValueKind::List(dirs) = &dirs.kind else {
            panic!("DIRS should remain a list");
        };
        let is_added = |items: &[PythonListItem]| {
            matches!(
                items,
                [PythonListItem::Value(PythonValue {
                    kind: PythonValueKind::Str(value),
                    ..
                })] if value == "added"
            )
        };
        assert!(is_added(dirs.semantic_items()));
        assert!(dirs.alternatives().all(|alternative| match alternative {
            PythonListAlternativeRef::Exact { items, .. } => is_added(items),
            PythonListAlternativeRef::Remainder { .. } => false,
        }));
    }

    fn append_to_dirs(
        backend: &mut PythonValue,
        argument: &PythonValue,
        mutation_origin: Origin,
    ) -> bool {
        let PythonValueKind::Dict(dict) = &mut backend.kind else {
            return false;
        };
        let Some(dirs) = dict.items.iter_mut().find_map(|item| match item {
            PythonDictItem::Entry { value, .. } => Some(value),
            PythonDictItem::UnknownUnpack(_) => None,
        }) else {
            return false;
        };
        let PythonValueKind::List(list) = &mut dirs.kind else {
            return false;
        };
        list.append(&PythonListItem::Value(argument.clone()));
        dirs.record_origin(mutation_origin);
        backend.record_origin(mutation_origin);
        true
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
        let from_a = Origin::new(test_file(0), Span::new(10, 1));
        let from_b = Origin::new(test_file(1), Span::new(20, 1));
        let exact = |origins: Vec<Origin>| {
            origins
                .into_iter()
                .map(|origin| {
                    PythonBinding::bound(
                        PythonValue::known(PythonValueKind::Str("same".to_string()), origin),
                        origin,
                    )
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
    fn equal_list_alternative_join_obeys_laws_and_retains_correlated_sequences() {
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
        assert_eq!(list.alternatives.exact.len(), 3);
        let semantic_starts = list
            .semantic_items()
            .iter()
            .map(|item| match item {
                PythonListItem::Value(value) => value
                    .origins()
                    .map(|origin| origin.span.start())
                    .collect::<Vec<_>>(),
                PythonListItem::UnknownElement(_) | PythonListItem::UnknownUnpack(_) => {
                    panic!("the semantic list should contain only exact values")
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(semantic_starts, [vec![10, 20, 30], vec![11, 21, 31]]);

        let starts = list
            .alternatives
            .exact
            .iter()
            .map(|alternative| {
                alternative
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
                            panic!("test alternatives contain only exact values")
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
    fn correlated_indexed_mutation_updates_every_projection_and_retains_recursive_origins() {
        let mut list = correlated_backend_list([10, 20, 30]);
        let argument = PythonValue::known(PythonValueKind::Str("added".to_string()), origin(200));
        let mutation_origin = origin(300);

        assert!(list.try_mutate_indexed_value(0, |backend| {
            append_to_dirs(backend, &argument, mutation_origin)
        }));

        assert_dirs_mutated(list.semantic_items(), mutation_origin);
        for alternative in list.alternatives() {
            let PythonListAlternativeRef::Exact { items, .. } = alternative else {
                panic!("the test list should not have a remainder");
            };
            assert_dirs_mutated(items, mutation_origin);
        }
    }

    #[test]
    fn failing_correlated_indexed_mutation_leaves_every_projection_unchanged() {
        let mut list = correlated_backend_list([10, 20, 30]);
        let before = list.clone();
        let mutation_origin = origin(300);

        assert!(!list.try_mutate_indexed_value(0, |backend| {
            let is_summary = backend.origins().len() > 1;
            let is_rejected_alternative = backend.origins().any(|origin| origin.span.start() == 20);
            if !is_summary && is_rejected_alternative {
                return false;
            }
            backend.record_origin(mutation_origin);
            true
        }));
        assert_eq!(list, before);
    }

    #[test]
    fn correlated_indexed_mutation_preserves_the_64_plus_remainder_state() {
        let mut list = correlated_backend_list(0..32);
        list.merge_semantically_equal(correlated_backend_list(32..65), Some(origin(1_000)));
        assert_eq!(list.alternatives.exact.len(), 64);
        let remainder = list
            .alternatives
            .remainder
            .as_mut()
            .expect("overflow should retain one typed remainder");
        remainder.constraints.select(origin(1_500), 1);
        let remainder = remainder.clone();
        let argument = PythonValue::known(PythonValueKind::Str("added".to_string()), origin(3_000));
        let mutation_origin = origin(4_000);

        assert!(list.try_mutate_indexed_value(0, |backend| {
            append_to_dirs(backend, &argument, mutation_origin)
        }));

        assert_eq!(list.alternatives.exact.len(), 64);
        for alternative in &list.alternatives.exact {
            assert_dirs_mutated(&alternative.items, mutation_origin);
        }
        assert_eq!(list.alternatives.remainder.as_ref(), Some(&remainder));
    }

    #[test]
    fn list_extension_has_exact_boundary_and_typed_remainder() {
        let mut at_limit = correlated_list(0..8);
        at_limit.extend(&correlated_list(100..108), origin(1_000));
        assert_eq!(at_limit.alternatives.exact.len(), 64);
        assert!(at_limit.alternatives.remainder.is_none());

        let mut overflowed = correlated_list(0..8);
        overflowed.extend(&correlated_list(100..109), origin(2_000));
        assert_eq!(overflowed.alternatives.exact.len(), 64);
        assert_eq!(
            overflowed
                .alternatives
                .remainder
                .as_ref()
                .map(|remainder| remainder.origin),
            Some(origin(2_000))
        );
    }

    #[test]
    fn capped_list_merge_is_idempotent_and_preserves_remainder_constraints() {
        let mut list = correlated_list(0..32);
        list.merge_semantically_equal(correlated_list(32..65), Some(origin(1_000)));
        list.alternatives
            .remainder
            .as_mut()
            .expect("overflow should retain one typed remainder")
            .constraints
            .select(origin(1_500), 1);
        let before = list.clone();

        list.merge_semantically_equal(before.clone(), Some(origin(2_000)));

        assert_eq!(list, before);
    }

    #[test]
    fn list_extension_retains_only_feasible_remainder_products() {
        let join = origin(1_000);
        let mut left = correlated_list([0]);
        left.alternatives.exact[0].constraints.select(join, 0);
        let mut remainder_constraints = BranchConstraints::unconstrained();
        remainder_constraints.select(join, 1);
        left.alternatives.remainder = Some(PythonListAlternativeRemainder {
            origin: origin(1_500),
            constraints: remainder_constraints.clone(),
        });

        let mut exact_right = correlated_list([100]);
        exact_right.alternatives.exact[0]
            .constraints
            .select(join, 0);
        let mut exact_product = left.clone();
        exact_product.extend(&exact_right, origin(2_000));
        assert_eq!(exact_product.alternatives.exact.len(), 1);
        assert!(exact_product.alternatives.remainder.is_none());

        let mut remainder_right = correlated_list([200]);
        remainder_right.alternatives.exact[0]
            .constraints
            .select(join, 1);
        left.extend(&remainder_right, origin(3_000));
        assert!(left.alternatives.exact.is_empty());
        assert_eq!(
            left.alternatives
                .remainder
                .as_ref()
                .map(|remainder| &remainder.constraints),
            Some(&remainder_constraints),
        );
    }

    #[test]
    fn list_alternative_merge_is_capped_at_the_exact_boundary() {
        let mut at_limit = correlated_list(0..32);
        at_limit.merge_semantically_equal(correlated_list(32..64), Some(origin(1_000)));
        assert_eq!(at_limit.alternatives.exact.len(), 64);
        assert!(at_limit.alternatives.remainder.is_none());

        let mut overflowed = correlated_list(0..32);
        overflowed.merge_semantically_equal(correlated_list(32..65), Some(origin(2_000)));
        assert_eq!(overflowed.alternatives.exact.len(), 64);
        assert_eq!(
            overflowed
                .alternatives
                .remainder
                .as_ref()
                .map(|remainder| remainder.origin),
            Some(origin(2_000))
        );
    }

    #[test]
    fn repeated_list_self_extension_stays_bounded_and_uses_each_operation_origin() {
        let mut list = correlated_list(0..2);
        for operation_start in [100, 200, 300, 400] {
            let extension = list.clone();
            list.extend(&extension, origin(operation_start));
            assert!(list.alternatives.exact.len() <= MAX_EXACT_PYTHON_ALTERNATIVES);
        }

        assert_eq!(list.alternatives.exact.len(), 64);
        assert_eq!(
            list.alternatives
                .remainder
                .as_ref()
                .map(|remainder| remainder.origin),
            Some(origin(400))
        );
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
        assert!(
            !at_limit
                .alternatives()
                .any(PythonBindingState::is_limit_remainder)
        );

        let overflowed = assert_join_laws(alternatives(65));
        assert_eq!(overflowed.alternatives().len(), 65);
        let PythonBindingState::Bound(overflow) = overflowed
            .alternatives()
            .find(|state| state.is_limit_remainder())
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
