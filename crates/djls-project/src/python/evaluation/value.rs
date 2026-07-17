use camino::Utf8PathBuf;
use djls_source::FileReadError;
use djls_source::Origin;

use super::BranchConstraints;
use super::CanonicalOrigins;
use super::PythonDict;
use super::PythonList;
use super::PythonMapping;
use super::PythonSequence;
use super::PythonSequenceItem;
use super::PythonTuple;
use super::ReachableAllocationSites;
use super::allocation::AllocationSites;
use super::origin_sort_key;
use crate::python::PythonModuleName;
use crate::python::PythonSyntaxError;
use crate::python::module::PythonImportError;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonValueEvidence {
    origin: Origin,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PythonValueEvidenceSet(Vec<PythonValueEvidence>);

impl PythonValueEvidenceSet {
    fn one(origin: Origin) -> Self {
        Self(vec![PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
        }])
    }

    fn from_origins(origins: impl IntoIterator<Item = Origin>) -> Self {
        let mut evidence = Self::default();
        evidence.extend(origins.into_iter().map(|origin| PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
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

    fn origins_with_constraints(&self) -> impl Iterator<Item = (Origin, &BranchConstraints)> {
        self.0
            .iter()
            .map(|evidence| (evidence.origin, &evidence.constraints))
    }

    fn rebase(&mut self, origin: Origin) {
        self.0.clear();
        self.0.push(PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
        });
    }

    fn record(&mut self, origin: Origin) {
        self.insert(PythonValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
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
            } else {
                normalized.push(evidence);
            }
        }
        self.0 = normalized;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonValue {
    pub(in crate::python::evaluation) kind: PythonValueKind,
    evidence: PythonValueEvidenceSet,
}

impl PythonValue {
    pub(super) fn unknown(
        cause: PythonUnknownCause,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Self {
        let unknown = PythonUnknown::new(cause, origins);
        let evidence = PythonValueEvidenceSet::from_origins(unknown.origins());
        Self {
            kind: PythonValueKind::Unknown(unknown),
            evidence,
        }
    }

    pub(super) fn string(value: String, origin: Origin) -> Self {
        Self::scalar(PythonValueKind::Str(value), origin)
    }

    pub(super) fn bool(value: bool, origin: Origin) -> Self {
        Self::scalar(PythonValueKind::Bool(value), origin)
    }

    pub(super) fn path(value: Utf8PathBuf, origin: Origin) -> Self {
        Self::scalar(PythonValueKind::Path(value), origin)
    }

    pub(super) fn list(items: Vec<PythonSequenceItem>, origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::List(PythonList::new(items, origin)),
            evidence: PythonValueEvidenceSet::one(origin),
        }
    }

    pub(super) fn tuple(items: Vec<PythonSequenceItem>, origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::Tuple(PythonTuple::new(items)),
            evidence: PythonValueEvidenceSet::one(origin),
        }
    }

    pub(super) fn empty_dict(origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::Dict(PythonDict::empty(origin)),
            evidence: PythonValueEvidenceSet::one(origin),
        }
    }

    pub(super) fn dict_entry(key: PythonValue, value: PythonValue, origin: Origin) -> Self {
        let mut dict = PythonDict::empty(origin);
        dict.append_entry(key, value);
        Self {
            kind: PythonValueKind::Dict(dict),
            evidence: PythonValueEvidenceSet::one(origin),
        }
    }

    fn scalar(kind: PythonValueKind, origin: Origin) -> Self {
        debug_assert!(matches!(
            &kind,
            PythonValueKind::Str(_) | PythonValueKind::Bool(_) | PythonValueKind::Path(_)
        ));
        Self {
            kind,
            evidence: PythonValueEvidenceSet::one(origin),
        }
    }

    #[cfg(test)]
    fn known(kind: PythonValueKind, origin: Origin) -> Self {
        Self {
            kind,
            evidence: PythonValueEvidenceSet::one(origin),
        }
    }

    pub(crate) fn origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.evidence.origins()
    }

    pub(crate) fn string_value(&self) -> Option<&str> {
        let PythonValueKind::Str(value) = &self.kind else {
            return None;
        };
        Some(value)
    }

    pub(crate) fn bool_value(&self) -> Option<bool> {
        let PythonValueKind::Bool(value) = &self.kind else {
            return None;
        };
        Some(*value)
    }

    pub(crate) fn path_value(&self) -> Option<&Utf8PathBuf> {
        let PythonValueKind::Path(value) = &self.kind else {
            return None;
        };
        Some(value)
    }

    pub(crate) fn mapping(&self) -> Option<PythonMapping<'_>> {
        let PythonValueKind::Dict(value) = &self.kind else {
            return None;
        };
        Some(value.mapping())
    }

    pub(crate) fn unknown_value(&self) -> Option<&PythonUnknown> {
        let PythonValueKind::Unknown(value) = &self.kind else {
            return None;
        };
        Some(value)
    }

    /// Intentional owned structural projection for the stable test adapter.
    pub(crate) fn into_kind(self) -> PythonValueKind {
        self.kind
    }

    /// The allocation sites this value owns directly, if it is a concrete
    /// mutable container. Tuples and scalars own none.
    pub(super) fn own_mutable_sites(&self) -> Option<&AllocationSites> {
        match &self.kind {
            PythonValueKind::List(list) => Some(list.allocation_sites()),
            PythonValueKind::Dict(dict) => Some(dict.allocation_sites()),
            PythonValueKind::Tuple(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Unknown(_) => None,
        }
    }

    /// This value's honest sequence projection: lists, tuples, and strings are
    /// all Python sequences. Consumers that reject strings (such as
    /// collection-shaped settings) match [`PythonSequence::String`] explicitly
    /// rather than relying on this method to hide it.
    pub(crate) fn sequence(&self) -> Option<PythonSequence<'_>> {
        match &self.kind {
            PythonValueKind::List(list) => Some(PythonSequence::List(list)),
            PythonValueKind::Tuple(tuple) => Some(PythonSequence::Tuple(tuple)),
            PythonValueKind::Str(text) => Some(PythonSequence::String(text)),
            PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Unknown(_) => None,
        }
    }

    /// Classify this value's iterability. Lists, tuples, and strings are
    /// sequences; dictionaries are iterable over their keys; booleans are
    /// definitely not iterable; unknown and path values are indeterminate
    /// because their runtime iterability cannot be decided here.
    pub(super) fn iterable_knowledge(&self) -> PythonIterableKnowledge<'_> {
        match &self.kind {
            PythonValueKind::List(list) => {
                PythonIterableKnowledge::Known(PythonIterable::Sequence(PythonSequence::List(list)))
            }
            PythonValueKind::Tuple(tuple) => PythonIterableKnowledge::Known(
                PythonIterable::Sequence(PythonSequence::Tuple(tuple)),
            ),
            PythonValueKind::Str(text) => PythonIterableKnowledge::Known(PythonIterable::Sequence(
                PythonSequence::String(text),
            )),
            PythonValueKind::Dict(dict) => {
                PythonIterableKnowledge::Known(PythonIterable::MappingKeys(dict.mapping()))
            }
            PythonValueKind::Bool(_) => PythonIterableKnowledge::NotIterable,
            // A path fact erases whether the runtime source was a string or a
            // `pathlib.Path`, so its iterability is indeterminate rather than a
            // synthesized nominal kind.
            PythonValueKind::Path(_) => {
                PythonIterableKnowledge::Indeterminate(self.imprecise_iteration_unknown())
            }
            PythonValueKind::Unknown(unknown) => {
                PythonIterableKnowledge::Indeterminate(unknown.clone())
            }
        }
    }

    /// A typed unknown-unpack that retains this value's source provenance, used
    /// when iterating a known-but-imprecise or indeterminate source contributes
    /// elements that cannot be materialized.
    pub(super) fn imprecise_iteration_unknown(&self) -> PythonUnknown {
        PythonUnknown::new(PythonUnknownCause::UnsupportedExpression, self.origins())
    }

    pub(super) fn reachable_allocation_sites(&self) -> ReachableAllocationSites {
        let mut sites = ReachableAllocationSites::default();
        self.collect_reachable_sites(&mut sites);
        sites
    }

    /// Push this value's own constrained sites, then recurse into any nested
    /// mutable containers, preserving one group per reachable object.
    pub(super) fn collect_reachable_sites(&self, sites: &mut ReachableAllocationSites) {
        if let Some(own) = self.own_mutable_sites() {
            sites.push_group(own.clone());
        }
        match &self.kind {
            PythonValueKind::List(list) => {
                for item in list.semantic_items() {
                    if let PythonSequenceItem::Value(value) = item {
                        value.collect_reachable_sites(sites);
                    }
                }
            }
            PythonValueKind::Tuple(tuple) => {
                for item in tuple.semantic_items() {
                    if let PythonSequenceItem::Value(value) = item {
                        value.collect_reachable_sites(sites);
                    }
                }
            }
            PythonValueKind::Dict(dict) => dict.mapping().collect_reachable_sites(sites),
            PythonValueKind::Unknown(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_) => {}
        }
    }

    pub(super) fn allocation_site_occurrences(&self, wanted: &ReachableAllocationSites) -> usize {
        let own = usize::from(
            self.own_mutable_sites()
                .is_some_and(|sites| wanted.intersects_group(sites)),
        );
        own + match &self.kind {
            PythonValueKind::List(list) => list.allocation_site_occurrences(wanted),
            PythonValueKind::Tuple(tuple) => tuple.allocation_site_occurrences(wanted),
            PythonValueKind::Dict(dict) => dict.mapping().allocation_site_occurrences(wanted),
            PythonValueKind::Unknown(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_) => 0,
        }
    }

    /// Whether `wanted` appears as provenance anywhere in this value's
    /// structure, recursing into nested sequences and dictionaries. Used by
    /// settings to attribute a mutation origin to a value without reaching into
    /// the private mapping log.
    pub(crate) fn contains_origin(&self, wanted: Origin) -> bool {
        if self.origins().any(|origin| origin == wanted) {
            return true;
        }
        match &self.kind {
            PythonValueKind::List(list) => list.contains_origin(wanted),
            PythonValueKind::Tuple(tuple) => tuple.contains_origin(wanted),
            PythonValueKind::Dict(dict) => dict.mapping().contains_origin(wanted),
            PythonValueKind::Unknown(unknown) => unknown.contains_origin(wanted),
            PythonValueKind::Str(_) | PythonValueKind::Bool(_) | PythonValueKind::Path(_) => false,
        }
    }

    /// Nominal binary `+`. Concatenation is decided by the concrete operand
    /// kinds, never by iterable extension: only same-kind list/tuple/string
    /// operands (and a known list/tuple with an unknown right operand) produce
    /// a nominal result. Every other pair is an unsupported-expression unknown.
    /// A successful result rebases top-level provenance to the operation, and a
    /// new list additionally allocates a fresh site there.
    pub(super) fn add(mut self, right: &Self, origin: Origin) -> Self {
        let combined = match (&mut self.kind, &right.kind) {
            (PythonValueKind::List(list), PythonValueKind::List(right)) => {
                list.concatenate(right, origin);
                true
            }
            (PythonValueKind::List(list), PythonValueKind::Unknown(unknown)) => {
                list.append(&PythonSequenceItem::UnknownUnpack(unknown.clone()));
                true
            }
            (PythonValueKind::Tuple(tuple), PythonValueKind::Tuple(right)) => {
                tuple.concatenate(right, origin);
                true
            }
            (PythonValueKind::Tuple(tuple), PythonValueKind::Unknown(unknown)) => {
                tuple.append(&PythonSequenceItem::UnknownUnpack(unknown.clone()));
                true
            }
            (PythonValueKind::Str(left), PythonValueKind::Str(right)) => {
                left.push_str(right);
                true
            }
            (
                PythonValueKind::Str(_)
                | PythonValueKind::Bool(_)
                | PythonValueKind::Path(_)
                | PythonValueKind::List(_)
                | PythonValueKind::Tuple(_)
                | PythonValueKind::Dict(_)
                | PythonValueKind::Unknown(_),
                _,
            ) => false,
        };
        if combined {
            self.rebase_origin(origin);
            self
        } else {
            Self::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin))
        }
    }

    /// Append one constructed element to a list or tuple literal under
    /// construction. An unknown element is recorded as a typed unknown element;
    /// every other value becomes an exact sequence element.
    pub(super) fn push_constructed_element(&mut self, value: PythonValue) {
        let item = match value.kind {
            PythonValueKind::Unknown(unknown) => PythonSequenceItem::UnknownElement(unknown),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::List(_)
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_) => PythonSequenceItem::Value(value),
        };
        match &mut self.kind {
            PythonValueKind::List(list) => list.append(&item),
            PythonValueKind::Tuple(tuple) => tuple.append(&item),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Unknown(_) => {
                unreachable!("sequence construction appends into a list or tuple")
            }
        }
    }

    /// Star-unpack an iterable `source` into this list or tuple literal under
    /// construction. A definitely non-iterable source (bool) returns `None`
    /// so the caller can collapse the whole constructed expression to an
    /// unknown.
    pub(super) fn star_extend_construction(
        &mut self,
        source: &PythonValue,
        origin: Origin,
    ) -> Option<()> {
        match &mut self.kind {
            PythonValueKind::List(list) => list.extend_from(source, origin),
            PythonValueKind::Tuple(tuple) => tuple.extend_from_iterable(source, origin),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Unknown(_) => {
                unreachable!("sequence construction extends a list or tuple")
            }
        }
    }

    pub(crate) fn origins_with_constraints(
        &self,
    ) -> impl Iterator<Item = (Origin, &BranchConstraints)> {
        self.evidence.origins_with_constraints()
    }

    pub(super) fn rebase_origin(&mut self, origin: Origin) {
        self.evidence.rebase(origin);
        match &mut self.kind {
            PythonValueKind::List(list) => list.rebase_allocation_site(origin),
            PythonValueKind::Unknown(unknown) => unknown.replace_origins([origin]),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_) => {}
        }
        self.debug_assert_unknown_evidence_aligned();
    }

    pub(super) fn record_origin(&mut self, origin: Origin) {
        self.evidence.record(origin);
        if let PythonValueKind::Unknown(unknown) = &mut self.kind {
            unknown.insert_origin(origin);
        }
        self.debug_assert_unknown_evidence_aligned();
    }

    pub(super) fn normalize(&mut self) {
        self.evidence.normalize();
        self.kind.normalize();
        self.debug_assert_unknown_evidence_aligned();
    }

    fn debug_assert_unknown_evidence_aligned(&self) {
        if let PythonValueKind::Unknown(unknown) = &self.kind {
            debug_assert!(self.evidence.origins().eq(unknown.origins()));
        }
    }

    pub(super) fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        self.evidence.constrain(constraints);
        match &mut self.kind {
            PythonValueKind::List(list) => list.constrain_value_evidence(constraints),
            PythonValueKind::Tuple(tuple) => tuple.constrain_value_evidence(constraints),
            PythonValueKind::Dict(dict) => dict.constrain_value_evidence(constraints),
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
    Tuple(PythonTuple),
    Dict(PythonDict),
    Unknown(PythonUnknown),
}

impl PythonValueKind {
    fn normalize(&mut self) {
        match self {
            Self::List(list) => list.normalize(),
            Self::Tuple(tuple) => tuple.normalize(),
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
            (Self::Tuple(left), Self::Tuple(right)) => left.same_semantic_value(right),
            (Self::Dict(left), Self::Dict(right)) => left.same_semantic_value(right),
            (Self::Unknown(left), Self::Unknown(right)) => left.cause == right.cause,
            (
                Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::List(_)
                | Self::Tuple(_)
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
            (Self::Tuple(existing), Self::Tuple(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin);
            }
            (Self::Dict(existing), Self::Dict(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin);
            }
            (Self::Unknown(existing), Self::Unknown(incoming)) => {
                existing.merge_origins(&incoming);
            }
            (Self::Str(_), Self::Str(_))
            | (Self::Bool(_), Self::Bool(_))
            | (Self::Path(_), Self::Path(_)) => {}
            (
                Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::List(_)
                | Self::Tuple(_)
                | Self::Dict(_)
                | Self::Unknown(_),
                _,
            ) => unreachable!("semantic equality requires matching value variants"),
        }
    }
}

/// The classification of a value's iterability: what an iteration consumer can
/// know about iterating it. It is a data-bearing projection over the closed
/// value model, not a stored capability tag.
pub(super) enum PythonIterableKnowledge<'a> {
    Known(PythonIterable<'a>),
    Indeterminate(PythonUnknown),
    NotIterable,
}

/// A definitely-iterable value: a sequence (list, tuple, or string) or a
/// mapping iterated over its keys.
pub(super) enum PythonIterable<'a> {
    Sequence(PythonSequence<'a>),
    MappingKeys(PythonMapping<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonUnknown {
    pub(crate) cause: PythonUnknownCause,
    origins: CanonicalOrigins,
}

impl PythonUnknown {
    pub(super) fn new(
        cause: PythonUnknownCause,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Self {
        Self {
            cause,
            origins: origins.into_iter().collect(),
        }
    }

    pub(crate) fn origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.origins.iter()
    }

    pub(super) fn contains_origin(&self, wanted: Origin) -> bool {
        self.origins.contains(wanted)
    }

    fn insert_origin(&mut self, origin: Origin) {
        self.origins.insert(origin);
    }

    pub(super) fn merge_origins(&mut self, incoming: &Self) {
        debug_assert_eq!(self.cause, incoming.cause);
        self.origins.extend(incoming.origins.iter());
    }

    pub(super) fn replace_origins(&mut self, origins: impl IntoIterator<Item = Origin>) {
        self.origins.replace(origins);
    }
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

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::BranchConstraints;
    use super::Origin;
    use super::PythonDict;
    use super::PythonIterable;
    use super::PythonIterableKnowledge;
    use super::PythonList;
    use super::PythonSequence;
    use super::PythonSequenceItem;
    use super::PythonTuple;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use super::PythonValueEvidence;
    use super::PythonValueEvidenceSet;
    use super::PythonValueKind;
    use super::ReachableAllocationSites;

    fn origin(offset: usize) -> Origin {
        let file = File::from_id(Id::from_bits(1));
        Origin::new(file, Span::saturating_from_parts_usize(offset, 1))
    }

    fn list_value(site: Origin, items: Vec<PythonSequenceItem>) -> PythonValue {
        PythonValue::known(PythonValueKind::List(PythonList::new(items, site)), site)
    }

    fn dict_value(site: Origin) -> PythonValue {
        PythonValue::known(PythonValueKind::Dict(PythonDict::empty(site)), site)
    }

    fn tuple_value(site: Origin, items: Vec<PythonSequenceItem>) -> PythonValue {
        PythonValue::known(PythonValueKind::Tuple(PythonTuple::new(items)), site)
    }

    fn str_value(site: Origin, text: &str) -> PythonValue {
        PythonValue::known(PythonValueKind::Str(text.to_string()), site)
    }

    fn bool_value(site: Origin, flag: bool) -> PythonValue {
        PythonValue::known(PythonValueKind::Bool(flag), site)
    }

    fn path_value(site: Origin, text: &str) -> PythonValue {
        PythonValue::known(PythonValueKind::Path(Utf8PathBuf::from(text)), site)
    }

    fn unknown_value(site: Origin) -> PythonValue {
        PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(site))
    }

    fn str_item(site: Origin, text: &str) -> PythonSequenceItem {
        PythonSequenceItem::Value(str_value(site, text))
    }

    fn item_texts(value: &PythonValue) -> Vec<String> {
        let items = match &value.kind {
            PythonValueKind::List(list) => list.semantic_items(),
            PythonValueKind::Tuple(tuple) => tuple.semantic_items(),
            _ => panic!("expected a sequence value"),
        };
        items
            .iter()
            .map(|item| match item {
                PythonSequenceItem::Value(PythonValue {
                    kind: PythonValueKind::Str(text),
                    ..
                }) => format!("str:{text}"),
                PythonSequenceItem::Value(_) => "value".to_string(),
                PythonSequenceItem::UnknownElement(_) => "element".to_string(),
                PythonSequenceItem::UnknownUnpack(_) => "unpack".to_string(),
            })
            .collect()
    }

    fn site_origins(value: &PythonValue) -> Vec<Origin> {
        value
            .own_mutable_sites()
            .map(|sites| sites.origins().collect())
            .unwrap_or_default()
    }

    fn wanted(value: &PythonValue) -> ReachableAllocationSites {
        let mut wanted = ReachableAllocationSites::default();
        wanted.push_group(value.own_mutable_sites().expect("mutable value").clone());
        wanted
    }

    fn sorted(mut origins: Vec<Origin>) -> Vec<Origin> {
        origins.sort_by_key(super::origin_sort_key);
        origins
    }

    #[test]
    fn lists_and_dicts_own_sites_but_tuples_and_scalars_do_not() {
        assert_eq!(
            site_origins(&list_value(origin(1), Vec::new())),
            vec![origin(1)]
        );
        assert_eq!(site_origins(&dict_value(origin(2))), vec![origin(2)]);
        assert!(
            tuple_value(origin(3), Vec::new())
                .own_mutable_sites()
                .is_none()
        );
        assert!(str_value(origin(4), "x").own_mutable_sites().is_none());
    }

    #[test]
    fn reachability_recurses_and_counts_repeated_occurrences() {
        let inner = list_value(origin(2), Vec::new());
        let outer = list_value(
            origin(1),
            vec![
                PythonSequenceItem::Value(inner.clone()),
                PythonSequenceItem::Value(inner.clone()),
            ],
        );

        assert_eq!(
            outer.allocation_site_occurrences(&wanted(&inner)),
            2,
            "the inner list is reached through two distinct positions"
        );
        assert_eq!(
            outer.allocation_site_occurrences(&wanted(&outer)),
            1,
            "the outer list is reached exactly once"
        );

        let reachable = outer.reachable_allocation_sites();
        assert!(reachable.intersects(&wanted(&inner)));
        assert!(reachable.intersects(&wanted(&outer)));
    }

    #[test]
    fn tuple_indexing_reaches_nested_mutable_without_owning_sites() {
        let inner = list_value(origin(2), Vec::new());
        let tuple = tuple_value(origin(1), vec![PythonSequenceItem::Value(inner.clone())]);
        assert!(tuple.own_mutable_sites().is_none());
        assert_eq!(tuple.allocation_site_occurrences(&wanted(&inner)), 1);
    }

    #[test]
    fn equal_lists_union_allocation_sites_on_merge() {
        let a = list_value(origin(1), Vec::new());
        let b = list_value(origin(2), Vec::new());
        let mut merged = a;
        merged.merge_semantically_equal(b, None);
        assert_eq!(site_origins(&merged), vec![origin(1), origin(2)]);
    }

    #[test]
    fn record_origin_adds_provenance_without_touching_sites() {
        let mut value = list_value(origin(1), Vec::new());
        value.record_origin(origin(2));
        assert_eq!(
            sorted(value.origins().collect()),
            vec![origin(1), origin(2)],
            "provenance accumulates"
        );
        assert_eq!(
            site_origins(&value),
            vec![origin(1)],
            "allocation identity is untouched by recording provenance"
        );
    }

    #[test]
    fn list_add_rebases_provenance_and_allocation_site_together() {
        let left = list_value(origin(1), Vec::new());
        let right = list_value(origin(2), Vec::new());
        let result = left.add(&right, origin(3));
        assert_eq!(result.origins().collect::<Vec<_>>(), vec![origin(3)]);
        assert_eq!(site_origins(&result), vec![origin(3)]);
    }

    #[test]
    fn in_place_extend_preserves_receiver_allocation_sites() {
        let mut receiver = list_value(origin(1), Vec::new());
        let source = list_value(
            origin(2),
            vec![PythonSequenceItem::Value(str_value(origin(5), "a"))],
        );
        let PythonValueKind::List(list) = &mut receiver.kind else {
            panic!("the receiver should remain a list");
        };
        let extended = list.extend_from(&source, origin(4));
        assert!(extended.is_some());
        assert_eq!(
            site_origins(&receiver),
            vec![origin(1)],
            "in-place extension preserves the receiver's allocation identity"
        );
    }

    #[test]
    fn evidence_with_the_same_origin_coalesces_constraints() {
        let evidence_origin = origin(10);
        let join = origin(20);
        let mut first = BranchConstraints::unconstrained();
        first.select(join, 0);
        let mut second = BranchConstraints::unconstrained();
        second.select(join, 1);

        let mut evidence = PythonValueEvidenceSet::default();
        evidence.insert(PythonValueEvidence {
            origin: evidence_origin,
            constraints: first.clone(),
        });
        evidence.insert(PythonValueEvidence {
            origin: evidence_origin,
            constraints: second.clone(),
        });

        let mut expected = first;
        expected.merge(second);
        assert_eq!(
            evidence.0.len(),
            1,
            "one origin coalesces to one evidence entry"
        );
        assert_eq!(evidence.0[0].constraints, expected);
        assert_eq!(evidence.origins().collect::<Vec<_>>(), [evidence_origin]);
    }

    #[test]
    fn same_kind_concatenation_produces_the_correct_nominal_result() {
        let list = list_value(origin(1), vec![str_item(origin(2), "a")]).add(
            &list_value(origin(3), vec![str_item(origin(4), "b")]),
            origin(5),
        );
        assert!(matches!(list.kind, PythonValueKind::List(_)));
        assert_eq!(item_texts(&list), vec!["str:a", "str:b"]);
        assert_eq!(list.origins().collect::<Vec<_>>(), vec![origin(5)]);
        assert_eq!(
            site_origins(&list),
            vec![origin(5)],
            "list `+` allocates a fresh site"
        );

        let tuple = tuple_value(origin(1), vec![str_item(origin(2), "a")]).add(
            &tuple_value(origin(3), vec![str_item(origin(4), "b")]),
            origin(5),
        );
        assert!(matches!(tuple.kind, PythonValueKind::Tuple(_)));
        assert_eq!(item_texts(&tuple), vec!["str:a", "str:b"]);
        assert!(
            tuple.own_mutable_sites().is_none(),
            "tuple `+` owns no site"
        );

        let string = str_value(origin(1), "ab").add(&str_value(origin(2), "cd"), origin(3));
        assert!(matches!(&string.kind, PythonValueKind::Str(text) if text == "abcd"));
        assert_eq!(string.origins().collect::<Vec<_>>(), vec![origin(3)]);
    }

    #[test]
    fn binary_add_covers_every_nominal_kind_pair() {
        fn matrix_value(kind: usize) -> PythonValue {
            match kind {
                0 => list_value(origin(1), Vec::new()),
                1 => tuple_value(origin(1), Vec::new()),
                2 => str_value(origin(1), "s"),
                3 => dict_value(origin(1)),
                4 => bool_value(origin(1), true),
                5 => path_value(origin(1), "p"),
                6 => unknown_value(origin(1)),
                _ => unreachable!("the matrix has seven nominal kinds"),
            }
        }

        for left_kind in 0..7 {
            for right_kind in 0..7 {
                let result = matrix_value(left_kind).add(&matrix_value(right_kind), origin(9));
                let supported = matches!((left_kind, right_kind), (0, 0 | 6) | (1, 1 | 6) | (2, 2));
                assert_eq!(
                    !matches!(result.kind, PythonValueKind::Unknown(_)),
                    supported,
                    "binary `+` row {left_kind}, column {right_kind}",
                );
                assert_eq!(result.origins().collect::<Vec<_>>(), vec![origin(9)]);
            }
        }
    }

    #[test]
    fn known_sequence_plus_unknown_preserves_prefix_and_typed_remainder() {
        let list = list_value(origin(1), vec![str_item(origin(2), "a")])
            .add(&unknown_value(origin(3)), origin(4));
        assert_eq!(item_texts(&list), vec!["str:a", "unpack"]);

        let tuple = tuple_value(origin(1), vec![str_item(origin(2), "a")])
            .add(&unknown_value(origin(3)), origin(4));
        assert_eq!(item_texts(&tuple), vec!["str:a", "unpack"]);
    }

    #[test]
    fn cross_kind_and_incompatible_concatenation_is_unsupported() {
        let cases = [
            list_value(origin(1), Vec::new()).add(&tuple_value(origin(2), Vec::new()), origin(3)),
            tuple_value(origin(1), Vec::new()).add(&list_value(origin(2), Vec::new()), origin(3)),
            list_value(origin(1), Vec::new()).add(&str_value(origin(2), "x"), origin(3)),
            str_value(origin(1), "x").add(&unknown_value(origin(2)), origin(3)),
            bool_value(origin(1), true).add(&bool_value(origin(2), false), origin(3)),
            dict_value(origin(1)).add(&dict_value(origin(2)), origin(3)),
            unknown_value(origin(1)).add(&list_value(origin(2), Vec::new()), origin(3)),
        ];
        for result in cases {
            assert!(
                matches!(result.kind, PythonValueKind::Unknown(_)),
                "an unsupported binary `+` degrades to an unknown",
            );
            assert_eq!(result.origins().collect::<Vec<_>>(), vec![origin(3)]);
        }
    }

    #[test]
    fn iterable_classification_covers_every_value_kind() {
        assert!(matches!(
            list_value(origin(1), Vec::new()).iterable_knowledge(),
            PythonIterableKnowledge::Known(PythonIterable::Sequence(PythonSequence::List(_)))
        ));
        assert!(matches!(
            tuple_value(origin(1), Vec::new()).iterable_knowledge(),
            PythonIterableKnowledge::Known(PythonIterable::Sequence(PythonSequence::Tuple(_)))
        ));
        assert!(matches!(
            str_value(origin(1), "x").iterable_knowledge(),
            PythonIterableKnowledge::Known(PythonIterable::Sequence(PythonSequence::String(_)))
        ));
        assert!(matches!(
            dict_value(origin(1)).iterable_knowledge(),
            PythonIterableKnowledge::Known(PythonIterable::MappingKeys(_))
        ));
        assert!(matches!(
            bool_value(origin(1), true).iterable_knowledge(),
            PythonIterableKnowledge::NotIterable
        ));
        assert!(matches!(
            path_value(origin(1), "p").iterable_knowledge(),
            PythonIterableKnowledge::Indeterminate(_)
        ));
        assert!(matches!(
            unknown_value(origin(1)).iterable_knowledge(),
            PythonIterableKnowledge::Indeterminate(_)
        ));
    }

    #[test]
    fn sequence_projection_is_nominal() {
        // Lists, tuples, and strings are all honest Python sequences.
        assert!(matches!(
            list_value(origin(1), Vec::new()).sequence(),
            Some(PythonSequence::List(_))
        ));
        assert!(matches!(
            tuple_value(origin(1), Vec::new()).sequence(),
            Some(PythonSequence::Tuple(_))
        ));
        assert!(matches!(
            str_value(origin(1), "x").sequence(),
            Some(PythonSequence::String(_))
        ));
        for value in [
            bool_value(origin(1), true),
            path_value(origin(1), "p"),
            dict_value(origin(1)),
            unknown_value(origin(1)),
        ] {
            assert!(value.sequence().is_none());
        }
    }

    fn extend_list(source: &PythonValue) -> (Option<()>, PythonValue) {
        let mut receiver = list_value(origin(1), vec![str_item(origin(2), "seed")]);
        let PythonValueKind::List(list) = &mut receiver.kind else {
            panic!("the receiver should remain a list");
        };
        let extended = list.extend_from(source, origin(9));
        (extended, receiver)
    }

    #[test]
    fn canonical_unknown_origins_merge_top_level_values_commutatively() {
        let merged = |left: Origin, right: Origin| {
            let mut value = PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, [left]);
            value.merge_semantically_equal(
                PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, [right]),
                None,
            );
            value
        };

        let forward = merged(origin(20), origin(10));
        let reversed = merged(origin(10), origin(20));
        assert_eq!(forward, reversed);
        assert_eq!(
            forward.origins().collect::<Vec<_>>(),
            [origin(10), origin(20)]
        );
        let unknown = forward
            .unknown_value()
            .expect("value should remain unknown");
        assert_eq!(
            unknown.origins().collect::<Vec<_>>(),
            [origin(10), origin(20)]
        );

        let mut idempotent = forward.clone();
        idempotent.merge_semantically_equal(forward.clone(), None);
        assert_eq!(idempotent, forward);
    }

    #[test]
    fn canonical_unknown_origins_seed_imprecise_sequence_iteration() {
        let mut source = str_value(origin(20), "abc");
        source.merge_semantically_equal(str_value(origin(10), "abc"), None);
        let (ok, result) = extend_list(&source);
        assert!(ok.is_some());
        let PythonValueKind::List(list) = result.kind else {
            panic!("extension receiver should remain a list");
        };
        let PythonSequenceItem::UnknownUnpack(unknown) = list
            .semantic_items()
            .last()
            .expect("imprecise source should append an unpack")
        else {
            panic!("imprecise source should append an unknown unpack");
        };
        assert_eq!(
            unknown.origins().collect::<Vec<_>>(),
            [origin(10), origin(20)]
        );
    }

    #[test]
    fn extend_from_follows_every_iterable_source_row() {
        let (ok, list) = extend_list(&list_value(origin(3), vec![str_item(origin(4), "a")]));
        assert!(ok.is_some());
        assert_eq!(item_texts(&list), vec!["str:seed", "str:a"]);

        let (ok, tuple) = extend_list(&tuple_value(origin(3), vec![str_item(origin(4), "b")]));
        assert!(ok.is_some());
        assert_eq!(item_texts(&tuple), vec!["str:seed", "str:b"]);

        let (ok, string) = extend_list(&str_value(origin(3), "abc"));
        assert!(ok.is_some());
        assert_eq!(item_texts(&string), vec!["str:seed", "unpack"]);

        let (ok, empty_string) = extend_list(&str_value(origin(3), ""));
        assert!(ok.is_some());
        assert_eq!(item_texts(&empty_string), vec!["str:seed", "unpack"]);

        let mut dict = dict_value(origin(3));
        if let PythonValueKind::Dict(inner) = &mut dict.kind {
            inner.append_entry(str_value(origin(4), "k"), str_value(origin(6), "v"));
        }
        let (ok, from_dict) = extend_list(&dict);
        assert!(ok.is_some());
        assert_eq!(item_texts(&from_dict), vec!["str:seed", "unpack"]);

        let (ok, empty_dict) = extend_list(&dict_value(origin(3)));
        assert!(ok.is_some());
        assert_eq!(item_texts(&empty_dict), vec!["str:seed", "unpack"]);

        let (ok, from_unknown) = extend_list(&unknown_value(origin(3)));
        assert!(ok.is_some());
        assert_eq!(item_texts(&from_unknown), vec!["str:seed", "unpack"]);

        let (ok, from_path) = extend_list(&path_value(origin(3), "p"));
        assert!(ok.is_some());
        assert_eq!(item_texts(&from_path), vec!["str:seed", "unpack"]);

        let (ok, from_bool) = extend_list(&bool_value(origin(3), true));
        assert!(ok.is_none(), "a bool source is definitely not iterable");
        assert_eq!(item_texts(&from_bool), vec!["str:seed"]);
        assert_eq!(
            site_origins(&from_bool),
            vec![origin(1)],
            "a failed extension leaves the receiver untouched",
        );
    }
}
