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
