mod analysis;
mod blocks;
mod filters;
mod models;
mod registry;
mod signature;
mod types;

#[cfg(test)]
pub(crate) mod testing;

use djls_source::File;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::statement_visitor::walk_stmt;

use crate::extraction::registry::RegistrationInfo;
use crate::extraction::registry::collect_registrations_from_body;
use crate::names::ModulePath;
use crate::parse::parse_python_module;
use crate::specs::analysis::CallContext;
use crate::specs::analysis::calls::AbstractValueKey;
use crate::specs::analysis::calls::extract_return_value;
use crate::specs::analysis::state::AbstractValue;
use crate::specs::analysis::state::Env;
use crate::specs::analysis::statements::process_statements;
pub use crate::specs::models::compute_model_graph;
pub use crate::specs::models::extract::extract_model_graph;
pub use crate::specs::models::graph::ModelGraph;
use crate::specs::registry::ExtractionOutput;
pub use crate::specs::types::ArgumentCountConstraint;
pub use crate::specs::types::AsVar;
pub use crate::specs::types::BlockSpec;
pub use crate::specs::types::BlockSpecs;
pub use crate::specs::types::ChoiceAt;
pub use crate::specs::types::ExtractedDiagnosticConstraint;
pub use crate::specs::types::ExtractedDiagnosticMessage;
pub use crate::specs::types::ExtractedMessageArg;
pub use crate::specs::types::ExtractedMessageTemplate;
pub use crate::specs::types::ExtractionResult;
pub use crate::specs::types::FilterArity;
pub use crate::specs::types::FilterArityMap;
pub use crate::specs::types::KnownOptions;
pub use crate::specs::types::RequiredKeyword;
pub use crate::specs::types::SplitPosition;
pub use crate::specs::types::SymbolKey;
pub use crate::specs::types::TagArgument;
pub use crate::specs::types::TagArgumentKind;
pub use crate::specs::types::TagRule;
pub use crate::specs::types::TagRuleMap;
pub use crate::specs::types::TemplateSymbolKind;

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
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(func) if func.name.as_str() == name => return Some(func),
            Stmt::ClassDef(class) => {
                if let Some(found) = find_function_def(&class.body, name) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
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
    registration_module: ModulePath,
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

/// Extract filter arities from a Python file, cached by Salsa.
///
/// This domain-specific query lets filter argument validation depend only on
/// filter signature extraction.
#[salsa::tracked(returns(ref))]
pub fn extract_filter_arities(
    db: &dyn djls_source::Db,
    file: File,
    registration_module: ModulePath,
) -> FilterArityMap {
    with_parsed_body(db, file, |body| {
        let registration_module = registration_module.into_string();
        let mut filter_arities = FilterArityMap::default();

        for_each_registration(body, &registration_module, |reg, func, key| {
            if let Some(arity) = reg.kind.extract_filter_arity(func) {
                filter_arities.insert(key, arity);
            }
        });

        filter_arities
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
    registration_module: ModulePath,
) -> BlockSpecs {
    with_parsed_body(db, file, |body| {
        let registration_module = registration_module.into_string();
        let mut block_specs = BlockSpecs::default();

        for_each_registration(body, &registration_module, |reg, func, key| {
            if let Some(block_spec) =
                normalize_block_spec(reg.kind.extract_block_spec(func), &key.name)
            {
                block_specs.insert(key, block_spec);
            }
        });

        block_specs
    })
}

/// Extract validation rules from a Python registration module source.
///
/// Parses the source with Ruff's Python parser, walks the AST to find
/// `@register.tag` / `@register.filter` decorators, and extracts validation
/// semantics (argument counts, block structure, option constraints) from the
/// associated compile functions.
///
/// The `module_path` parameter is the dotted Python module path (e.g.,
/// `"django.template.defaulttags"`) used as the `registration_module` field
/// in `SymbolKey`s. Pass an empty string if unknown.
///
/// Returns an `ExtractionResult` mapping each discovered `SymbolKey` to its
/// extracted rules.
#[must_use]
pub fn extract_rules(source: &str, module_path: &str) -> ExtractionResult {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return ExtractionResult::default();
    };
    let module = parsed.into_syntax();
    let mut result = ExtractionResult::default();

    for_each_registration(&module.body, module_path, |reg, func, key| {
        match reg.kind.extract(func) {
            ExtractionOutput::Filter(arity) => {
                result.filter_arities.insert(key, arity);
            }
            ExtractionOutput::Tag { rule, block_spec } => {
                if let Some(rule) = rule {
                    result.tag_rules.insert(key.clone(), rule.into());
                }
                if let Some(block_spec) = normalize_block_spec(block_spec, &key.name) {
                    result.block_specs.insert(key, block_spec);
                }
            }
        }
    });

    result
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

fn for_each_registration(
    body: &[Stmt],
    module_path: &str,
    mut f: impl FnMut(&RegistrationInfo, &StmtFunctionDef, SymbolKey),
) {
    let registrations = collect_registrations_from_body(body);
    let func_defs = collect_func_defs(body);

    for reg in &registrations {
        let Some(func) = reg.func_name.as_deref().and_then(|name| {
            func_defs
                .iter()
                .find(|func| func.name.as_str() == name)
                .copied()
        }) else {
            continue;
        };

        let kind = reg.kind;
        let key = SymbolKey {
            registration_module: module_path.to_string(),
            name: reg.name.clone(),
            kind: kind.symbol_kind(),
        };

        f(reg, func, key);
    }
}

fn normalize_block_spec(block_spec: Option<BlockSpec>, tag_name: &str) -> Option<BlockSpec> {
    block_spec.map(|mut block_spec| {
        if block_spec.end_tag.is_none() {
            block_spec.end_tag = Some(format!("end{tag_name}"));
        }
        block_spec
    })
}

/// Recursively collect all function definitions from a module body.
fn collect_func_defs(body: &[Stmt]) -> Vec<&StmtFunctionDef> {
    let mut visitor = FunctionDefCollector::default();
    visitor.visit_body(body);
    visitor.defs
}

#[derive(Default)]
struct FunctionDefCollector<'a> {
    defs: Vec<&'a StmtFunctionDef>,
}

impl<'a> StatementVisitor<'a> for FunctionDefCollector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(func) => {
                self.defs.push(func);
            }
            Stmt::ClassDef(_) => {
                walk_stmt(self, stmt);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::extraction::registry::RegistrationKind;

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
