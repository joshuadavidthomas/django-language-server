use djls_source::Origin;

use super::BranchConstraints;
use super::CanonicalOrigins;
use super::MAX_EXACT_PYTHON_ALTERNATIVES;
use super::PythonUnknown;
use super::PythonValue;
use super::PythonValueKind;
use super::ReachableAllocationSites;
use super::allocation::AllocationSites;
use super::value::PythonIterable;
use super::value::PythonIterableKnowledge;

/// A concrete Python `list` value: shared sequence facts plus the constrained,
/// non-empty allocation sites that give the list its mutable object identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonList {
    sequence: SequenceFacts,
    allocation_sites: AllocationSites,
}

impl PythonList {
    pub(super) fn new(summary: Vec<PythonSequenceItem>, origin: Origin) -> Self {
        Self {
            sequence: SequenceFacts::new(summary),
            allocation_sites: AllocationSites::one(origin),
        }
    }

    pub(crate) fn semantic_items(&self) -> &[PythonSequenceItem] {
        self.sequence.semantic_items()
    }

    pub(super) fn is_authoritative(&self) -> bool {
        self.sequence.is_authoritative()
    }

    pub(super) fn allocation_sites(&self) -> &AllocationSites {
        &self.allocation_sites
    }

    pub(super) fn append(&mut self, item: &PythonSequenceItem) {
        self.sequence.append(item);
    }

    pub(super) fn append_value(&mut self, value: PythonValue) {
        self.append(&PythonSequenceItem::Value(value));
    }

    fn extend(&mut self, extension: &SequenceFacts, operation_origin: Origin) {
        self.sequence.extend(extension, operation_origin);
    }

    pub(super) fn concatenate(&mut self, extension: &Self, operation_origin: Origin) {
        self.extend(&extension.sequence, operation_origin);
    }

    /// Extend the list by consuming an iterable source in place. Returns whether
    /// the source was iterable at all: list/tuple sources contribute exact
    /// ordered facts, known-but-imprecise (string/mapping) and indeterminate
    /// (unknown/path) sources contribute a typed unknown-unpack, and a
    /// definitely non-iterable source (bool) fails without touching the list.
    pub(super) fn extend_from(
        &mut self,
        source: &PythonValue,
        operation_origin: Origin,
    ) -> Option<()> {
        self.sequence.extend_from_iterable(source, operation_origin)
    }

    pub(super) fn len(&self) -> usize {
        self.semantic_items().len()
    }

    pub(super) fn insert_value(&mut self, index: usize, value: PythonValue) {
        self.sequence
            .insert(index, &PythonSequenceItem::Value(value));
    }

    pub(super) fn remove_str(&mut self, needle: &str) -> bool {
        let Some(index) = self.semantic_items().iter().position(|item| {
            matches!(
                item,
                PythonSequenceItem::Value(PythonValue {
                    kind: PythonValueKind::Str(candidate),
                    ..
                }) if candidate == needle
            )
        }) else {
            return false;
        };
        self.sequence.remove(index);
        true
    }

    pub(super) fn try_mutate_indexed_value(
        &mut self,
        index: usize,
        mutate: impl Fn(&mut PythonValue) -> bool,
    ) -> bool {
        self.sequence.try_mutate_indexed_value(index, mutate)
    }

    pub(super) fn allocation_site_occurrences(&self, wanted: &ReachableAllocationSites) -> usize {
        self.sequence.allocation_site_occurrences(wanted)
    }

    pub(super) fn contains_origin(&self, wanted: Origin) -> bool {
        self.sequence.contains_origin(wanted)
    }

    /// Rebase the allocation identity to a single fresh site at `origin`, used
    /// when a binary concatenation allocates a new list at the operation.
    pub(super) fn rebase_allocation_site(&mut self, origin: Origin) {
        self.allocation_sites.rebase(origin);
    }

    pub(super) fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        self.sequence.constrain_value_evidence(constraints);
        self.allocation_sites.constrain(constraints);
    }

    pub(super) fn normalize(&mut self) {
        self.sequence.normalize(None);
    }

    pub(super) fn same_semantic_value(&self, other: &Self) -> bool {
        self.sequence.same_semantic_value(&other.sequence)
    }

    pub(super) fn merge_semantically_equal(
        &mut self,
        incoming: Self,
        operation_origin: Option<Origin>,
    ) {
        self.sequence
            .merge_semantically_equal(incoming.sequence, operation_origin);
        self.allocation_sites.merge(incoming.allocation_sites);
    }
}

/// A concrete Python `tuple` value: shared sequence facts with no allocation
/// identity. Tuples cannot own allocation sites, though nested mutable values
/// remain reachable through tuple indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonTuple {
    sequence: SequenceFacts,
}

impl PythonTuple {
    pub(super) fn new(summary: Vec<PythonSequenceItem>) -> Self {
        Self {
            sequence: SequenceFacts::new(summary),
        }
    }

    pub(crate) fn semantic_items(&self) -> &[PythonSequenceItem] {
        self.sequence.semantic_items()
    }

    pub(super) fn append(&mut self, item: &PythonSequenceItem) {
        self.sequence.append(item);
    }

    fn extend(&mut self, extension: &SequenceFacts, operation_origin: Origin) {
        self.sequence.extend(extension, operation_origin);
    }

    pub(super) fn concatenate(&mut self, extension: &Self, operation_origin: Origin) {
        self.extend(&extension.sequence, operation_origin);
    }

    /// Star-unpack an iterable source into this tuple during construction.
    /// Returns `None` for a non-iterable source. A tuple owns no allocation
    /// site, but nested mutable values remain reachable through it.
    pub(super) fn extend_from_iterable(
        &mut self,
        source: &PythonValue,
        operation_origin: Origin,
    ) -> Option<()> {
        self.sequence.extend_from_iterable(source, operation_origin)
    }

    /// Traverse into the value at `index` for transactional mutation of a
    /// nested mutable container. The tuple's own structure is never mutated;
    /// only a nested list/dict reached through indexing changes.
    pub(super) fn try_mutate_indexed_value(
        &mut self,
        index: usize,
        mutate: impl Fn(&mut PythonValue) -> bool,
    ) -> bool {
        self.sequence.try_mutate_indexed_value(index, mutate)
    }

    pub(super) fn allocation_site_occurrences(&self, wanted: &ReachableAllocationSites) -> usize {
        self.sequence.allocation_site_occurrences(wanted)
    }

    pub(super) fn contains_origin(&self, wanted: Origin) -> bool {
        self.sequence.contains_origin(wanted)
    }

    pub(super) fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        self.sequence.constrain_value_evidence(constraints);
    }

    pub(super) fn normalize(&mut self) {
        self.sequence.normalize(None);
    }

    pub(super) fn same_semantic_value(&self, other: &Self) -> bool {
        self.sequence.same_semantic_value(&other.sequence)
    }

    pub(super) fn merge_semantically_equal(
        &mut self,
        incoming: Self,
        operation_origin: Option<Origin>,
    ) {
        self.sequence
            .merge_semantically_equal(incoming.sequence, operation_origin);
    }
}

/// Ordered summary plus bounded correlated alternatives shared by lists and
/// tuples. Owns sequence access, normalization, semantic joins, and compatible
/// concatenation. It does not expose runtime mutation as public behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceFacts {
    /// Materialized semantic projection used for equality and ordinary value consumers.
    summary: Vec<PythonSequenceItem>,
    /// Bounded correlated exact alternatives plus the possible unmaterialized remainder.
    alternatives: SequenceAlternatives,
}

impl SequenceFacts {
    fn new(summary: Vec<PythonSequenceItem>) -> Self {
        let facts = Self {
            alternatives: SequenceAlternatives::one(summary.clone()),
            summary,
        };
        facts.debug_assert_semantic_equivalence();
        facts
    }

    fn semantic_items(&self) -> &[PythonSequenceItem] {
        &self.summary
    }

    fn is_authoritative(&self) -> bool {
        self.summary
            .iter()
            .all(|item| matches!(item, PythonSequenceItem::Value(_)))
    }

    fn alternatives(&self) -> impl Iterator<Item = PythonSequenceAlternativeRef<'_>> {
        self.alternatives
            .exact
            .iter()
            .map(|alternative| PythonSequenceAlternativeRef::Exact {
                items: &alternative.items,
                constraints: &alternative.constraints,
            })
            .chain(self.alternatives.remainder.iter().map(|remainder| {
                PythonSequenceAlternativeRef::Remainder {
                    origins: remainder.origins.as_slice(),
                    constraints: &remainder.constraints,
                }
            }))
    }

    fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        for item in &mut self.summary {
            item.constrain_value_evidence(constraints);
        }
        self.alternatives.constrain_value_evidence(constraints);
        self.debug_assert_semantic_equivalence();
    }

    fn append(&mut self, item: &PythonSequenceItem) {
        self.summary.push(item.clone());
        self.alternatives.append(item);
        self.debug_assert_semantic_equivalence();
    }

    fn extend(&mut self, extension: &Self, operation_origin: Origin) {
        self.summary.extend(extension.summary.clone());
        self.alternatives
            .extend(&extension.alternatives, operation_origin);
        self.debug_assert_semantic_equivalence();
    }

    /// Extend by consuming an iterable source: list/tuple sources contribute
    /// exact ordered facts, known-but-imprecise (string/mapping) and
    /// indeterminate (unknown/path) sources contribute a typed unknown-unpack,
    /// and a definitely non-iterable source (bool) returns `false` without
    /// touching the facts.
    fn extend_from_iterable(
        &mut self,
        source: &PythonValue,
        operation_origin: Origin,
    ) -> Option<()> {
        match source.iterable_knowledge() {
            PythonIterableKnowledge::Known(PythonIterable::Sequence(PythonSequence::List(
                list,
            ))) => self.extend(&list.sequence, operation_origin),
            PythonIterableKnowledge::Known(PythonIterable::Sequence(PythonSequence::Tuple(
                tuple,
            ))) => self.extend(&tuple.sequence, operation_origin),
            PythonIterableKnowledge::Known(PythonIterable::Sequence(PythonSequence::String(_))) => {
                self.append(&PythonSequenceItem::UnknownUnpack(
                    source.imprecise_iteration_unknown(),
                ));
            }
            PythonIterableKnowledge::Known(PythonIterable::MappingKeys(mapping)) => self.append(
                &PythonSequenceItem::UnknownUnpack(mapping.keys_iteration_unknown()),
            ),
            PythonIterableKnowledge::Indeterminate(unknown) => {
                self.append(&PythonSequenceItem::UnknownUnpack(unknown));
            }
            PythonIterableKnowledge::NotIterable => return None,
        }
        Some(())
    }

    fn insert(&mut self, index: usize, item: &PythonSequenceItem) {
        self.summary.insert(index, item.clone());
        self.alternatives.insert(index, item);
        self.debug_assert_semantic_equivalence();
    }

    fn remove(&mut self, index: usize) {
        self.summary.remove(index);
        self.alternatives.remove(index);
        self.debug_assert_semantic_equivalence();
    }

    fn try_mutate_indexed_value(
        &mut self,
        index: usize,
        mutate: impl Fn(&mut PythonValue) -> bool,
    ) -> bool {
        let mut summary = self.summary.clone();
        let Some(PythonSequenceItem::Value(next)) = summary.get_mut(index) else {
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

    fn allocation_site_occurrences(&self, wanted: &ReachableAllocationSites) -> usize {
        self.summary
            .iter()
            .filter_map(|item| match item {
                PythonSequenceItem::Value(value) => Some(value.allocation_site_occurrences(wanted)),
                PythonSequenceItem::UnknownElement(_) | PythonSequenceItem::UnknownUnpack(_) => {
                    None
                }
            })
            .sum()
    }

    fn contains_origin(&self, wanted: Origin) -> bool {
        self.summary.iter().any(|item| match item {
            PythonSequenceItem::Value(value) => value.contains_origin(wanted),
            PythonSequenceItem::UnknownElement(unknown)
            | PythonSequenceItem::UnknownUnpack(unknown) => unknown.contains_origin(wanted),
        })
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        for item in &mut self.summary {
            item.normalize();
        }
        self.alternatives.normalize(operation_origin);
        self.debug_assert_semantic_equivalence();
    }

    fn debug_assert_semantic_equivalence(&self) {
        debug_assert!(self.alternatives.exact.iter().all(|alternative| {
            PythonSequenceItem::slices_same_semantic_value(&self.summary, &alternative.items)
        }));
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        PythonSequenceItem::slices_same_semantic_value(&self.summary, &other.summary)
    }

    fn merge_semantically_equal(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        debug_assert!(self.same_semantic_value(&incoming));
        for (existing, incoming) in self.summary.iter_mut().zip(incoming.summary) {
            existing.merge_semantically_equal(incoming, operation_origin);
        }
        self.alternatives
            .merge(incoming.alternatives, operation_origin);
        self.debug_assert_semantic_equivalence();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConstrainedExactSequence {
    items: Vec<PythonSequenceItem>,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceAlternativeRemainder {
    origins: CanonicalOrigins,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceAlternatives {
    exact: Vec<ConstrainedExactSequence>,
    remainder: Option<SequenceAlternativeRemainder>,
}

impl SequenceAlternatives {
    fn one(items: Vec<PythonSequenceItem>) -> Self {
        Self {
            exact: vec![ConstrainedExactSequence {
                items,
                constraints: BranchConstraints::unconstrained(),
            }],
            remainder: None,
        }
    }

    fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        for alternative in &mut self.exact {
            alternative.constraints = alternative.constraints.intersection(constraints);
            for item in &mut alternative.items {
                item.constrain_value_evidence(constraints);
            }
        }
        if let Some(remainder) = &mut self.remainder {
            remainder.constraints = remainder.constraints.intersection(constraints);
        }
        self.normalize(None);
    }

    fn append(&mut self, item: &PythonSequenceItem) {
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
                exact.push(ConstrainedExactSequence { items, constraints });
            }
        }

        let mut remainder_constraints = None;
        let mut remainder_origins = CanonicalOrigins::default();
        if let Some(right_remainder) = &extension.remainder {
            for left in &self.exact {
                let constraints = left.constraints.intersection(&right_remainder.constraints);
                if constraints.is_impossible() {
                    continue;
                }
                merge_feasible_constraints(&mut remainder_constraints, constraints);
                remainder_origins.extend(right_remainder.origins.iter());
                for item in &left.items {
                    item.extend_origins(&mut remainder_origins);
                }
            }
        }
        if let Some(left_remainder) = &self.remainder {
            for right in &extension.exact {
                let constraints = left_remainder.constraints.intersection(&right.constraints);
                if constraints.is_impossible() {
                    continue;
                }
                merge_feasible_constraints(&mut remainder_constraints, constraints);
                remainder_origins.extend(left_remainder.origins.iter());
                for item in &right.items {
                    item.extend_origins(&mut remainder_origins);
                }
            }
        }
        if let (Some(left_remainder), Some(right_remainder)) =
            (&self.remainder, &extension.remainder)
        {
            let constraints = left_remainder
                .constraints
                .intersection(&right_remainder.constraints);
            if !constraints.is_impossible() {
                merge_feasible_constraints(&mut remainder_constraints, constraints);
                remainder_origins.extend(left_remainder.origins.iter());
                remainder_origins.extend(right_remainder.origins.iter());
            }
        }

        *self = Self {
            exact,
            remainder: remainder_constraints.map(|constraints| {
                remainder_origins.insert(operation_origin);
                SequenceAlternativeRemainder {
                    origins: remainder_origins,
                    constraints,
                }
            }),
        };
        self.normalize(Some(operation_origin));
    }

    fn insert(&mut self, index: usize, item: &PythonSequenceItem) {
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
            let Some(PythonSequenceItem::Value(next)) = alternative.items.get_mut(index) else {
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
            (Some(mut existing), Some(incoming)) => {
                existing.constraints.merge(incoming.constraints);
                existing.origins.extend(incoming.origins.iter());
                Some(existing)
            }
            (Some(remainder), None) | (None, Some(remainder)) => Some(remainder),
            (None, None) => None,
        };
        self.normalize(operation_origin);
    }

    fn normalize(&mut self, operation_origin: Option<Origin>) {
        for alternative in &mut self.exact {
            for item in &mut alternative.items {
                item.normalize();
            }
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
            let (mut constraints, mut origins) = self.remainder.take().map_or_else(
                || (None, CanonicalOrigins::default()),
                |remainder| (Some(remainder.constraints), remainder.origins),
            );
            for alternative in omitted {
                merge_feasible_constraints(&mut constraints, alternative.constraints);
                for item in &alternative.items {
                    item.extend_origins(&mut origins);
                }
            }
            origins.insert(
                operation_origin
                    .expect("truncating sequence alternatives requires an operation origin"),
            );
            self.remainder = constraints.map(|constraints| SequenceAlternativeRemainder {
                origins,
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

/// Borrowed view over the sequence protocol shared by concrete lists, tuples,
/// and strings. It is a data-bearing projection over the closed value model,
/// not a stored capability tag: settings and other consumers ask a value for
/// this view instead of matching nominal kinds directly.
#[derive(Clone, Copy)]
pub(crate) enum PythonSequence<'a> {
    List(&'a PythonList),
    Tuple(&'a PythonTuple),
    #[expect(
        dead_code,
        reason = "string data is carried honestly while precise character facts remain deferred"
    )]
    String(&'a str),
}

impl<'a> PythonSequence<'a> {
    /// Project sequence members whose exact element facts are materialized.
    /// String membership is honest, but character facts remain deliberately
    /// imprecise rather than masquerading as an empty sequence.
    pub(crate) fn materialized(self) -> Option<PythonMaterializedSequence<'a>> {
        match self {
            Self::List(list) => Some(PythonMaterializedSequence::List(list)),
            Self::Tuple(tuple) => Some(PythonMaterializedSequence::Tuple(tuple)),
            Self::String(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PythonMaterializedSequence<'a> {
    List(&'a PythonList),
    Tuple(&'a PythonTuple),
}

impl<'a> PythonMaterializedSequence<'a> {
    fn facts(self) -> &'a SequenceFacts {
        match self {
            Self::List(list) => &list.sequence,
            Self::Tuple(tuple) => &tuple.sequence,
        }
    }

    pub(crate) fn semantic_items(self) -> &'a [PythonSequenceItem] {
        self.facts().semantic_items()
    }

    pub(crate) fn alternatives(self) -> impl Iterator<Item = PythonSequenceAlternativeRef<'a>> {
        self.facts().alternatives()
    }
}

pub(crate) enum PythonSequenceAlternativeRef<'a> {
    Exact {
        items: &'a [PythonSequenceItem],
        constraints: &'a BranchConstraints,
    },
    Remainder {
        origins: &'a [Origin],
        constraints: &'a BranchConstraints,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonSequenceItem {
    Value(PythonValue),
    UnknownElement(PythonUnknown),
    UnknownUnpack(PythonUnknown),
}

impl PythonSequenceItem {
    fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        if let Self::Value(value) = self {
            value.constrain_value_evidence(constraints);
        }
    }

    fn normalize(&mut self) {
        if let Self::Value(value) = self {
            value.normalize();
        }
    }

    fn extend_origins(&self, origins: &mut CanonicalOrigins) {
        match self {
            Self::Value(value) => origins.extend(value.origins()),
            Self::UnknownElement(unknown) | Self::UnknownUnpack(unknown) => {
                origins.extend(unknown.origins());
            }
        }
    }

    fn slices_same_semantic_value(left: &[Self], right: &[Self]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| left.same_semantic_value(right))
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Value(left), Self::Value(right)) => left.same_semantic_value(right),
            (Self::UnknownElement(left), Self::UnknownElement(right))
            | (Self::UnknownUnpack(left), Self::UnknownUnpack(right)) => left.cause == right.cause,
            (Self::Value(_) | Self::UnknownElement(_) | Self::UnknownUnpack(_), _) => false,
        }
    }

    fn merge_semantically_equal(&mut self, incoming: Self, operation_origin: Option<Origin>) {
        debug_assert!(self.same_semantic_value(&incoming));
        match (self, incoming) {
            (Self::Value(existing), Self::Value(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin);
            }
            (Self::UnknownElement(existing), Self::UnknownElement(incoming))
            | (Self::UnknownUnpack(existing), Self::UnknownUnpack(incoming)) => {
                existing.merge_origins(&incoming);
            }
            _ => unreachable!("semantic equality requires matching sequence item variants"),
        }
    }
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::super::PythonUnknownCause;
    use super::BranchConstraints;
    use super::CanonicalOrigins;
    use super::ConstrainedExactSequence;
    use super::MAX_EXACT_PYTHON_ALTERNATIVES;
    use super::Origin;
    use super::PythonSequenceItem;
    use super::PythonUnknown;
    use super::PythonValue;
    use super::PythonValueKind;
    use super::SequenceAlternativeRemainder;
    use super::SequenceAlternatives;
    use super::SequenceFacts;

    fn origin(offset: usize) -> Origin {
        let file = File::from_id(Id::from_bits(1));
        Origin::new(file, Span::saturating_from_parts_usize(offset, 1))
    }

    fn str_item(text: &str, offset: usize) -> PythonSequenceItem {
        PythonSequenceItem::Value(PythonValue::string(text.to_string(), origin(offset)))
    }

    /// A single-position correlated sequence whose summary is one `"same"`
    /// string and whose exact alternatives differ only by element origin. This
    /// keeps every alternative semantically equal while remaining distinct for
    /// deduplication, exactly as the original list coverage relied on.
    fn correlated_strings(starts: impl IntoIterator<Item = usize>) -> SequenceFacts {
        SequenceFacts {
            summary: vec![str_item("same", 0)],
            alternatives: SequenceAlternatives {
                exact: starts
                    .into_iter()
                    .map(|start| ConstrainedExactSequence {
                        items: vec![str_item("same", start)],
                        constraints: BranchConstraints::unconstrained(),
                    })
                    .collect(),
                remainder: None,
            },
        }
    }

    fn nested_item(offset: usize) -> PythonSequenceItem {
        PythonSequenceItem::Value(PythonValue::list(Vec::new(), origin(offset)))
    }

    /// A single-position correlated sequence whose element is an empty nested
    /// list, used to prove indexed mutation reaches nested mutable containers
    /// across every correlated projection.
    fn correlated_nested(starts: impl IntoIterator<Item = usize>) -> SequenceFacts {
        SequenceFacts {
            summary: vec![nested_item(0)],
            alternatives: SequenceAlternatives {
                exact: starts
                    .into_iter()
                    .map(|start| ConstrainedExactSequence {
                        items: vec![nested_item(start)],
                        constraints: BranchConstraints::unconstrained(),
                    })
                    .collect(),
                remainder: None,
            },
        }
    }

    fn pair(first: usize, second: usize) -> SequenceFacts {
        let items = vec![str_item("first", first), str_item("second", second)];
        SequenceFacts {
            summary: items.clone(),
            alternatives: SequenceAlternatives {
                exact: vec![ConstrainedExactSequence {
                    items,
                    constraints: BranchConstraints::unconstrained(),
                }],
                remainder: None,
            },
        }
    }

    fn merge_all(facts: Vec<SequenceFacts>, operation_offset: usize) -> SequenceFacts {
        let mut facts = facts.into_iter();
        let mut accumulated = facts
            .next()
            .expect("a merge needs at least one alternative");
        for incoming in facts {
            accumulated.merge_semantically_equal(incoming, Some(origin(operation_offset)));
        }
        accumulated
    }

    fn value_origin_start(item: &PythonSequenceItem) -> u32 {
        let PythonSequenceItem::Value(value) = item else {
            panic!("expected an exact value item");
        };
        value
            .origins()
            .next()
            .expect("a test value has an origin")
            .span
            .start()
    }

    fn append_added(nested: &mut PythonValue, mutation_origin: Origin) -> bool {
        let PythonValueKind::List(list) = &mut nested.kind else {
            return false;
        };
        list.append_value(PythonValue::string("added".to_string(), origin(200)));
        nested.record_origin(mutation_origin);
        true
    }

    fn assert_nested_added(items: &[PythonSequenceItem], mutation_origin: Origin) {
        let PythonSequenceItem::Value(nested) = &items[0] else {
            panic!("the mutated projection should keep an exact nested value");
        };
        assert!(
            nested.origins().any(|origin| origin == mutation_origin),
            "the mutation origin should be recorded on the nested value",
        );
        let PythonValueKind::List(list) = &nested.kind else {
            panic!("the nested value should remain a list");
        };
        assert!(matches!(
            list.semantic_items(),
            [PythonSequenceItem::Value(PythonValue {
                kind: PythonValueKind::Str(text),
                ..
            })] if text == "added"
        ));
    }

    #[test]
    fn correlated_sequence_merge_obeys_laws_and_retains_alternatives() {
        let forward = merge_all(vec![pair(10, 21), pair(20, 11), pair(30, 31)], 1_000);
        let reversed = merge_all(vec![pair(30, 31), pair(20, 11), pair(10, 21)], 1_000);
        assert_eq!(forward, reversed, "merge must be order-independent");

        let duplicated = merge_all(
            vec![
                pair(10, 21),
                pair(20, 11),
                pair(30, 31),
                pair(10, 21),
                pair(20, 11),
                pair(30, 31),
            ],
            1_000,
        );
        assert_eq!(forward, duplicated, "merge must be idempotent");

        assert_eq!(
            forward.alternatives.exact.len(),
            3,
            "the three correlated orderings are retained",
        );
        let mut orderings = forward
            .alternatives
            .exact
            .iter()
            .map(|alternative| {
                (
                    value_origin_start(&alternative.items[0]),
                    value_origin_start(&alternative.items[1]),
                )
            })
            .collect::<Vec<_>>();
        orderings.sort_unstable();
        assert_eq!(orderings, vec![(10, 21), (20, 11), (30, 31)]);

        let position_starts = |index: usize| {
            let mut starts = forward
                .semantic_items()
                .get(index)
                .map(|item| match item {
                    PythonSequenceItem::Value(value) => {
                        value.origins().map(|origin| origin.span.start()).collect()
                    }
                    PythonSequenceItem::UnknownElement(_)
                    | PythonSequenceItem::UnknownUnpack(_) => Vec::new(),
                })
                .unwrap_or_default();
            starts.sort_unstable();
            starts
        };
        assert_eq!(position_starts(0), vec![10, 20, 30]);
        assert_eq!(position_starts(1), vec![11, 21, 31]);
    }

    #[test]
    fn correlated_indexed_mutation_updates_every_projection_and_retains_recursive_origins() {
        let mut facts = correlated_nested([10, 20, 30]);
        let mutation_origin = origin(300);

        assert!(facts.try_mutate_indexed_value(0, |nested| append_added(nested, mutation_origin)));

        assert_nested_added(facts.semantic_items(), mutation_origin);
        for alternative in &facts.alternatives.exact {
            assert_nested_added(&alternative.items, mutation_origin);
        }
    }

    #[test]
    fn failing_correlated_indexed_mutation_leaves_every_projection_unchanged() {
        let mut facts = correlated_nested([10, 20, 30]);
        let before = facts.clone();
        let mutation_origin = origin(300);

        assert!(!facts.try_mutate_indexed_value(0, |nested| {
            if nested.origins().any(|origin| origin.span.start() == 20) {
                return false;
            }
            append_added(nested, mutation_origin)
        }));
        assert_eq!(
            facts, before,
            "a rejected projection reverts the whole value"
        );
    }

    #[test]
    fn correlated_indexed_mutation_preserves_the_capped_remainder_state() {
        let mut facts = correlated_nested(0..32);
        facts.merge_semantically_equal(correlated_nested(32..65), Some(origin(1_000)));
        assert_eq!(facts.alternatives.exact.len(), 64);
        let remainder = facts
            .alternatives
            .remainder
            .as_mut()
            .expect("overflow should retain one typed remainder");
        remainder.constraints.select(origin(1_500), 1);
        let remainder = remainder.clone();
        let mutation_origin = origin(4_000);

        assert!(facts.try_mutate_indexed_value(0, |nested| append_added(nested, mutation_origin)));

        assert_eq!(facts.alternatives.exact.len(), 64);
        for alternative in &facts.alternatives.exact {
            assert_nested_added(&alternative.items, mutation_origin);
        }
        assert_eq!(facts.alternatives.remainder.as_ref(), Some(&remainder));
    }

    #[test]
    fn sequence_extension_has_exact_boundary_and_typed_remainder() {
        let mut at_limit = correlated_strings(0..8);
        at_limit.extend(&correlated_strings(100..108), origin(1_000));
        assert_eq!(at_limit.alternatives.exact.len(), 64);
        assert!(at_limit.alternatives.remainder.is_none());

        let mut overflowed = correlated_strings(0..8);
        overflowed.extend(&correlated_strings(100..109), origin(2_000));
        assert_eq!(overflowed.alternatives.exact.len(), 64);
        let origins = &overflowed
            .alternatives
            .remainder
            .as_ref()
            .expect("overflow should retain a remainder")
            .origins;
        assert!(origins.contains(origin(2_000)));
        assert!(
            origins.iter().len() > 1,
            "omitted path evidence should survive"
        );
    }

    #[test]
    fn capped_sequence_merge_is_idempotent_and_preserves_remainder_constraints() {
        let mut facts = correlated_strings(0..32);
        facts.merge_semantically_equal(correlated_strings(32..65), Some(origin(1_000)));
        facts
            .alternatives
            .remainder
            .as_mut()
            .expect("overflow should retain one typed remainder")
            .constraints
            .select(origin(1_500), 1);
        let before = facts.clone();

        facts.merge_semantically_equal(before.clone(), Some(origin(2_000)));

        assert_eq!(facts, before);
    }

    #[test]
    fn sequence_extension_retains_only_feasible_remainder_products() {
        let join = origin(1_000);
        let mut left = correlated_strings([0]);
        left.alternatives.exact[0].constraints.select(join, 0);
        let mut remainder_constraints = BranchConstraints::unconstrained();
        remainder_constraints.select(join, 1);
        left.alternatives.remainder = Some(SequenceAlternativeRemainder {
            origins: [origin(1_500)].into_iter().collect(),
            constraints: remainder_constraints.clone(),
        });

        let mut exact_right = correlated_strings([100]);
        exact_right.alternatives.exact[0]
            .constraints
            .select(join, 0);
        let mut exact_product = left.clone();
        exact_product.extend(&exact_right, origin(2_000));
        assert_eq!(exact_product.alternatives.exact.len(), 1);
        assert!(exact_product.alternatives.remainder.is_none());

        let mut remainder_right = correlated_strings([200]);
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
    fn sequence_alternative_merge_is_capped_at_the_exact_boundary() {
        let mut at_limit = correlated_strings(0..32);
        at_limit.merge_semantically_equal(correlated_strings(32..64), Some(origin(1_000)));
        assert_eq!(at_limit.alternatives.exact.len(), 64);
        assert!(at_limit.alternatives.remainder.is_none());

        let mut overflowed = correlated_strings(0..32);
        overflowed.merge_semantically_equal(correlated_strings(32..65), Some(origin(2_000)));
        assert_eq!(overflowed.alternatives.exact.len(), 64);
        let origins = &overflowed
            .alternatives
            .remainder
            .as_ref()
            .expect("overflow should retain a remainder")
            .origins;
        assert!(origins.contains(origin(2_000)));
        assert!(
            origins.iter().len() > 1,
            "omitted path evidence should survive"
        );
    }

    #[test]
    fn canonical_unknown_origins_keep_correlated_exact_rows_local() {
        let join = origin(1_000);
        let facts = |offset, arm| {
            let summary_item = PythonSequenceItem::Value(PythonValue::unknown(
                PythonUnknownCause::UnsupportedExpression,
                [origin(offset)],
            ));
            let mut constraints = BranchConstraints::unconstrained();
            constraints.select(join, arm);
            let mut exact_item = summary_item.clone();
            exact_item.constrain_value_evidence(&constraints);
            SequenceFacts {
                summary: vec![summary_item],
                alternatives: SequenceAlternatives {
                    exact: vec![ConstrainedExactSequence {
                        items: vec![exact_item],
                        constraints,
                    }],
                    remainder: None,
                },
            }
        };

        let mut merged = facts(10, 0);
        merged.merge_semantically_equal(facts(20, 1), None);
        merged.normalize(None);

        let [PythonSequenceItem::Value(summary)] = merged.semantic_items() else {
            panic!("the summary should retain one unknown value");
        };
        assert_eq!(
            summary.origins().collect::<Vec<_>>(),
            [origin(10), origin(20)],
            "the aggregate summary should union both origins",
        );
        assert_eq!(
            summary
                .unknown_value()
                .expect("the summary value should remain unknown")
                .origins()
                .collect::<Vec<_>>(),
            [origin(10), origin(20)],
            "summary value evidence and unknown evidence should stay aligned",
        );

        assert_eq!(merged.alternatives.exact.len(), 2);
        assert!(merged.alternatives.remainder.is_none());
        let mut rows = merged
            .alternatives
            .exact
            .iter()
            .map(|alternative| {
                let [PythonSequenceItem::Value(value)] = alternative.items.as_slice() else {
                    panic!("each exact row should retain one unknown value");
                };
                let mut evidence = value.origins_with_constraints();
                let (row_origin, evidence_constraints) = evidence
                    .next()
                    .expect("each exact row should keep one origin");
                assert!(evidence.next().is_none());
                assert_eq!(evidence_constraints, &alternative.constraints);
                assert_eq!(
                    value
                        .unknown_value()
                        .expect("the exact value should remain unknown")
                        .origins()
                        .collect::<Vec<_>>(),
                    [row_origin],
                    "aggregate summary evidence must not leak into an exact row",
                );
                (row_origin.span.start(), alternative.constraints.clone())
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|(start, _)| *start);

        let selected = |arm| {
            let mut constraints = BranchConstraints::unconstrained();
            constraints.select(join, arm);
            constraints
        };
        assert_eq!(rows, [(10, selected(0)), (20, selected(1))]);
    }

    #[test]
    fn canonical_unknown_origins_merge_sequence_items_and_remainders() {
        let unknown = |offset| {
            PythonUnknown::new(PythonUnknownCause::UnsupportedExpression, [origin(offset)])
        };
        for mut existing in [
            PythonSequenceItem::UnknownElement(unknown(20)),
            PythonSequenceItem::UnknownUnpack(unknown(20)),
        ] {
            let incoming = match &existing {
                PythonSequenceItem::UnknownElement(_) => {
                    PythonSequenceItem::UnknownElement(unknown(10))
                }
                PythonSequenceItem::UnknownUnpack(_) => {
                    PythonSequenceItem::UnknownUnpack(unknown(10))
                }
                PythonSequenceItem::Value(_) => unreachable!(),
            };
            existing.merge_semantically_equal(incoming, None);
            let unknown = match existing {
                PythonSequenceItem::UnknownElement(unknown)
                | PythonSequenceItem::UnknownUnpack(unknown) => unknown,
                PythonSequenceItem::Value(_) => unreachable!(),
            };
            assert_eq!(
                unknown.origins().collect::<Vec<_>>(),
                [origin(10), origin(20)]
            );
        }

        let remainder = |offset| SequenceAlternativeRemainder {
            origins: [origin(offset)].into_iter().collect::<CanonicalOrigins>(),
            constraints: BranchConstraints::unconstrained(),
        };
        let mut alternatives = SequenceAlternatives {
            exact: Vec::new(),
            remainder: Some(remainder(20)),
        };
        alternatives.merge(
            SequenceAlternatives {
                exact: Vec::new(),
                remainder: Some(remainder(10)),
            },
            None,
        );
        assert_eq!(
            alternatives
                .remainder
                .expect("remainders should merge")
                .origins
                .iter()
                .collect::<Vec<_>>(),
            [origin(10), origin(20)],
        );
    }

    #[test]
    fn repeated_sequence_self_extension_stays_bounded_and_uses_each_operation_origin() {
        let mut facts = correlated_strings(0..2);
        for operation_offset in [100, 200, 300, 400] {
            let extension = facts.clone();
            facts.extend(&extension, origin(operation_offset));
            assert!(facts.alternatives.exact.len() <= MAX_EXACT_PYTHON_ALTERNATIVES);
        }

        assert_eq!(facts.alternatives.exact.len(), 64);
        let origins = &facts
            .alternatives
            .remainder
            .as_ref()
            .expect("repeated extension should retain a remainder")
            .origins;
        for operation_offset in [300, 400] {
            assert!(
                origins.contains(origin(operation_offset)),
                "remainder operation {operation_offset} should survive"
            );
        }
    }
}
