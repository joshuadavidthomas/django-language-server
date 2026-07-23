use std::cmp::Ordering;
use std::iter;
use std::mem;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileReadError;
use djls_source::Origin;

use super::BranchConstraints;
use super::MappingLogItem;
use super::OriginSet;
use super::PythonDict;
use super::PythonList;
use super::PythonMapping;
use super::PythonSequence;
use super::PythonSequenceItem;
use super::PythonTuple;
use super::ReachableAllocationSites;
use super::StructuralOrd;
use super::allocation::AllocationSites;
use crate::python::PythonEnvIntrinsic;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonPath;
use crate::python::PythonPathIntrinsic;
use crate::python::PythonPathNamespace;
use crate::python::PythonSyntaxError;
use crate::python::module::PythonImportNameError;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValueEvidence {
    origin: Origin,
    constraints: BranchConstraints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar<'a> {
    String(&'a str),
    Bool(bool),
}

/// A known scalar together with proof that it has source provenance.
///
/// Construction stays behind [`PythonValue::known_scalar`], which returns no
/// projection when an internally malformed scalar has no evidence.
pub(crate) struct PythonKnownScalar<'a> {
    scalar: Scalar<'a>,
    first_evidence: &'a ValueEvidence,
    additional_evidence: &'a [ValueEvidence],
}

impl<'a> PythonKnownScalar<'a> {
    pub(crate) fn string_value(&self) -> Option<&'a str> {
        let Scalar::String(value) = self.scalar else {
            return None;
        };
        Some(value)
    }

    pub(crate) fn bool_value(&self) -> Option<bool> {
        let Scalar::Bool(value) = self.scalar else {
            return None;
        };
        Some(value)
    }

    pub(crate) fn first_origin(&self) -> Origin {
        self.first_evidence.origin
    }

    pub(crate) fn additional_origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.additional_evidence
            .iter()
            .map(|evidence| evidence.origin)
    }

    pub(crate) fn origins_with_constraints(
        &self,
    ) -> impl Iterator<Item = (Origin, &BranchConstraints)> {
        iter::once((self.first_evidence.origin, &self.first_evidence.constraints)).chain(
            self.additional_evidence
                .iter()
                .map(|evidence| (evidence.origin, &evidence.constraints)),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ValueEvidenceSet(Vec<ValueEvidence>);

impl StructuralOrd for ValueEvidenceSet {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        for (left, right) in self.0.iter().zip(&other.0) {
            let ordering = left.structural_cmp(right);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.0.len().cmp(&other.0.len())
    }
}

impl ValueEvidenceSet {
    fn one(origin: Origin) -> Self {
        Self(vec![ValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
        }])
    }

    fn from_origins(origins: impl IntoIterator<Item = Origin>) -> Self {
        let mut evidence = Self::default();
        evidence.extend(origins.into_iter().map(|origin| ValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
        }));
        evidence
    }

    fn insert(&mut self, evidence: ValueEvidence) {
        self.0.push(evidence);
        self.normalize();
    }

    fn extend(&mut self, evidence: impl IntoIterator<Item = ValueEvidence>) {
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
        self.0.push(ValueEvidence {
            origin,
            constraints: BranchConstraints::unconstrained(),
        });
    }

    fn record(&mut self, origin: Origin) {
        self.insert(ValueEvidence {
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
        self.0.sort_by(ValueEvidence::structural_cmp);
        let mut normalized: Vec<ValueEvidence> = Vec::with_capacity(self.0.len());
        for evidence in mem::take(&mut self.0) {
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

impl StructuralOrd for ValueEvidence {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.origin
            .structural_cmp(&other.origin)
            .then_with(|| self.constraints.structural_cmp(&other.constraints))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonValue {
    pub(in crate::python::evaluation) kind: PythonValueKind,
    evidence: ValueEvidenceSet,
}

impl StructuralOrd for PythonValue {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.kind
            .structural_cmp(&other.kind)
            .then_with(|| self.evidence.structural_cmp(&other.evidence))
    }
}

impl PythonValue {
    pub(super) fn unknown(
        cause: PythonUnknownCause,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Self {
        let unknown = PythonUnknown::new(cause, origins);
        let evidence = ValueEvidenceSet::from_origins(unknown.origins());
        Self {
            kind: PythonValueKind::Unknown(unknown),
            evidence,
        }
    }

    pub(super) fn string(value: String, origin: Origin) -> Self {
        Self::atomic(PythonValueKind::Str(value), origin)
    }

    pub(super) fn bool(value: bool, origin: Origin) -> Self {
        Self::atomic(PythonValueKind::Bool(value), origin)
    }

    pub(super) fn path(value: Utf8PathBuf, origin: Origin) -> Self {
        Self::python_path(PythonPath::object(value), origin)
    }

    pub(super) fn path_intrinsic(intrinsic: PythonPathIntrinsic, origin: Origin) -> Self {
        Self::python_path(PythonPath::intrinsic(intrinsic), origin)
    }

    pub(super) fn env_intrinsic(intrinsic: PythonEnvIntrinsic, origin: Origin) -> Self {
        Self::atomic(PythonValueKind::Env(intrinsic), origin)
    }

    pub(super) fn python_path(path: PythonPath, origin: Origin) -> Self {
        Self::atomic(PythonValueKind::Path(path), origin)
    }

    pub(super) fn unsupported_literal(origin: Origin) -> Self {
        Self::atomic(PythonValueKind::UnsupportedLiteral, origin)
    }

    pub(super) fn list(items: Vec<PythonSequenceItem>, origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::List(PythonList::new(items, origin)),
            evidence: ValueEvidenceSet::one(origin),
        }
    }

    pub(super) fn tuple(items: Vec<PythonSequenceItem>, origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::Tuple(PythonTuple::new(items)),
            evidence: ValueEvidenceSet::one(origin),
        }
    }

    /// A nominal module value. Identity only: it never embeds the module's
    /// intrinsic values or its loaded-child table.
    pub(super) fn module(id: PythonModule, origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::Module(id),
            evidence: ValueEvidenceSet::one(origin),
        }
    }

    pub(super) fn empty_dict(origin: Origin) -> Self {
        Self {
            kind: PythonValueKind::Dict(PythonDict::empty(origin)),
            evidence: ValueEvidenceSet::one(origin),
        }
    }

    pub(super) fn dict_entry(key: PythonValue, value: PythonValue, origin: Origin) -> Self {
        let mut dict = PythonDict::empty(origin);
        dict.append_entry(key, value);
        Self {
            kind: PythonValueKind::Dict(dict),
            evidence: ValueEvidenceSet::one(origin),
        }
    }

    fn atomic(kind: PythonValueKind, origin: Origin) -> Self {
        debug_assert!(matches!(
            &kind,
            PythonValueKind::Str(_)
                | PythonValueKind::Bool(_)
                | PythonValueKind::Path(_)
                | PythonValueKind::Env(_)
                | PythonValueKind::UnsupportedLiteral
        ));
        Self {
            kind,
            evidence: ValueEvidenceSet::one(origin),
        }
    }

    #[cfg(test)]
    fn known(kind: PythonValueKind, origin: Origin) -> Self {
        Self {
            kind,
            evidence: ValueEvidenceSet::one(origin),
        }
    }

    pub(crate) fn origins(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.evidence.origins()
    }

    pub(crate) fn known_scalar(&self) -> Option<PythonKnownScalar<'_>> {
        let scalar = match &self.kind {
            PythonValueKind::Str(value) => Scalar::String(value),
            PythonValueKind::Bool(value) => Scalar::Bool(*value),
            PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::List(_)
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => return None,
        };
        let (first_evidence, additional_evidence) = self.evidence.0.split_first()?;
        Some(PythonKnownScalar {
            scalar,
            first_evidence,
            additional_evidence,
        })
    }

    pub(crate) fn path_value(&self) -> Option<&Utf8Path> {
        let PythonValueKind::Path(value) = &self.kind else {
            return None;
        };
        value.object_path()
    }

    pub(super) fn mutable_namespace(&self) -> Option<PythonPathNamespace> {
        match &self.kind {
            PythonValueKind::Path(PythonPath::Intrinsic(intrinsic)) => {
                Some(intrinsic.mutable_namespace())
            }
            PythonValueKind::Env(intrinsic) => Some(intrinsic.mutable_namespace()),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(PythonPath::Object(_))
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::List(_)
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => None,
        }
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
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Module(_)
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
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => None,
        }
    }

    /// Classify this value's iterability. Lists, tuples, and strings are
    /// sequences; dictionaries are iterable over their keys; booleans,
    /// modules, exact `pathlib.Path` values, and path intrinsics are definitely
    /// not iterable. Unknown and `UnsupportedLiteral` values are indeterminate
    /// because their runtime iterability cannot be decided here.
    pub(super) fn iterability(&self) -> Iterability<'_> {
        match &self.kind {
            PythonValueKind::List(list) => {
                Iterability::Known(Iterable::Sequence(PythonSequence::List(list)))
            }
            PythonValueKind::Tuple(tuple) => {
                Iterability::Known(Iterable::Sequence(PythonSequence::Tuple(tuple)))
            }
            PythonValueKind::Str(text) => {
                Iterability::Known(Iterable::Sequence(PythonSequence::String(text)))
            }
            PythonValueKind::Dict(dict) => {
                Iterability::Known(Iterable::MappingKeys(dict.mapping()))
            }
            // Module objects and callable intrinsics are nominal values, never
            // Python sequences, mappings, or iterables.
            PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(
                PythonEnvIntrinsic::EnvironGetFunction | PythonEnvIntrinsic::GetenvFunction,
            )
            | PythonValueKind::Module(_) => Iterability::NotIterable,
            // `os.environ` is iterable but its contents are unknown;
            // `UnsupportedLiteral` may or may not be iterable.
            PythonValueKind::Env(PythonEnvIntrinsic::EnvironObject)
            | PythonValueKind::UnsupportedLiteral => {
                Iterability::Indeterminate(self.imprecise_iteration_unknown())
            }
            PythonValueKind::Unknown(unknown) => Iterability::Indeterminate(unknown.clone()),
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

    pub(super) fn iterated_reachable_allocation_sites(&self) -> ReachableAllocationSites {
        let mut sites = ReachableAllocationSites::default();
        match &self.kind {
            PythonValueKind::List(list) => {
                for item in list.semantic_items() {
                    if let PythonSequenceItem::Value(value) = item {
                        value.collect_reachable_sites(&mut sites);
                    }
                }
            }
            PythonValueKind::Tuple(tuple) => {
                for item in tuple.semantic_items() {
                    if let PythonSequenceItem::Value(value) = item {
                        value.collect_reachable_sites(&mut sites);
                    }
                }
            }
            PythonValueKind::Dict(dict) => {
                for item in dict.mapping().projection() {
                    if let MappingLogItem::Entry { key, .. } = item {
                        key.collect_reachable_sites(&mut sites);
                    }
                }
            }
            PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral => {}
        }
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
            PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral => {}
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
            PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral => 0,
        }
    }

    pub(crate) fn has_origin_feasible_under(
        &self,
        wanted: Origin,
        constraints: &BranchConstraints,
    ) -> bool {
        self.origins_with_constraints()
            .any(|(origin, evidence_constraints)| {
                origin == wanted
                    && !evidence_constraints
                        .intersection(constraints)
                        .is_impossible()
            })
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
            PythonValueKind::Module(_)
            | PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral => false,
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
                | PythonValueKind::Env(_)
                | PythonValueKind::UnsupportedLiteral
                | PythonValueKind::List(_)
                | PythonValueKind::Tuple(_)
                | PythonValueKind::Dict(_)
                | PythonValueKind::Module(_)
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
    pub(super) fn push_constructed_element(&mut self, value: PythonValue) -> bool {
        let item = match value.kind {
            PythonValueKind::Unknown(unknown) => PythonSequenceItem::UnknownElement(unknown),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::List(_)
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_) => PythonSequenceItem::Value(value),
        };
        match &mut self.kind {
            PythonValueKind::List(list) => {
                list.append(&item);
                true
            }
            PythonValueKind::Tuple(tuple) => {
                tuple.append(&item);
                true
            }
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => false,
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
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => None,
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
            // Module identity is separate from allocation provenance, so only
            // the value evidence rebases.
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_) => {}
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
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Module(_)
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
    ) -> bool {
        if !self.same_semantic_value(&incoming) {
            return false;
        }
        self.evidence.merge(incoming.evidence);
        let merged = self
            .kind
            .merge_semantically_equal(incoming.kind, operation_origin);
        self.normalize();
        merged
    }
}

/// The deliberately partial abstract value domain used by static settings and
/// import evaluation, not an exhaustive model of Python runtime types.
///
/// Variants exist only for facts current consumers can use and for nominal
/// identities required to evaluate those facts. Closed literals outside that
/// domain retain the weaker `UnsupportedLiteral` fact; expressions whose result
/// cannot be represented conservatively become `Unknown`. Add a variant only
/// when a consumer needs the distinction and its joins, operations, and
/// projections can be defined without executing Python.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonValueKind {
    Str(String),
    Bool(bool),
    Path(PythonPath),
    Env(PythonEnvIntrinsic),
    UnsupportedLiteral,
    List(PythonList),
    Tuple(PythonTuple),
    Dict(PythonDict),
    Module(PythonModule),
    Unknown(PythonUnknown),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PythonValueKindOrder {
    Bool,
    Dict,
    List,
    Module,
    PathObject,
    Env,
    Str,
    Tuple,
    UnsupportedLiteral,
    PathIntrinsic,
    Unknown,
}

impl PythonValueKind {
    fn structural_order(&self) -> PythonValueKindOrder {
        match self {
            Self::Bool(_) => PythonValueKindOrder::Bool,
            Self::Dict(_) => PythonValueKindOrder::Dict,
            Self::List(_) => PythonValueKindOrder::List,
            Self::Module(_) => PythonValueKindOrder::Module,
            Self::Path(PythonPath::Object(_)) => PythonValueKindOrder::PathObject,
            Self::Env(_) => PythonValueKindOrder::Env,
            Self::Str(_) => PythonValueKindOrder::Str,
            Self::Tuple(_) => PythonValueKindOrder::Tuple,
            Self::UnsupportedLiteral => PythonValueKindOrder::UnsupportedLiteral,
            Self::Path(PythonPath::Intrinsic(_)) => PythonValueKindOrder::PathIntrinsic,
            Self::Unknown(_) => PythonValueKindOrder::Unknown,
        }
    }
}

impl StructuralOrd for PythonValueKind {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Str(left), Self::Str(right)) => left.cmp(right),
            (Self::Bool(left), Self::Bool(right)) => left.cmp(right),
            (Self::Path(PythonPath::Object(left)), Self::Path(PythonPath::Object(right))) => {
                left.cmp(right)
            }
            (Self::Path(PythonPath::Intrinsic(left)), Self::Path(PythonPath::Intrinsic(right))) => {
                left.structural_rank().cmp(&right.structural_rank())
            }
            (Self::Env(left), Self::Env(right)) => {
                left.structural_rank().cmp(&right.structural_rank())
            }
            (Self::UnsupportedLiteral, Self::UnsupportedLiteral) => Ordering::Equal,
            (Self::List(left), Self::List(right)) => left.structural_cmp(right),
            (Self::Tuple(left), Self::Tuple(right)) => left.structural_cmp(right),
            (Self::Dict(left), Self::Dict(right)) => left.structural_cmp(right),
            (Self::Module(left), Self::Module(right)) => left.structural_cmp(right),
            (Self::Unknown(left), Self::Unknown(right)) => left.structural_cmp(right),
            (
                left @ (Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::Env(_)
                | Self::UnsupportedLiteral
                | Self::List(_)
                | Self::Tuple(_)
                | Self::Dict(_)
                | Self::Module(_)
                | Self::Unknown(_)),
                right @ (Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::Env(_)
                | Self::UnsupportedLiteral
                | Self::List(_)
                | Self::Tuple(_)
                | Self::Dict(_)
                | Self::Module(_)
                | Self::Unknown(_)),
            ) => left.structural_order().cmp(&right.structural_order()),
        }
    }
}

impl PythonValueKind {
    fn normalize(&mut self) {
        match self {
            Self::List(list) => list.normalize(),
            Self::Tuple(tuple) => tuple.normalize(),
            Self::Dict(dict) => dict.normalize(),
            Self::Str(_)
            | Self::Bool(_)
            | Self::Path(_)
            | Self::Env(_)
            | Self::UnsupportedLiteral
            | Self::Module(_)
            | Self::Unknown(_) => {}
        }
    }

    fn same_semantic_value(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Path(left), Self::Path(right)) => left == right,
            (Self::Env(left), Self::Env(right)) => left == right,
            (Self::UnsupportedLiteral, Self::UnsupportedLiteral) => true,
            (Self::List(left), Self::List(right)) => left.same_semantic_value(right),
            (Self::Tuple(left), Self::Tuple(right)) => left.same_semantic_value(right),
            (Self::Dict(left), Self::Dict(right)) => left.same_semantic_value(right),
            // Module identity: same nominal object is the same semantic value.
            (Self::Module(left), Self::Module(right)) => left == right,
            (Self::Unknown(left), Self::Unknown(right)) => left.cause == right.cause,
            (
                Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::Env(_)
                | Self::UnsupportedLiteral
                | Self::List(_)
                | Self::Tuple(_)
                | Self::Dict(_)
                | Self::Module(_)
                | Self::Unknown(_),
                _,
            ) => false,
        }
    }

    fn merge_semantically_equal(
        &mut self,
        incoming: Self,
        operation_origin: Option<Origin>,
    ) -> bool {
        if !self.same_semantic_value(&incoming) {
            return false;
        }
        match (self, incoming) {
            (Self::List(existing), Self::List(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin)
            }
            (Self::Tuple(existing), Self::Tuple(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin)
            }
            (Self::Dict(existing), Self::Dict(incoming)) => {
                existing.merge_semantically_equal(incoming, operation_origin)
            }
            (Self::Unknown(existing), Self::Unknown(incoming)) => {
                existing.merge_origins(&incoming);
                true
            }
            // Atomic facts and module identity carry no mergeable payload;
            // equal values stay equal.
            (Self::Str(_), Self::Str(_))
            | (Self::Bool(_), Self::Bool(_))
            | (Self::Path(_), Self::Path(_))
            | (Self::Env(_), Self::Env(_))
            | (Self::UnsupportedLiteral, Self::UnsupportedLiteral)
            | (Self::Module(_), Self::Module(_)) => true,
            (
                Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::Env(_)
                | Self::UnsupportedLiteral
                | Self::List(_)
                | Self::Tuple(_)
                | Self::Dict(_)
                | Self::Module(_)
                | Self::Unknown(_),
                Self::Str(_)
                | Self::Bool(_)
                | Self::Path(_)
                | Self::Env(_)
                | Self::UnsupportedLiteral
                | Self::List(_)
                | Self::Tuple(_)
                | Self::Dict(_)
                | Self::Module(_)
                | Self::Unknown(_),
            ) => false,
        }
    }
}

/// The classification of a value's iterability: what an iteration consumer can
/// know about iterating it. It is a data-bearing projection over the closed
/// value model, not a stored capability tag.
pub(super) enum Iterability<'a> {
    Known(Iterable<'a>),
    Indeterminate(PythonUnknown),
    NotIterable,
}

/// A definitely-iterable value: a sequence (list, tuple, or string) or a
/// mapping iterated over its keys.
pub(super) enum Iterable<'a> {
    Sequence(PythonSequence<'a>),
    MappingKeys(PythonMapping<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonUnknown {
    pub(crate) cause: PythonUnknownCause,
    origins: OriginSet,
}

impl StructuralOrd for PythonUnknown {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.cause
            .structural_cmp(&other.cause)
            .then_with(|| self.origins.structural_cmp(&other.origins))
    }
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
    InvalidImport(PythonImportNameError),
    ImportNotFound(PythonModuleName),
    MissingImportMember {
        module: PythonModuleName,
        member: String,
    },
    /// A module attribute read that resolved to no attached child and no
    /// intrinsic source binding. This is the caller-neutral expression-read
    /// residual, distinct from `MissingImportMember` (the import caller's
    /// failure policy).
    ModuleAttribute {
        module: PythonModuleName,
        member: String,
    },
    SkippedExternal(PythonModuleName),
    Unreadable(FileReadError),
    SyntaxErrors(Vec<PythonSyntaxError>),
    Cycle,
    AlternativeLimitExceeded,
    EnvValueUnknown {
        key: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PythonUnknownCauseOrder {
    AlternativeLimitExceeded,
    Cycle,
    ImportNotFound,
    InvalidImport,
    MissingImportMember,
    SkippedExternal,
    SyntaxErrors,
    Unreadable,
    UnsupportedExpression,
    UnsupportedMutation,
    ModuleAttribute,
    EnvValueUnknown,
}

impl PythonUnknownCause {
    fn structural_order(&self) -> PythonUnknownCauseOrder {
        match self {
            Self::AlternativeLimitExceeded => PythonUnknownCauseOrder::AlternativeLimitExceeded,
            Self::Cycle => PythonUnknownCauseOrder::Cycle,
            Self::ImportNotFound(_) => PythonUnknownCauseOrder::ImportNotFound,
            Self::InvalidImport(_) => PythonUnknownCauseOrder::InvalidImport,
            Self::MissingImportMember { .. } => PythonUnknownCauseOrder::MissingImportMember,
            Self::SkippedExternal(_) => PythonUnknownCauseOrder::SkippedExternal,
            Self::SyntaxErrors(_) => PythonUnknownCauseOrder::SyntaxErrors,
            Self::Unreadable(_) => PythonUnknownCauseOrder::Unreadable,
            Self::UnsupportedExpression => PythonUnknownCauseOrder::UnsupportedExpression,
            Self::UnsupportedMutation => PythonUnknownCauseOrder::UnsupportedMutation,
            Self::ModuleAttribute { .. } => PythonUnknownCauseOrder::ModuleAttribute,
            Self::EnvValueUnknown { .. } => PythonUnknownCauseOrder::EnvValueUnknown,
        }
    }
}

impl StructuralOrd for PythonUnknownCause {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::UnsupportedExpression, Self::UnsupportedExpression)
            | (Self::UnsupportedMutation, Self::UnsupportedMutation)
            | (Self::Cycle, Self::Cycle)
            | (Self::AlternativeLimitExceeded, Self::AlternativeLimitExceeded) => Ordering::Equal,
            (Self::InvalidImport(left), Self::InvalidImport(right)) => left.structural_cmp(right),
            (Self::ImportNotFound(left), Self::ImportNotFound(right))
            | (Self::SkippedExternal(left), Self::SkippedExternal(right)) => left.cmp(right),
            (
                Self::MissingImportMember {
                    module: left_module,
                    member: left_member,
                },
                Self::MissingImportMember {
                    module: right_module,
                    member: right_member,
                },
            )
            | (
                Self::ModuleAttribute {
                    module: left_module,
                    member: left_member,
                },
                Self::ModuleAttribute {
                    module: right_module,
                    member: right_member,
                },
            ) => left_module
                .cmp(right_module)
                .then_with(|| left_member.cmp(right_member)),
            (Self::EnvValueUnknown { key: left }, Self::EnvValueUnknown { key: right }) => {
                left.cmp(right)
            }
            (Self::Unreadable(left), Self::Unreadable(right)) => left.structural_cmp(right),
            (Self::SyntaxErrors(left), Self::SyntaxErrors(right)) => {
                left.as_slice().structural_cmp(right.as_slice())
            }
            (
                left @ (Self::UnsupportedExpression
                | Self::UnsupportedMutation
                | Self::InvalidImport(_)
                | Self::ImportNotFound(_)
                | Self::MissingImportMember { .. }
                | Self::ModuleAttribute { .. }
                | Self::SkippedExternal(_)
                | Self::Unreadable(_)
                | Self::SyntaxErrors(_)
                | Self::Cycle
                | Self::AlternativeLimitExceeded
                | Self::EnvValueUnknown { .. }),
                right @ (Self::UnsupportedExpression
                | Self::UnsupportedMutation
                | Self::InvalidImport(_)
                | Self::ImportNotFound(_)
                | Self::MissingImportMember { .. }
                | Self::ModuleAttribute { .. }
                | Self::SkippedExternal(_)
                | Self::Unreadable(_)
                | Self::SyntaxErrors(_)
                | Self::Cycle
                | Self::AlternativeLimitExceeded
                | Self::EnvValueUnknown { .. }),
            ) => left.structural_order().cmp(&right.structural_order()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::io::ErrorKind;

    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::FileReadError;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::BranchConstraints;
    use super::Iterability;
    use super::Iterable;
    use super::Origin;
    use super::PythonDict;
    use super::PythonList;
    use super::PythonSequence;
    use super::PythonSequenceItem;
    use super::PythonTuple;
    use super::PythonUnknown;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use super::PythonValueKind;
    use super::ReachableAllocationSites;
    use super::StructuralOrd;
    use super::ValueEvidence;
    use super::ValueEvidenceSet;
    use crate::python::InvalidModuleName;
    use crate::python::PythonModule;
    use crate::python::PythonModuleName;
    use crate::python::PythonPath;
    use crate::python::PythonPathIntrinsic;
    use crate::python::PythonSyntaxError;
    use crate::python::PythonSyntaxErrorClass;
    use crate::python::module::PythonImportNameError;

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
        PythonValue::known(
            PythonValueKind::Path(PythonPath::Object(Utf8PathBuf::from(text))),
            site,
        )
    }

    fn path_intrinsic_value(site: Origin, intrinsic: PythonPathIntrinsic) -> PythonValue {
        PythonValue::known(
            PythonValueKind::Path(PythonPath::Intrinsic(intrinsic)),
            site,
        )
    }

    fn unsupported_literal_value(site: Origin) -> PythonValue {
        PythonValue::known(PythonValueKind::UnsupportedLiteral, site)
    }

    fn module_value(site: Origin, name: &str) -> PythonValue {
        let id = PythonModule::Namespace(crate::python::PythonNamespacePackage::new(
            PythonModuleName::parse(name).expect("valid module name"),
            Vec::new(),
        ));
        PythonValue::known(PythonValueKind::Module(id), site)
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
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => panic!("expected a sequence value"),
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
        origins.sort_by(StructuralOrd::structural_cmp);
        origins
    }

    #[test]
    fn typed_value_order_is_total_across_value_and_unknown_variants() {
        let values = [
            bool_value(origin(1), false),
            dict_value(origin(1)),
            list_value(origin(1), Vec::new()),
            module_value(origin(1), "pkg"),
            path_value(origin(1), "path"),
            str_value(origin(1), "text"),
            tuple_value(origin(1), Vec::new()),
            unsupported_literal_value(origin(1)),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::BuiltinsModule),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::BuiltinStrType),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibModule),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibPathType),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::OsModule),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::OsPathModule),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::OsPathJoinFunction),
            path_intrinsic_value(origin(1), PythonPathIntrinsic::OsPathDirnameFunction),
            unknown_value(origin(1)),
        ];
        for (index, left) in values.iter().enumerate() {
            for (other_index, right) in values.iter().enumerate() {
                let ordering = left.structural_cmp(right);
                assert_eq!(ordering, right.structural_cmp(left).reverse());
                assert_eq!(ordering == Ordering::Equal, left == right);
                assert_eq!(ordering, index.cmp(&other_index));
            }
        }

        let module = |name| PythonModuleName::parse(name).expect("valid module name");
        let causes = vec![
            PythonUnknownCause::AlternativeLimitExceeded,
            PythonUnknownCause::Cycle,
            PythonUnknownCause::ImportNotFound(module("a")),
            PythonUnknownCause::InvalidImport(PythonImportNameError::EmptyAbsoluteImport),
            PythonUnknownCause::MissingImportMember {
                module: module("a"),
                member: "member".to_string(),
            },
            PythonUnknownCause::SkippedExternal(module("a")),
            PythonUnknownCause::SyntaxErrors(vec![PythonSyntaxError {
                class: PythonSyntaxErrorClass::Ordinary,
                span: Span::new(1, 2),
                message: "syntax".to_string(),
            }]),
            PythonUnknownCause::Unreadable(FileReadError::new(
                Utf8PathBuf::from("a.py"),
                ErrorKind::PermissionDenied,
            )),
            PythonUnknownCause::UnsupportedExpression,
            PythonUnknownCause::UnsupportedMutation,
            PythonUnknownCause::ModuleAttribute {
                module: module("a"),
                member: "member".to_string(),
            },
        ];
        for (index, left) in causes.iter().enumerate() {
            for (other_index, right) in causes.iter().enumerate() {
                let ordering = left.structural_cmp(right);
                assert_eq!(ordering, right.structural_cmp(left).reverse());
                assert_eq!(ordering == Ordering::Equal, left == right);
                assert_eq!(ordering, index.cmp(&other_index));
            }
        }

        let import_errors = [
            PythonImportNameError::EmptyAbsoluteImport,
            PythonImportNameError::InvalidModuleName(InvalidModuleName::Empty),
            PythonImportNameError::TooManyDots,
        ];
        for (index, left) in import_errors.iter().enumerate() {
            for (other_index, right) in import_errors.iter().enumerate() {
                assert_eq!(left.structural_cmp(right), index.cmp(&other_index));
            }
        }

        let module_name_errors = [
            InvalidModuleName::ContainsConsecutiveDots,
            InvalidModuleName::ContainsWhitespace,
            InvalidModuleName::Empty,
            InvalidModuleName::EndsWithDot,
            InvalidModuleName::InvalidSegment("segment".to_string()),
            InvalidModuleName::MustHavePyExtension,
            InvalidModuleName::SourcePathIsAbsolute("/absolute.py".to_string()),
            InvalidModuleName::StartsWithDot,
        ];
        for (index, left) in module_name_errors.iter().enumerate() {
            for (other_index, right) in module_name_errors.iter().enumerate() {
                let ordering = left.structural_cmp(right);
                assert_eq!(ordering, right.structural_cmp(left).reverse());
                assert_eq!(ordering == Ordering::Equal, left == right);
                assert_eq!(ordering, index.cmp(&other_index));
            }
        }
    }

    #[test]
    fn typed_value_order_compares_every_unknown_payload_and_exact_evidence() {
        let module = |name| PythonModuleName::parse(name).expect("valid module name");
        let payload_pairs = [
            (
                PythonUnknownCause::InvalidImport(PythonImportNameError::InvalidModuleName(
                    InvalidModuleName::InvalidSegment("a".to_string()),
                )),
                PythonUnknownCause::InvalidImport(PythonImportNameError::InvalidModuleName(
                    InvalidModuleName::InvalidSegment("b".to_string()),
                )),
            ),
            (
                PythonUnknownCause::ImportNotFound(module("a")),
                PythonUnknownCause::ImportNotFound(module("b")),
            ),
            (
                PythonUnknownCause::InvalidImport(PythonImportNameError::EmptyAbsoluteImport),
                PythonUnknownCause::InvalidImport(PythonImportNameError::TooManyDots),
            ),
            (
                PythonUnknownCause::MissingImportMember {
                    module: module("a"),
                    member: "a".to_string(),
                },
                PythonUnknownCause::MissingImportMember {
                    module: module("a"),
                    member: "b".to_string(),
                },
            ),
            (
                PythonUnknownCause::MissingImportMember {
                    module: module("a"),
                    member: "same".to_string(),
                },
                PythonUnknownCause::MissingImportMember {
                    module: module("b"),
                    member: "same".to_string(),
                },
            ),
            (
                PythonUnknownCause::SkippedExternal(module("a")),
                PythonUnknownCause::SkippedExternal(module("b")),
            ),
            (
                PythonUnknownCause::Unreadable(FileReadError::new(
                    Utf8PathBuf::from("a.py"),
                    ErrorKind::PermissionDenied,
                )),
                PythonUnknownCause::Unreadable(FileReadError::new(
                    Utf8PathBuf::from("b.py"),
                    ErrorKind::PermissionDenied,
                )),
            ),
            (
                PythonUnknownCause::Unreadable(FileReadError::new(
                    Utf8PathBuf::from("same.py"),
                    ErrorKind::NotFound,
                )),
                PythonUnknownCause::Unreadable(FileReadError::new(
                    Utf8PathBuf::from("same.py"),
                    ErrorKind::PermissionDenied,
                )),
            ),
        ];
        for (left, right) in payload_pairs {
            assert_ne!(left, right);
            assert_ne!(left.structural_cmp(&right), Ordering::Equal);
        }
    }

    #[test]
    fn typed_value_order_compares_every_syntax_error_payload() {
        let syntax_error = |class, span, message: &str| {
            PythonUnknownCause::SyntaxErrors(vec![PythonSyntaxError {
                class,
                span,
                message: message.to_string(),
            }])
        };
        let ordinary = syntax_error(PythonSyntaxErrorClass::Ordinary, Span::new(1, 2), "same");
        for different in [
            syntax_error(PythonSyntaxErrorClass::Unsupported, Span::new(1, 2), "same"),
            syntax_error(PythonSyntaxErrorClass::Ordinary, Span::new(2, 2), "same"),
            syntax_error(PythonSyntaxErrorClass::Ordinary, Span::new(1, 3), "same"),
            syntax_error(
                PythonSyntaxErrorClass::Ordinary,
                Span::new(1, 2),
                "different",
            ),
            PythonUnknownCause::SyntaxErrors(vec![
                PythonSyntaxError {
                    class: PythonSyntaxErrorClass::Ordinary,
                    span: Span::new(1, 2),
                    message: "same".to_string(),
                },
                PythonSyntaxError {
                    class: PythonSyntaxErrorClass::Ordinary,
                    span: Span::new(2, 2),
                    message: "second".to_string(),
                },
            ]),
        ] {
            assert_ne!(ordinary, different);
            assert_ne!(ordinary.structural_cmp(&different), Ordering::Equal);
        }
    }

    #[test]
    fn typed_value_order_compares_unknown_origins_and_exact_value_evidence() {
        let left = PythonUnknown::new(PythonUnknownCause::Cycle, [origin(1)]);
        let right = PythonUnknown::new(PythonUnknownCause::Cycle, [origin(2)]);
        assert_ne!(left.structural_cmp(&right), Ordering::Equal);

        let join = origin(20);
        let mut first_constraints = BranchConstraints::unconstrained();
        first_constraints.select(join, 0);
        let mut second_constraints = BranchConstraints::unconstrained();
        second_constraints.select(join, 1);
        let first = ValueEvidenceSet(vec![ValueEvidence {
            origin: origin(10),
            constraints: first_constraints,
        }]);
        let different_constraints = ValueEvidenceSet(vec![ValueEvidence {
            origin: origin(10),
            constraints: second_constraints,
        }]);
        let different_origin = ValueEvidenceSet(vec![ValueEvidence {
            origin: origin(11),
            constraints: BranchConstraints::unconstrained(),
        }]);
        for other in [&different_constraints, &different_origin] {
            assert_ne!(&first, other);
            assert_ne!(first.structural_cmp(other), Ordering::Equal);
        }

        let evidence = |entries: [(Origin, BranchConstraints); 2]| {
            let mut evidence = ValueEvidenceSet::default();
            for (origin, constraints) in entries {
                evidence.insert(ValueEvidence {
                    origin,
                    constraints,
                });
            }
            evidence
        };
        let forward = evidence([
            (origin(11), BranchConstraints::unconstrained()),
            (origin(10), BranchConstraints::unconstrained()),
        ]);
        let reversed = evidence([
            (origin(10), BranchConstraints::unconstrained()),
            (origin(11), BranchConstraints::unconstrained()),
        ]);
        assert_eq!(forward, reversed);
        assert_eq!(forward.structural_cmp(&reversed), Ordering::Equal);

        let mut merged = str_value(origin(1), "same");
        let incoming = str_value(origin(2), "same");
        assert!(merged.same_semantic_value(&incoming));
        assert_ne!(merged.structural_cmp(&incoming), Ordering::Equal);
        merged.merge_semantically_equal(incoming, None);
        assert_eq!(merged.origins().collect::<Vec<_>>(), [origin(1), origin(2)]);
    }

    #[test]
    fn known_scalar_projection_carries_nonempty_string_and_bool_evidence() {
        let mut string = str_value(origin(2), "same");
        string.merge_semantically_equal(str_value(origin(1), "same"), None);
        let scalar = string
            .known_scalar()
            .expect("known string with evidence should project");

        assert_eq!(scalar.string_value(), Some("same"));
        assert_eq!(scalar.bool_value(), None);
        assert_eq!(scalar.first_origin(), origin(1));
        assert_eq!(scalar.additional_origins().collect::<Vec<_>>(), [origin(2)]);
        assert_eq!(
            scalar
                .origins_with_constraints()
                .map(|(origin, _)| origin)
                .collect::<Vec<_>>(),
            [origin(1), origin(2)]
        );

        let value = bool_value(origin(3), true);
        let scalar = value
            .known_scalar()
            .expect("known bool with evidence should project");
        assert_eq!(scalar.string_value(), None);
        assert_eq!(scalar.bool_value(), Some(true));
        assert_eq!(scalar.first_origin(), origin(3));
        assert!(scalar.additional_origins().next().is_none());
    }

    #[test]
    fn scalar_without_evidence_has_no_known_projection() {
        let string = PythonValue {
            kind: PythonValueKind::Str("malformed".to_string()),
            evidence: ValueEvidenceSet::default(),
        };
        assert!(string.known_scalar().is_none());

        let boolean = PythonValue {
            kind: PythonValueKind::Bool(true),
            evidence: ValueEvidenceSet::default(),
        };
        assert!(boolean.known_scalar().is_none());
    }

    #[test]
    fn path_intrinsics_and_unsupported_literals_have_only_atomic_value_state() {
        let path_intrinsic = path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibPathType);
        let unsupported_literal = unsupported_literal_value(origin(2));

        for value in [&path_intrinsic, &unsupported_literal] {
            assert!(value.known_scalar().is_none());
            assert!(value.path_value().is_none());
            assert!(value.sequence().is_none());
            assert!(value.mapping().is_none());
            assert!(value.unknown_value().is_none());
            assert!(value.own_mutable_sites().is_none());
            assert!(value.reachable_allocation_sites().is_empty());
        }

        let mut constructed = list_value(origin(3), Vec::new());
        assert!(constructed.push_constructed_element(path_intrinsic));
        assert!(constructed.push_constructed_element(unsupported_literal));
        let list = match constructed.kind {
            PythonValueKind::List(list) => Some(list),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => None,
        }
        .expect("constructed value should remain a list");
        assert!(matches!(
            &list.semantic_items()[0],
            PythonSequenceItem::Value(PythonValue {
                kind: PythonValueKind::Path(PythonPath::Intrinsic(
                    PythonPathIntrinsic::PathlibPathType
                )),
                ..
            })
        ));
        assert!(matches!(
            &list.semantic_items()[1],
            PythonSequenceItem::Value(PythonValue {
                kind: PythonValueKind::UnsupportedLiteral,
                ..
            })
        ));
    }

    #[test]
    fn construction_mutations_reject_non_sequence_receivers_without_panicking() {
        let mut receiver = str_value(origin(1), "receiver");
        let original = receiver.clone();

        assert!(!receiver.push_constructed_element(str_value(origin(2), "element")));
        assert_eq!(receiver, original);
        assert_eq!(
            receiver.star_extend_construction(&list_value(origin(3), Vec::new()), origin(4)),
            None
        );
        assert_eq!(receiver, original);
    }

    #[test]
    fn mismatched_semantic_merge_returns_false_and_preserves_the_receiver() {
        let mut receiver = list_value(origin(1), Vec::new());
        let original = receiver.clone();

        assert!(!receiver.merge_semantically_equal(str_value(origin(2), "other"), None));
        assert_eq!(receiver, original);
    }

    #[test]
    fn path_intrinsics_and_unsupported_literals_rebase_constrain_and_merge_evidence() {
        let join = origin(10);
        let mut constraints = BranchConstraints::unconstrained();
        constraints.select(join, 0);

        for mut value in [
            path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibPathType),
            unsupported_literal_value(origin(1)),
        ] {
            value.constrain_value_evidence(&constraints);
            assert_eq!(
                value.origins_with_constraints().collect::<Vec<_>>(),
                [(origin(1), &constraints)]
            );

            value.rebase_origin(origin(2));
            let unconstrained = BranchConstraints::unconstrained();
            assert_eq!(
                value.origins_with_constraints().collect::<Vec<_>>(),
                [(origin(2), &unconstrained)]
            );
        }

        let mut path_intrinsic =
            path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibPathType);
        let same_symbol = path_intrinsic_value(origin(2), PythonPathIntrinsic::PathlibPathType);
        let different_symbol =
            path_intrinsic_value(origin(3), PythonPathIntrinsic::OsPathJoinFunction);
        assert!(path_intrinsic.same_semantic_value(&same_symbol));
        assert!(!path_intrinsic.same_semantic_value(&different_symbol));
        path_intrinsic.merge_semantically_equal(same_symbol, None);
        assert_eq!(
            path_intrinsic.origins().collect::<Vec<_>>(),
            [origin(1), origin(2)]
        );
        assert!(matches!(
            path_intrinsic.kind,
            PythonValueKind::Path(PythonPath::Intrinsic(PythonPathIntrinsic::PathlibPathType))
        ));

        let mut unsupported_literal = unsupported_literal_value(origin(1));
        let incoming = unsupported_literal_value(origin(2));
        assert!(unsupported_literal.same_semantic_value(&incoming));
        unsupported_literal.merge_semantically_equal(incoming, None);
        assert_eq!(
            unsupported_literal.origins().collect::<Vec<_>>(),
            [origin(1), origin(2)]
        );
        assert!(matches!(
            unsupported_literal.kind,
            PythonValueKind::UnsupportedLiteral
        ));
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
        let list = match &mut receiver.kind {
            PythonValueKind::List(list) => Some(list),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => None,
        }
        .expect("the receiver should remain a list");
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

        let mut evidence = ValueEvidenceSet::default();
        evidence.insert(ValueEvidence {
            origin: evidence_origin,
            constraints: first.clone(),
        });
        evidence.insert(ValueEvidence {
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
        let matrix_values = || {
            [
                list_value(origin(1), Vec::new()),
                tuple_value(origin(1), Vec::new()),
                str_value(origin(1), "s"),
                dict_value(origin(1)),
                bool_value(origin(1), true),
                path_value(origin(1), "p"),
                path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibPathType),
                unsupported_literal_value(origin(1)),
                unknown_value(origin(1)),
            ]
        };

        for (left_kind, left) in matrix_values().into_iter().enumerate() {
            for (right_kind, right) in matrix_values().into_iter().enumerate() {
                let result = left.clone().add(&right, origin(9));
                let supported = matches!((left_kind, right_kind), (0, 0 | 8) | (1, 1 | 8) | (2, 2));
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
            list_value(origin(1), Vec::new()).iterability(),
            Iterability::Known(Iterable::Sequence(PythonSequence::List(_)))
        ));
        assert!(matches!(
            tuple_value(origin(1), Vec::new()).iterability(),
            Iterability::Known(Iterable::Sequence(PythonSequence::Tuple(_)))
        ));
        assert!(matches!(
            str_value(origin(1), "x").iterability(),
            Iterability::Known(Iterable::Sequence(PythonSequence::String(_)))
        ));
        assert!(matches!(
            dict_value(origin(1)).iterability(),
            Iterability::Known(Iterable::MappingKeys(_))
        ));
        assert!(matches!(
            bool_value(origin(1), true).iterability(),
            Iterability::NotIterable
        ));
        assert!(matches!(
            path_value(origin(1), "p").iterability(),
            Iterability::NotIterable
        ));
        assert!(matches!(
            path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibPathType).iterability(),
            Iterability::NotIterable
        ));
        assert!(matches!(
            unsupported_literal_value(origin(1)).iterability(),
            Iterability::Indeterminate(_)
        ));
        assert!(matches!(
            unknown_value(origin(1)).iterability(),
            Iterability::Indeterminate(_)
        ));
    }

    #[test]
    fn module_value_is_never_a_sequence_mapping_or_iterable() {
        let module = module_value(origin(1), "pkg");
        assert!(module.sequence().is_none(), "a module is not a sequence");
        assert!(module.mapping().is_none(), "a module is not a mapping");
        assert!(
            matches!(module.iterability(), Iterability::NotIterable),
            "a module object is a nominal value and never iterable",
        );
        assert!(module.known_scalar().is_none(), "a module is not a scalar");
        assert!(module.path_value().is_none());
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
            path_intrinsic_value(origin(1), PythonPathIntrinsic::PathlibPathType),
            unsupported_literal_value(origin(1)),
            dict_value(origin(1)),
            unknown_value(origin(1)),
        ] {
            assert!(value.sequence().is_none());
        }
    }

    fn extend_list(source: &PythonValue) -> (Option<()>, PythonValue) {
        let mut receiver = list_value(origin(1), vec![str_item(origin(2), "seed")]);
        let list = match &mut receiver.kind {
            PythonValueKind::List(list) => Some(list),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => None,
        }
        .expect("the receiver should remain a list");
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
        let list = match result.kind {
            PythonValueKind::List(list) => Some(list),
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::Env(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Module(_)
            | PythonValueKind::Unknown(_) => None,
        }
        .expect("extension receiver should remain a list");
        let unknown = list
            .semantic_items()
            .last()
            .and_then(|item| match item {
                PythonSequenceItem::UnknownUnpack(unknown) => Some(unknown),
                PythonSequenceItem::UnknownElement(_) | PythonSequenceItem::Value(_) => None,
            })
            .expect("imprecise source should append an unknown unpack");
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
        assert!(ok.is_none());
        assert_eq!(item_texts(&from_path), vec!["str:seed"]);

        let (ok, from_path_intrinsic) = extend_list(&path_intrinsic_value(
            origin(3),
            PythonPathIntrinsic::PathlibPathType,
        ));
        assert!(ok.is_none());
        assert_eq!(item_texts(&from_path_intrinsic), vec!["str:seed"]);

        let (ok, from_unsupported_literal) = extend_list(&unsupported_literal_value(origin(3)));
        assert!(ok.is_some());
        assert_eq!(
            item_texts(&from_unsupported_literal),
            vec!["str:seed", "unpack"]
        );

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
