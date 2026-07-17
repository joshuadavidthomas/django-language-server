use djls_source::Origin;

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
        self.0.sort_by_key(origin_sort_key);
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

impl FromIterator<Origin> for CanonicalOrigins {
    fn from_iter<T: IntoIterator<Item = Origin>>(iter: T) -> Self {
        let mut origins = Self::default();
        origins.extend(iter);
        origins
    }
}

pub(crate) fn origin_sort_key(origin: &Origin) -> (String, u32, u32) {
    (
        format!("{:?}", origin.file),
        origin.span.start(),
        origin.span.length(),
    )
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::CanonicalOrigins;
    use super::Origin;
    use super::origin_sort_key;

    fn origin(file: u32, start: u32) -> Origin {
        // SAFETY: Test indexes are below `salsa::Id::MAX_U32`; these synthetic
        // files are compared only as opaque IDs and are never read.
        let file = File::from_id(unsafe { salsa::Id::from_index(file) });
        Origin::new(file, Span::new(start, 1))
    }

    #[test]
    fn canonical_unknown_origins_are_unique_and_order_independent() {
        let first = origin(0, 1);
        let second = origin(0, 2);

        let empty = CanonicalOrigins::default();
        assert!(empty.iter().next().is_none());

        let forward: CanonicalOrigins = [second, first, second].into_iter().collect();
        let reversed: CanonicalOrigins = [first, second].into_iter().collect();
        assert_eq!(forward, reversed);
        assert_eq!(forward.iter().collect::<Vec<_>>(), [first, second]);
    }

    #[test]
    fn canonical_unknown_origins_use_the_shared_cross_file_debug_order() {
        let numerically_first = origin(2, 1);
        let numerically_later = origin(10, 1);
        assert_ne!(
            origin_sort_key(&numerically_first),
            origin_sort_key(&numerically_later),
            "unequal origins must have unequal canonical keys"
        );

        let origins: CanonicalOrigins =
            [numerically_first, numerically_later].into_iter().collect();
        let mut expected = vec![numerically_first, numerically_later];
        expected.sort_by_key(origin_sort_key);
        assert_eq!(origins.iter().collect::<Vec<_>>(), expected);
        assert!(origins.contains(numerically_first));
        assert!(origins.contains(numerically_later));
    }
}
