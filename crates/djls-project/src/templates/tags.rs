mod analysis;
mod blocks;
mod registry;
mod signature;
mod types;

#[cfg(test)]
pub(crate) mod testing;

use std::ops::ControlFlow;

use djls_source::File;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;

use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::python::PythonModuleName;
use crate::python::parse_python_module;
use crate::templates::for_each_registration;
use crate::templates::tags::analysis::AbstractValue;
use crate::templates::tags::analysis::AbstractValueKey;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::Env;
use crate::templates::tags::analysis::extract_return_value;
use crate::templates::tags::analysis::process_statements;
use crate::templates::tags::blocks::EndTagEvidence;
pub use crate::templates::tags::types::ArgumentCountConstraint;
pub use crate::templates::tags::types::AsVar;
pub use crate::templates::tags::types::BlockSpec;
pub use crate::templates::tags::types::BlockSpecs;
pub use crate::templates::tags::types::ChoiceAt;
pub use crate::templates::tags::types::ExtractedDiagnosticConstraint;
pub use crate::templates::tags::types::ExtractedDiagnosticMessage;
pub use crate::templates::tags::types::ExtractedMessageArg;
pub use crate::templates::tags::types::ExtractedMessageTemplate;
pub use crate::templates::tags::types::KnownOptions;
pub use crate::templates::tags::types::RequiredKeyword;
pub use crate::templates::tags::types::SplitPosition;
pub use crate::templates::tags::types::TagArgument;
pub use crate::templates::tags::types::TagArgumentKind;
pub use crate::templates::tags::types::TagRule;
pub use crate::templates::tags::types::TagRuleMap;

/// Interned key for a helper function call.
///
/// Salsa uses interning to deduplicate identical helper calls: same file,
/// same callee name, same abstract argument values produce the same
/// `HelperCall` identity, enabling Salsa's built-in memoization.
#[salsa::interned]
pub(crate) struct HelperCall<'db> {
    pub file: File,
    #[returns(ref)]
    pub callee_name: String,
    #[returns(ref)]
    pub args: Vec<AbstractValueKey>,
}

/// Analyze a helper function call and return its abstract return value.
///
/// This is a Salsa tracked function with cycle recovery: if A calls B
/// which calls A (directly or transitively), the cycle resolves to
/// `AbstractValue::Unknown` instead of panicking.
///
/// Looks up the callee by name in the parsed module's AST, binds
/// parameters to the abstract argument values from `HelperCall`, runs
/// the analyzer on the callee body, and extracts the return
/// value.
#[salsa::tracked(
    cycle_initial=analyze_helper_cycle_initial,
    cycle_fn=analyze_helper_cycle_recover,
)]
pub(crate) fn analyze_helper(db: &dyn djls_source::Db, call: HelperCall<'_>) -> AbstractValue {
    let Some(parsed) = parse_python_module(db, call.file(db)) else {
        return AbstractValue::Unknown;
    };

    let callee_name = call.callee_name(db);
    let args = call.args(db);

    let Some(callee) = find_function_def(parsed.body(db), callee_name) else {
        return AbstractValue::Unknown;
    };

    let mut callee_env = Env::default();
    for (i, param) in callee.parameters.args.iter().enumerate() {
        let value = args
            .get(i)
            .map_or(AbstractValue::Unknown, AbstractValue::from);
        callee_env.set(param.parameter.name.to_string(), value);
    }

    let mut ctx = CallContext {
        db: Some(db),
        file: Some(call.file(db)),
    };

    let _result = process_statements(&callee.body, &mut callee_env, &mut ctx);

    extract_return_value(&callee.body, &mut callee_env)
}

fn analyze_helper_cycle_initial(
    _db: &dyn djls_source::Db,
    _id: salsa::Id,
    _call: HelperCall<'_>,
) -> AbstractValue {
    AbstractValue::Unknown
}

fn analyze_helper_cycle_recover(
    _db: &dyn djls_source::Db,
    _cycle: &salsa::Cycle,
    _last_provisional: &AbstractValue,
    _value: AbstractValue,
    _call: HelperCall<'_>,
) -> AbstractValue {
    AbstractValue::Unknown
}

fn find_function_def<'a>(body: &'a [Stmt], name: &str) -> Option<&'a StmtFunctionDef> {
    let mut found = None;
    walk_stmts(body, Recurse::IntoClasses, |stmt| {
        if let Stmt::FunctionDef(func) = stmt
            && func.name.as_str() == name
        {
            found = Some(func);
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    found
}

/// Extract tag validation rules from a Python file, cached by Salsa.
///
/// This domain-specific query lets tag argument validation depend only on tag
/// rule extraction. Filter-only changes can backdate here and avoid invalidating
/// tag specs.
#[salsa::tracked(returns(ref))]
pub fn extract_tag_rules(
    db: &dyn djls_source::Db,
    file: File,
    registration_module: PythonModuleName,
) -> TagRuleMap {
    with_parsed_body(db, file, |body| {
        let registration_module = registration_module.into_string();
        let mut tag_rules = TagRuleMap::default();

        for_each_registration(body, &registration_module, |reg, func, key| {
            if let Some(rule) = reg.kind.extract_tag_rule(func) {
                tag_rules.insert(key, rule.into());
            }
        });

        tag_rules
    })
}

/// Extract block specs from a Python file, cached by Salsa.
///
/// This domain-specific query lets structural tag validation depend on block
/// extraction without also depending on filter arities.
#[salsa::tracked(returns(ref))]
pub fn extract_block_specs(
    db: &dyn djls_source::Db,
    file: File,
    registration_module: PythonModuleName,
) -> BlockSpecs {
    with_parsed_body(db, file, |body| {
        let registration_module = registration_module.into_string();
        let mut block_specs = BlockSpecs::default();

        for_each_registration(body, &registration_module, |reg, func, key| {
            if let Some(block_spec) = reg.kind.extract_block_spec(func) {
                let end_tag = match block_spec.end_tag {
                    EndTagEvidence::Literal(end_tag) => Some(end_tag),
                    EndTagEvidence::SelfNamed => Some(format!("end{}", key.name)),
                    EndTagEvidence::Unknown => None,
                };
                block_specs.insert(
                    key,
                    BlockSpec {
                        end_tag,
                        intermediates: block_spec.intermediates,
                        opaque: block_spec.opaque,
                    },
                );
            }
        });

        block_specs
    })
}

fn with_parsed_body<M: Default>(
    db: &dyn djls_source::Db,
    file: File,
    f: impl FnOnce(&[Stmt]) -> M,
) -> M {
    let Some(parsed) = parse_python_module(db, file) else {
        return M::default();
    };

    f(parsed.body(db))
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use crate::templates::registrations::RegistrationInfo;
    use crate::templates::registrations::RegistrationKind;
    use crate::templates::registrations::collect_registrations_from_body;

    #[test]
    fn registry_collection_is_reachable_from_python_module() {
        let registrations: Vec<RegistrationInfo> = collect_registrations_from_body(&[]);
        assert!(registrations.is_empty());
        let _ = RegistrationKind::Tag;
    }

    // (d) Pure Rust — tests parser infrastructure works
    #[test]
    fn smoke_test_ruff_parser() {
        let source = r#"
from django import template

register = template.Library()

@register.simple_tag
def hello():
    return "Hello, world!"
"#;

        let result = parse_module(source);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        let module = parsed.into_syntax();
        assert!(!module.body.is_empty());
    }
}
