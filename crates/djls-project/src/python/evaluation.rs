use std::cmp::Ordering;

use djls_source::File;
use djls_source::FileReadError;
use djls_source::Origin;
use salsa::plumbing::AsId as _;

mod allocation;
mod binding;
mod constraints;
mod evaluator;
mod mapping;
mod module_object;
mod mutation;
mod name_analysis;
mod query;
mod result;
mod sequence;
mod touched_names;
mod truthiness;
mod unique_vec;
mod value;

pub(crate) use self::allocation::ReachableAllocationSites;
pub(crate) use self::binding::PythonBinding;
pub(crate) use self::binding::PythonBindingState;
pub(crate) use self::binding::PythonBoundValue;
pub(crate) use self::constraints::BranchConstraints;
use self::constraints::BranchJoin;
pub(crate) use self::mapping::MappingEntryEvidence;
pub(crate) use self::mapping::MappingLogItem;
pub(crate) use self::mapping::MappingOverride;
pub(crate) use self::mapping::PythonDict;
pub(crate) use self::mapping::PythonMapping;
pub(crate) use self::module_object::ChildImportFallback;
pub(crate) use self::module_object::PythonModuleEffects;
pub(crate) use self::mutation::PythonMutation;
pub(crate) use self::mutation::PythonMutationOperation;
pub(crate) use self::mutation::PythonMutationPath;
pub(crate) use self::mutation::PythonMutationPathSegment;
pub(crate) use self::query::python_import_trace;
pub(crate) use self::query::python_module_facts;
pub(crate) use self::result::PythonImportEdge;
pub(crate) use self::result::PythonImportEvaluationStatus;
pub(crate) use self::result::PythonImportOutcome;
pub(crate) use self::result::PythonImportTrace;
pub(crate) use self::result::PythonModuleFacts;
pub(crate) use self::result::PythonNamespaceCause;
pub(crate) use self::result::PythonNamespaceRemainder;
pub(crate) use self::result::PythonSyntaxErrorImpact;
pub(crate) use self::sequence::PythonList;
pub(crate) use self::sequence::PythonMaterializedSequence;
pub(crate) use self::sequence::PythonSequence;
pub(crate) use self::sequence::PythonSequenceAlternativeRef;
pub(crate) use self::sequence::PythonSequenceItem;
pub(crate) use self::sequence::PythonTuple;
pub(crate) use self::unique_vec::UniqueVec;
pub(crate) use self::value::PythonUnknown;
pub(crate) use self::value::PythonUnknownCause;
pub(crate) use self::value::PythonValue;
pub(crate) use self::value::PythonValueKind;

const MAX_EXACT_PYTHON_ALTERNATIVES: usize = 64;

/// A unique set of origins with deterministic structural iteration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OriginSet(Vec<Origin>);

impl OriginSet {
    fn insert(&mut self, origin: Origin) {
        if self.0.contains(&origin) {
            return;
        }
        self.0.push(origin);
        self.0.sort_by(StructuralOrd::structural_cmp);
    }

    fn extend(&mut self, origins: impl IntoIterator<Item = Origin>) {
        for origin in origins {
            self.insert(origin);
        }
    }

    fn replace(&mut self, origins: impl IntoIterator<Item = Origin>) {
        *self = origins.into_iter().collect();
    }

    fn first(&self) -> Option<Origin> {
        self.0.first().copied()
    }

    fn iter(&self) -> impl ExactSizeIterator<Item = Origin> + '_ {
        self.0.iter().copied()
    }

    fn as_slice(&self) -> &[Origin] {
        &self.0
    }

    fn contains(&self, origin: Origin) -> bool {
        self.0.contains(&origin)
    }
}

impl StructuralOrd for OriginSet {
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

impl FromIterator<Origin> for OriginSet {
    fn from_iter<T: IntoIterator<Item = Origin>>(iter: T) -> Self {
        let mut origins = Self::default();
        origins.extend(iter);
        origins
    }
}

/// PythonModuleEvaluator-owned total ordering for deterministic normalization and retention.
///
/// Implementations compare every field participating in structural equality.
/// Context-dependent policies, such as root-first dependency order, stay with
/// their owning type instead of becoming part of this intrinsic order.
pub(crate) trait StructuralOrd {
    fn structural_cmp(&self, other: &Self) -> Ordering;
}

impl<T: StructuralOrd> StructuralOrd for [T] {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        for (left, right) in self.iter().zip(other) {
            let ordering = left.structural_cmp(right);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.len().cmp(&other.len())
    }
}

impl StructuralOrd for File {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.as_id().cmp(&other.as_id())
    }
}

impl StructuralOrd for Origin {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.file
            .structural_cmp(&other.file)
            .then_with(|| self.span.start().cmp(&other.span.start()))
            .then_with(|| self.span.length().cmp(&other.span.length()))
    }
}

impl StructuralOrd for FileReadError {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.path()
            .cmp(other.path())
            .then_with(|| self.kind().cmp(&other.kind()))
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use djls_source::File;
    use djls_source::Span;
    use salsa::Id;
    use salsa::plumbing::AsId as _;
    use salsa::plumbing::FromId as _;

    use super::Origin;
    use super::OriginSet;
    use super::StructuralOrd as _;

    fn file(index: u32) -> File {
        // SAFETY: Test indexes are below `salsa::Id::MAX_U32`; these synthetic
        // files are compared only as opaque IDs and are never read.
        File::from_id(unsafe { Id::from_index(index) })
    }

    fn origin(file_index: u32, start: u32, length: u32) -> Origin {
        Origin::new(file(file_index), Span::new(start, length))
    }

    #[test]
    fn typed_provenance_order_uses_typed_salsa_file_identity() {
        // Salsa renders IDs in unpadded hexadecimal, so 0x10 sorts lexically
        // before 0xf. Typed identity must retain the numeric order instead.
        let numerically_first = file(15);
        let numerically_later = file(16);

        assert_eq!(numerically_first.as_id().index(), 15);
        assert_eq!(numerically_later.as_id().index(), 16);
        assert_eq!(
            numerically_first.structural_cmp(&numerically_later),
            Ordering::Less
        );
        assert_eq!(
            numerically_later.structural_cmp(&numerically_first),
            Ordering::Greater
        );
    }

    #[test]
    fn typed_provenance_order_compares_every_origin_field_without_collisions() {
        let first_file = origin(15, 4, 3);
        let later_file = origin(16, 1, 1);
        let earlier_start = origin(15, 3, 9);
        let shorter = origin(15, 4, 2);

        assert_eq!(first_file.structural_cmp(&later_file), Ordering::Less);
        assert_eq!(earlier_start.structural_cmp(&shorter), Ordering::Less);
        assert_eq!(shorter.structural_cmp(&first_file), Ordering::Less);

        for unequal in [later_file, earlier_start, shorter] {
            assert_ne!(first_file.structural_cmp(&unequal), Ordering::Equal);
            assert_ne!(unequal.structural_cmp(&first_file), Ordering::Equal);
        }
        assert_eq!(first_file.structural_cmp(&first_file), Ordering::Equal);
    }

    #[test]
    fn typed_provenance_order_origin_set_is_unique_and_order_independent() {
        let first = origin(15, 1, 1);
        let second = origin(15, 2, 1);

        let empty = OriginSet::default();
        assert!(empty.iter().next().is_none());

        let forward: OriginSet = [second, first, second].into_iter().collect();
        let reversed: OriginSet = [first, second].into_iter().collect();
        assert_eq!(forward, reversed);
        assert_eq!(forward.structural_cmp(&reversed), Ordering::Equal);
        assert_eq!(forward.iter().collect::<Vec<_>>(), [first, second]);
        assert!(forward.contains(first));
        assert!(forward.contains(second));

        let extended: OriginSet = [first, second, origin(16, 0, 1)].into_iter().collect();
        assert_eq!(forward.structural_cmp(&extended), Ordering::Less);
        assert_eq!(extended.structural_cmp(&forward), Ordering::Greater);
    }
}
