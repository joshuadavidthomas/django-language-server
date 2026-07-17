use djls_source::Origin;

use super::BranchConstraints;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ReachableAllocationSites;
use super::allocation::AllocationSites;

/// A concrete Python `dict` value stored as an ordered write/unpack log rather
/// than a flattened map: unknown unpacks and later exact entries affect lookup
/// authority. Dictionaries own constrained, non-empty allocation sites.
///
/// The ordered log stays private. Read behavior is exposed through the deep
/// [`PythonMapping`] view; transactional mutable traversal and literal
/// construction stay on the concrete type where owning the log is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonDict {
    items: Vec<PythonDictItem>,
    allocation_sites: AllocationSites,
}

impl PythonDict {
    pub(super) fn empty(origin: Origin) -> Self {
        Self {
            items: Vec::new(),
            allocation_sites: AllocationSites::one(origin),
        }
    }

    pub(super) fn allocation_sites(&self) -> &AllocationSites {
        &self.allocation_sites
    }

    /// The read-only behavioral view over this dictionary's ordered log.
    pub(crate) fn mapping(&self) -> PythonMapping<'_> {
        PythonMapping { dict: self }
    }

    /// Append a dictionary unpack (`**value`) to the ordered log. A concrete
    /// dictionary contributes its own ordered entries; every other value
    /// becomes a single typed unknown unpack sourced at the unpack expression.
    pub(super) fn extend_from_unpack(&mut self, unpacked: PythonValue, unpack_origin: Origin) {
        match unpacked.kind {
            PythonValueKind::Dict(unpacked) => self.items.extend(unpacked.items),
            PythonValueKind::Unknown(unknown) => {
                self.items.push(PythonDictItem::UnknownUnpack(unknown));
            }
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::List(_)
            | PythonValueKind::Tuple(_) => {
                self.items
                    .push(PythonDictItem::UnknownUnpack(PythonUnknown::new(
                        PythonUnknownCause::UnsupportedExpression,
                        [unpack_origin],
                    )));
            }
        }
    }

    /// Append one complete key/value entry to the ordered log. Both the key and
    /// value alternatives are fully evaluated before the entry is constructed,
    /// so the log never holds a placeholder value.
    pub(super) fn append_entry(&mut self, key: PythonValue, value: PythonValue) {
        self.items
            .push(PythonDictItem::Entry(Box::new(PythonDictEntry {
                key,
                value,
            })));
    }

    /// Move every entry of `other` onto this dictionary's ordered log, used to
    /// fold a correlated single-entry dictionary produced during construction
    /// into the accumulating dictionary.
    pub(super) fn append_entries_from(&mut self, other: Self) {
        self.items.extend(other.items);
    }

    /// Reach the value bound to an exact string key for transactional mutation,
    /// applying `apply` to it. The traversal is rejected (returns `false`
    /// without invoking `apply`) when a later unknown unpack or non-string key
    /// might shadow the selected entry, because such a write could redirect the
    /// runtime target away from the entry chosen here.
    pub(super) fn try_exact_string_value_mut(
        &mut self,
        wanted: &str,
        apply: impl FnOnce(&mut PythonValue) -> bool,
    ) -> bool {
        let mut selected = None;
        for item in self.items.iter_mut().rev() {
            match item {
                PythonDictItem::Entry(entry) => match &entry.key.kind {
                    PythonValueKind::Str(key) if key == wanted => {
                        selected = Some(&mut entry.value);
                        break;
                    }
                    PythonValueKind::Str(_) => {}
                    PythonValueKind::Unknown(_)
                    | PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::List(_)
                    | PythonValueKind::Tuple(_)
                    | PythonValueKind::Dict(_) => return false,
                },
                PythonDictItem::UnknownUnpack(_) => return false,
            }
        }
        match selected {
            Some(value) => apply(value),
            None => false,
        }
    }

    pub(super) fn constrain_value_evidence(&mut self, constraints: &BranchConstraints) {
        for item in &mut self.items {
            if let PythonDictItem::Entry(entry) = item {
                entry.key.constrain_value_evidence(constraints);
                entry.value.constrain_value_evidence(constraints);
            }
        }
        self.allocation_sites.constrain(constraints);
    }

    pub(super) fn normalize(&mut self) {
        for item in &mut self.items {
            if let PythonDictItem::Entry(entry) = item {
                entry.key.normalize();
                entry.value.normalize();
            }
        }
    }

    pub(super) fn same_semantic_value(&self, other: &Self) -> bool {
        self.items.len() == other.items.len()
            && self
                .items
                .iter()
                .zip(&other.items)
                .all(|(left, right)| match (left, right) {
                    (PythonDictItem::Entry(left), PythonDictItem::Entry(right)) => {
                        left.key.same_semantic_value(&right.key)
                            && left.value.same_semantic_value(&right.value)
                    }
                    (PythonDictItem::UnknownUnpack(left), PythonDictItem::UnknownUnpack(right)) => {
                        left.cause == right.cause
                    }
                    (PythonDictItem::Entry(_) | PythonDictItem::UnknownUnpack(_), _) => false,
                })
    }

    pub(super) fn merge_semantically_equal(
        &mut self,
        incoming: Self,
        operation_origin: Option<Origin>,
    ) {
        debug_assert!(self.same_semantic_value(&incoming));
        for (existing, incoming) in self.items.iter_mut().zip(incoming.items) {
            match (existing, incoming) {
                (PythonDictItem::Entry(existing), PythonDictItem::Entry(incoming)) => {
                    existing
                        .key
                        .merge_semantically_equal(incoming.key, operation_origin);
                    existing
                        .value
                        .merge_semantically_equal(incoming.value, operation_origin);
                }
                (
                    PythonDictItem::UnknownUnpack(existing),
                    PythonDictItem::UnknownUnpack(incoming),
                ) => {
                    existing.merge_origins(&incoming);
                }
                _ => unreachable!("semantic equality requires matching dictionary item variants"),
            }
        }
        self.allocation_sites.merge(incoming.allocation_sites);
    }
}

/// A deep, read-only behavioral view over a [`PythonDict`]'s ordered log. It
/// resolves Python mapping semantics (exact-key authority, unknown-write
/// shadowing, deterministic source order) so consumers translate neutral
/// domain evidence instead of reimplementing write authority.
#[derive(Clone, Copy)]
pub(crate) struct PythonMapping<'a> {
    dict: &'a PythonDict,
}

impl<'a> PythonMapping<'a> {
    /// The value bound to `wanted` by the latest exact string-key write, plus
    /// the unknown overrides recorded *after* that write. Overrides before the
    /// latest exact write are discarded because the exact write authoritatively
    /// establishes the value; only later unknown writes can still shadow it.
    pub(crate) fn lookup_string_key(self, wanted: &str) -> MappingLookup<'a> {
        let mut value = None;
        let mut overrides = Vec::new();
        for item in &self.dict.items {
            match item {
                PythonDictItem::Entry(entry) => match &entry.key.kind {
                    PythonValueKind::Str(key) if key == wanted => {
                        value = Some(&entry.value);
                        overrides.clear();
                    }
                    PythonValueKind::Unknown(_) => {
                        overrides.push(MappingOverride::UnknownKey(&entry.key));
                    }
                    PythonValueKind::Str(_)
                    | PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::List(_)
                    | PythonValueKind::Tuple(_)
                    | PythonValueKind::Dict(_) => {}
                },
                PythonDictItem::UnknownUnpack(unknown) => {
                    overrides.push(MappingOverride::UnknownUnpack(unknown));
                }
            }
        }
        MappingLookup { value, overrides }
    }

    /// The effective string-keyed entries in source order, resolving last-write
    /// authority: a string key is emitted only for its latest write and only
    /// when no later unknown key or unknown unpack could shadow it. Unknown and
    /// non-string keys and unknown unpacks are surfaced in source order as
    /// evidence so consumers can report them.
    pub(crate) fn effective_string_entries(self) -> Vec<MappingStringEntry<'a>> {
        let mut reversed = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut shadowed_by_unknown = false;
        for item in self.dict.items.iter().rev() {
            match item {
                PythonDictItem::Entry(entry) => match &entry.key.kind {
                    PythonValueKind::Str(alias) => {
                        // Always record the alias so earlier duplicate writes
                        // are suppressed, but emit only the latest unshadowed
                        // write.
                        if seen.insert(alias.clone()) && !shadowed_by_unknown {
                            reversed.push(MappingStringEntry::Value {
                                key: alias,
                                value: &entry.value,
                            });
                        }
                    }
                    PythonValueKind::Unknown(_) => {
                        shadowed_by_unknown = true;
                        reversed.push(MappingStringEntry::UnknownKey(&entry.key));
                    }
                    PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::List(_)
                    | PythonValueKind::Tuple(_)
                    | PythonValueKind::Dict(_) => {
                        reversed.push(MappingStringEntry::InvalidKey(&entry.key));
                    }
                },
                PythonDictItem::UnknownUnpack(unknown) => {
                    shadowed_by_unknown = true;
                    reversed.push(MappingStringEntry::UnknownUnpack(unknown));
                }
            }
        }
        reversed.reverse();
        reversed
    }

    /// The values that a write to `wanted` might target, conservatively: the
    /// latest exact string-key value (after which no earlier write can be the
    /// target) plus every earlier value whose non-string key could equal
    /// `wanted` at runtime. Yielded latest-write first, matching traversal.
    pub(crate) fn possible_string_values(self, wanted: &str) -> Vec<&'a PythonValue> {
        let mut values = Vec::new();
        for item in self.dict.items.iter().rev() {
            let PythonDictItem::Entry(entry) = item else {
                continue;
            };
            match &entry.key.kind {
                PythonValueKind::Str(key) if key == wanted => {
                    values.push(&entry.value);
                    return values;
                }
                PythonValueKind::Str(_) => {}
                PythonValueKind::Unknown(_)
                | PythonValueKind::Bool(_)
                | PythonValueKind::Path(_)
                | PythonValueKind::List(_)
                | PythonValueKind::Tuple(_)
                | PythonValueKind::Dict(_) => values.push(&entry.value),
            }
        }
        values
    }

    /// The ordered structural projection of the log: entries with their key and
    /// value plus unknown unpacks. Used for recursive key/value/unpack
    /// traversal and by the test adapter; it exposes shape, never mutation.
    pub(crate) fn projection(self) -> impl Iterator<Item = MappingProjection<'a>> {
        self.dict.items.iter().map(|item| match item {
            PythonDictItem::Entry(entry) => MappingProjection::Entry {
                key: &entry.key,
                value: &entry.value,
            },
            PythonDictItem::UnknownUnpack(unknown) => MappingProjection::UnknownUnpack(unknown),
        })
    }

    /// Recurse into every entry's key and value, collecting reachable
    /// allocation-site groups. Owner-held so the private log never leaks.
    pub(super) fn collect_reachable_sites(self, sites: &mut ReachableAllocationSites) {
        for item in &self.dict.items {
            if let PythonDictItem::Entry(entry) = item {
                entry.key.collect_reachable_sites(sites);
                entry.value.collect_reachable_sites(sites);
            }
        }
    }

    /// Count how many times `wanted` is reached through this dictionary's
    /// entries, preserving repeated occurrences.
    pub(super) fn allocation_site_occurrences(self, wanted: &ReachableAllocationSites) -> usize {
        self.dict
            .items
            .iter()
            .filter_map(|item| match item {
                PythonDictItem::Entry(entry) => Some(
                    entry.key.allocation_site_occurrences(wanted)
                        + entry.value.allocation_site_occurrences(wanted),
                ),
                PythonDictItem::UnknownUnpack(_) => None,
            })
            .sum()
    }

    /// Whether `wanted` appears as provenance anywhere in this dictionary's
    /// entries or unknown unpacks.
    pub(super) fn contains_origin(self, wanted: Origin) -> bool {
        self.dict.items.iter().any(|item| match item {
            PythonDictItem::Entry(entry) => {
                entry.key.contains_origin(wanted) || entry.value.contains_origin(wanted)
            }
            PythonDictItem::UnknownUnpack(unknown) => unknown.contains_origin(wanted),
        })
    }

    /// The typed unknown-unpack contributed by iterating this dictionary's
    /// keys. Precise key iteration is out of scope, so even a known empty
    /// mapping remains an imprecise iterable at this abstraction boundary.
    pub(super) fn keys_iteration_unknown(self) -> PythonUnknown {
        PythonUnknown::new(
            PythonUnknownCause::UnsupportedExpression,
            self.dict.allocation_sites.origins(),
        )
    }
}

/// The neutral result of an exact string-key lookup: the resolved value and any
/// unknown writes recorded after the latest exact write.
pub(crate) struct MappingLookup<'a> {
    value: Option<&'a PythonValue>,
    overrides: Vec<MappingOverride<'a>>,
}

impl<'a> MappingLookup<'a> {
    pub(crate) fn value(&self) -> Option<&'a PythonValue> {
        self.value
    }

    pub(crate) fn overrides(&self) -> &[MappingOverride<'a>] {
        &self.overrides
    }
}

/// A later write that may shadow an exact key lookup.
pub(crate) enum MappingOverride<'a> {
    UnknownUnpack(&'a PythonUnknown),
    UnknownKey(&'a PythonValue),
}

/// An entry surfaced by [`PythonMapping::effective_string_entries`].
pub(crate) enum MappingStringEntry<'a> {
    /// The latest unshadowed write for a string key.
    Value {
        key: &'a str,
        value: &'a PythonValue,
    },
    /// A key whose value is an unknown, so the key itself is unknown.
    UnknownKey(&'a PythonValue),
    /// A key that is neither a string nor unknown.
    InvalidKey(&'a PythonValue),
    /// A `**` unpack whose contents are unknown.
    UnknownUnpack(&'a PythonUnknown),
}

/// A structural item in the ordered log, exposed for recursive traversal and
/// the test adapter.
pub(crate) enum MappingProjection<'a> {
    Entry {
        key: &'a PythonValue,
        value: &'a PythonValue,
    },
    UnknownUnpack(&'a PythonUnknown),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PythonDictItem {
    Entry(Box<PythonDictEntry>),
    UnknownUnpack(PythonUnknown),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonDictEntry {
    key: PythonValue,
    value: PythonValue,
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::MappingOverride;
    use super::MappingProjection;
    use super::MappingStringEntry;
    use super::Origin;
    use super::PythonDict;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use super::PythonValueKind;

    fn origin(offset: usize) -> Origin {
        let file = File::from_id(Id::from_bits(1));
        Origin::new(file, Span::saturating_from_parts_usize(offset, 1))
    }

    fn str_value(text: &str, offset: usize) -> PythonValue {
        PythonValue::string(text.to_string(), origin(offset))
    }

    fn dict_with(entries: Vec<(PythonValue, PythonValue)>) -> PythonDict {
        let mut dict = PythonDict::empty(origin(0));
        for (key, value) in entries {
            dict.append_entry(key, value);
        }
        dict
    }

    fn unknown(offset: usize) -> PythonValue {
        PythonValue::unknown(
            PythonUnknownCause::UnsupportedExpression,
            Some(origin(offset)),
        )
    }

    #[test]
    fn lookup_retains_only_later_overrides_after_latest_exact_write() {
        let mut dict = dict_with(vec![
            (str_value("A", 1), str_value("first", 2)),
            (str_value("A", 3), str_value("second", 4)),
        ]);
        // A later unknown unpack shadows the latest exact write.
        dict.extend_from_unpack(unknown(5), origin(5));

        let mapping = dict.mapping();
        let lookup = mapping.lookup_string_key("A");
        assert!(matches!(
            lookup.value().map(|value| &value.kind),
            Some(PythonValueKind::Str(text)) if text == "second"
        ));
        assert_eq!(lookup.overrides().len(), 1);
        assert!(matches!(
            lookup.overrides()[0],
            MappingOverride::UnknownUnpack(_)
        ));
    }

    #[test]
    fn lookup_clears_overrides_recorded_before_the_exact_write() {
        let mut dict = PythonDict::empty(origin(0));
        dict.extend_from_unpack(unknown(1), origin(1));
        dict.append_entry(str_value("A", 2), str_value("value", 3));

        let mapping = dict.mapping();
        let lookup = mapping.lookup_string_key("A");
        assert!(lookup.value().is_some());
        assert!(lookup.overrides().is_empty());
    }

    #[test]
    fn effective_string_entries_prefer_last_write_and_respect_unknown_shadowing() {
        let mut dict = dict_with(vec![
            (str_value("shadowed", 1), str_value("early", 2)),
            (str_value("dup", 3), str_value("old", 4)),
            (str_value("dup", 5), str_value("new", 6)),
        ]);
        dict.extend_from_unpack(unknown(7), origin(7));
        dict.append_entry(str_value("late", 8), str_value("kept", 9));

        let mapping = dict.mapping();
        let entries = mapping.effective_string_entries();
        // "shadowed" and both "dup" writes are behind the unknown unpack, so
        // only the unpack evidence and the post-unpack "late" entry survive.
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], MappingStringEntry::UnknownUnpack(_)));
        assert!(matches!(
            entries[1],
            MappingStringEntry::Value { key, .. } if key == "late"
        ));
    }

    #[test]
    fn effective_string_entries_dedup_prefers_latest_write() {
        let dict = dict_with(vec![
            (str_value("k", 1), str_value("old", 2)),
            (str_value("k", 3), str_value("new", 4)),
        ]);
        let mapping = dict.mapping();
        let entries = mapping.effective_string_entries();
        assert_eq!(entries.len(), 1);
        let MappingStringEntry::Value { key, value } = &entries[0] else {
            panic!("expected a resolved string entry");
        };
        assert_eq!(*key, "k");
        assert!(matches!(&value.kind, PythonValueKind::Str(text) if text == "new"));
    }

    #[test]
    fn possible_values_stop_at_exact_match_but_keep_earlier_uncertain_keys() {
        let dict = dict_with(vec![
            (str_value("target", 1), str_value("exact", 2)),
            (unknown(3), str_value("maybe", 4)),
            (str_value("other", 5), str_value("skip", 6)),
        ]);
        let mapping = dict.mapping();
        let values = mapping.possible_string_values("target");
        // Reverse traversal: "other" is a mismatched string (skipped), the
        // unknown key is a possible target, then the exact "target" match stops
        // traversal.
        assert_eq!(values.len(), 2);
        assert!(matches!(&values[0].kind, PythonValueKind::Str(text) if text == "maybe"));
        assert!(matches!(&values[1].kind, PythonValueKind::Str(text) if text == "exact"));
    }

    #[test]
    fn exact_mutable_traversal_rejects_shadowing_writes_transactionally() {
        let mut unpack_shadowed = dict_with(vec![(str_value("k", 1), str_value("v", 2))]);
        unpack_shadowed.extend_from_unpack(unknown(3), origin(3));
        let unpack_applied = std::cell::Cell::new(false);
        assert!(!unpack_shadowed.try_exact_string_value_mut("k", |_| {
            unpack_applied.set(true);
            true
        }));
        assert!(
            !unpack_applied.get(),
            "a later unknown unpack must reject before applying the mutation"
        );

        let mut key_shadowed = dict_with(vec![
            (str_value("k", 1), str_value("v", 2)),
            (unknown(3), str_value("maybe", 4)),
        ]);
        let key_applied = std::cell::Cell::new(false);
        assert!(!key_shadowed.try_exact_string_value_mut("k", |_| {
            key_applied.set(true);
            true
        }));
        assert!(
            !key_applied.get(),
            "a later uncertain key must reject before applying the mutation"
        );

        let mut clean = dict_with(vec![(str_value("k", 1), str_value("v", 2))]);
        assert!(clean.try_exact_string_value_mut("k", |value| {
            matches!(&value.kind, PythonValueKind::Str(text) if text == "v")
        }));
    }

    #[test]
    fn canonical_unknown_origins_merge_unpacks_and_mapping_key_iteration() {
        let mut first = PythonDict::empty(origin(20));
        first.extend_from_unpack(unknown(40), origin(40));
        let mut second = PythonDict::empty(origin(10));
        second.extend_from_unpack(unknown(30), origin(30));

        first.merge_semantically_equal(second, None);
        let projection = first.mapping().projection().collect::<Vec<_>>();
        let [MappingProjection::UnknownUnpack(unknown)] = projection.as_slice() else {
            panic!("equal dictionaries should retain one unknown unpack");
        };
        assert_eq!(
            unknown.origins().collect::<Vec<_>>(),
            [origin(30), origin(40)]
        );
        assert_eq!(
            first
                .mapping()
                .keys_iteration_unknown()
                .origins()
                .collect::<Vec<_>>(),
            [origin(10), origin(20)],
        );
    }

    #[test]
    fn projection_preserves_source_order() {
        let mut dict = dict_with(vec![(str_value("a", 1), str_value("1", 2))]);
        dict.extend_from_unpack(unknown(3), origin(3));
        dict.append_entry(str_value("b", 4), str_value("2", 5));

        let mapping = dict.mapping();
        let projection: Vec<_> = mapping.projection().collect();
        assert_eq!(projection.len(), 3);
        assert!(matches!(
            projection[0],
            MappingProjection::Entry { key, .. }
                if matches!(&key.kind, PythonValueKind::Str(text) if text == "a")
        ));
        assert!(matches!(projection[1], MappingProjection::UnknownUnpack(_)));
        assert!(matches!(
            projection[2],
            MappingProjection::Entry { key, .. }
                if matches!(&key.kind, PythonValueKind::Str(text) if text == "b")
        ));
    }
}
