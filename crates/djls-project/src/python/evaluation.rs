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
pub(crate) use self::mapping::MappingOverride;
pub(crate) use self::mapping::MappingProjection;
pub(crate) use self::mapping::MappingStringEntry;
pub(crate) use self::mapping::PythonDict;
pub(crate) use self::mapping::PythonMapping;
pub(crate) use self::mutation::PythonMutation;
pub(crate) use self::mutation::PythonMutationOperation;
pub(crate) use self::mutation::PythonMutationPath;
pub(crate) use self::mutation::PythonMutationPathSegment;
pub(crate) use self::query::python_module_dependencies;
pub(crate) use self::query::python_module_values;
pub(crate) use self::result::PythonImportEdge;
pub(crate) use self::result::PythonImportEvaluationStatus;
pub(crate) use self::result::PythonImportOutcome;
pub(crate) use self::result::PythonModuleDependencies;
pub(crate) use self::result::PythonModuleValues;
pub(crate) use self::result::PythonNamespaceCause;
pub(crate) use self::result::PythonNamespaceRemainder;
pub(crate) use self::result::PythonSyntaxImpact;
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CanonicalOrigins(Vec<Origin>);

impl CanonicalOrigins {
    fn insert(&mut self, origin: Origin) {
        if self.0.contains(&origin) {
            return;
        }
        self.0.push(origin);
        self.0.sort_by(StructuralOrder::structural_cmp);
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

    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.0
            .iter()
            .map(StructuralOrder::structural_key)
            .cmp(other.0.iter().map(StructuralOrder::structural_key))
    }
}

impl FromIterator<Origin> for CanonicalOrigins {
    fn from_iter<T: IntoIterator<Item = Origin>>(iter: T) -> Self {
        let mut origins = Self::default();
        origins.extend(iter);
        origins
    }
}

/// Evaluator-owned structural ordering for Salsa-backed source-provenance leaves.
///
/// This remains deliberately narrower than `Ord`: only evaluator
/// structural comparison should depend on process-local Salsa identity ordering.
pub(crate) trait StructuralOrder {
    type Key: Ord;

    fn structural_key(&self) -> Self::Key;

    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.structural_key().cmp(&other.structural_key())
    }
}

impl StructuralOrder for File {
    type Key = salsa::Id;

    fn structural_key(&self) -> Self::Key {
        self.as_id()
    }
}

impl StructuralOrder for Origin {
    type Key = (salsa::Id, u32, u32);

    fn structural_key(&self) -> Self::Key {
        (
            self.file.structural_key(),
            self.span.start(),
            self.span.length(),
        )
    }
}

fn file_read_error_structural_cmp(left: &FileReadError, right: &FileReadError) -> Ordering {
    left.path()
        .cmp(right.path())
        .then_with(|| left.kind().cmp(&right.kind()))
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::AsId as _;
    use salsa::plumbing::FromId as _;

    use super::CanonicalOrigins;
    use super::Origin;
    use super::StructuralOrder as _;

    fn file(index: u32) -> File {
        // SAFETY: Test indexes are below `salsa::Id::MAX_U32`; these synthetic
        // files are compared only as opaque IDs and are never read.
        File::from_id(unsafe { salsa::Id::from_index(index) })
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
    fn typed_provenance_order_canonical_origins_are_unique_and_order_independent() {
        let first = origin(15, 1, 1);
        let second = origin(15, 2, 1);

        let empty = CanonicalOrigins::default();
        assert!(empty.iter().next().is_none());

        let forward: CanonicalOrigins = [second, first, second].into_iter().collect();
        let reversed: CanonicalOrigins = [first, second].into_iter().collect();
        assert_eq!(forward, reversed);
        assert_eq!(forward.structural_cmp(&reversed), Ordering::Equal);
        assert_eq!(forward.iter().collect::<Vec<_>>(), [first, second]);
        assert!(forward.contains(first));
        assert!(forward.contains(second));

        let extended: CanonicalOrigins = [first, second, origin(16, 0, 1)].into_iter().collect();
        assert_eq!(forward.structural_cmp(&extended), Ordering::Less);
        assert_eq!(extended.structural_cmp(&forward), Ordering::Greater);
    }
}
