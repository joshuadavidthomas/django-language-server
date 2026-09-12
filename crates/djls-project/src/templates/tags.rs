mod analysis;
pub(crate) mod blocks;
mod registry;
mod signature;
mod types;

#[cfg(test)]
pub(crate) mod testing;

use djls_source::File;

use crate::python::PythonFunctionDefinition;
use crate::templates::tags::analysis::AbstractValue;
use crate::templates::tags::analysis::AbstractValueKey;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::Env;
pub(crate) use crate::templates::tags::analysis::TagSourceContext;
use crate::templates::tags::analysis::process_statements;
pub use crate::templates::tags::types::ArgumentCountConstraint;
pub use crate::templates::tags::types::ArgumentFormCoverage;
pub use crate::templates::tags::types::AsVar;
pub use crate::templates::tags::types::AssignmentMode;
pub use crate::templates::tags::types::AssignmentOperand;
pub use crate::templates::tags::types::BlockSpec;
pub use crate::templates::tags::types::BlockSpecs;
pub use crate::templates::tags::types::BodyAnalysisEvidence;
pub use crate::templates::tags::types::ChoiceAt;
pub use crate::templates::tags::types::ExtractedDiagnosticConstraint;
pub use crate::templates::tags::types::ExtractedDiagnosticMessage;
pub use crate::templates::tags::types::ExtractedMessageArg;
pub use crate::templates::tags::types::ExtractedMessageTemplate;
pub use crate::templates::tags::types::FormAtomExpectation;
pub use crate::templates::tags::types::FormAtomMismatch;
pub use crate::templates::tags::types::FormContinuation;
pub use crate::templates::tags::types::KnownOptions;
pub use crate::templates::tags::types::OptionRejection;
pub use crate::templates::tags::types::ParameterRequirement;
pub use crate::templates::tags::types::RemainderPolicy;
pub use crate::templates::tags::types::RequiredKeyword;
pub use crate::templates::tags::types::SplitPosition;
pub use crate::templates::tags::types::TagArgument;
pub use crate::templates::tags::types::TagArgumentForm;
pub use crate::templates::tags::types::TagArgumentFormError;
pub use crate::templates::tags::types::TagArgumentFormMismatch;
pub use crate::templates::tags::types::TagArgumentKind;
pub use crate::templates::tags::types::TagArgumentPattern;
pub use crate::templates::tags::types::TagArgumentPatternKind;
pub use crate::templates::tags::types::TagArgumentSyntax;
pub use crate::templates::tags::types::TagRule;
pub use crate::templates::tags::types::TagRuleMap;
pub use crate::templates::tags::types::UniqueKeyCardinality;

/// Interned key for an exact helper function call.
#[salsa::interned]
pub(crate) struct HelperCall<'db> {
    #[returns(ref)]
    pub definition: PythonFunctionDefinition,
    #[returns(ref)]
    pub args: Vec<AbstractValueKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HelperOutcome {
    pub value: AbstractValue,
    pub consulted_files: Vec<File>,
    pub recovered_lookups: usize,
}

impl HelperOutcome {
    fn unknown() -> Self {
        Self {
            value: AbstractValue::Unknown,
            consulted_files: Vec::new(),
            recovered_lookups: 0,
        }
    }
}

/// Analyze a helper function call and return its abstract return value.
///
/// This is a Salsa tracked function with cycle recovery: if A calls B
/// which calls A (directly or transitively), the cycle resolves to
/// `AbstractValue::Unknown` instead of panicking.
///
/// Looks up the exact definition in the parsed module, binds the abstract
/// arguments, and returns the value with its consulted-source evidence.
#[salsa::tracked(
    returns(clone),
    cycle_initial=analyze_helper_cycle_initial,
    cycle_fn=analyze_helper_cycle_recover,
)]
pub(crate) fn analyze_helper(db: &dyn crate::db::Db, call: HelperCall<'_>) -> HelperOutcome {
    let definition = call.definition(db);
    let Some(callee) = definition.statement(db) else {
        return HelperOutcome::unknown();
    };
    let args = call.args(db);

    let mut callee_env = Env::default();
    analysis::constants::seed_static_bindings(
        analysis::constants::module_static_bindings(db, definition.file()),
        callee,
        &mut callee_env,
    );
    for (i, param) in callee.parameters.args.iter().enumerate() {
        let value = args
            .get(i)
            .map_or(AbstractValue::Unknown, AbstractValue::from);
        callee_env.set(param.parameter.name.to_string(), value);
    }

    let mut source = analysis::TagSourceContext::new(db, definition.clone());
    let mut ctx = CallContext {
        source: Some(&mut source),
    };

    let (_, value) = process_statements(&callee.body, &mut callee_env, &mut ctx);

    HelperOutcome {
        value,
        consulted_files: source.lookup.consulted_files().to_vec(),
        recovered_lookups: source.lookup.recovered_source_lookups(),
    }
}

fn analyze_helper_cycle_initial(
    _db: &dyn crate::db::Db,
    _id: salsa::Id,
    _call: HelperCall<'_>,
) -> HelperOutcome {
    HelperOutcome::unknown()
}

fn analyze_helper_cycle_recover(
    _db: &dyn crate::db::Db,
    _cycle: &salsa::Cycle,
    _last_provisional: &HelperOutcome,
    _value: HelperOutcome,
    _call: HelperCall<'_>,
) -> HelperOutcome {
    HelperOutcome::unknown()
}
