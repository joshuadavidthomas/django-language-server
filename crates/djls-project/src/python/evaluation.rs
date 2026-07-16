use djls_source::Origin;
use rustc_hash::FxHashSet;

mod binding;
mod constraints;
mod evaluator;
mod mutation;
mod name_analysis;
mod query;
mod result;
mod touched_names;
mod truthiness;
mod unique_vec;
mod value;

pub(crate) use self::binding::PythonBinding;
pub(crate) use self::binding::PythonBindingState;
pub(crate) use self::binding::PythonBoundValue;
pub(crate) use self::constraints::BranchConstraints;
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
pub(crate) use self::result::PythonModuleEvaluation;
pub(crate) use self::result::PythonModuleValues;
pub(crate) use self::result::PythonNamespaceCause;
pub(crate) use self::result::PythonNamespaceRemainder;
pub(crate) use self::result::PythonSyntaxImpact;
pub(crate) use self::unique_vec::UniqueVec;
pub(crate) use self::value::PythonDict;
pub(crate) use self::value::PythonDictItem;
pub(crate) use self::value::PythonList;
pub(crate) use self::value::PythonListAlternativeRef;
pub(crate) use self::value::PythonListItem;
pub(crate) use self::value::PythonUnknown;
pub(crate) use self::value::PythonUnknownCause;
pub(crate) use self::value::PythonValue;
pub(crate) use self::value::PythonValueKind;

const MAX_EXACT_PYTHON_ALTERNATIVES: usize = 64;

#[derive(Debug, Default)]
struct MutableOrigins(FxHashSet<Origin>);

impl MutableOrigins {
    fn insert(&mut self, origin: Origin) {
        self.0.insert(origin);
    }

    fn extend(&mut self, origins: impl IntoIterator<Item = Origin>) {
        self.0.extend(origins);
    }

    fn contains(&self, origin: &Origin) -> bool {
        self.0.contains(origin)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn intersects(&self, other: &Self) -> bool {
        self.0.iter().any(|origin| other.contains(origin))
    }

    fn iter(&self) -> impl Iterator<Item = Origin> + '_ {
        self.0.iter().copied()
    }
}

fn origin_sort_key(origin: &Origin) -> (String, u32, u32) {
    (
        format!("{:?}", origin.file),
        origin.span.start(),
        origin.span.length(),
    )
}
