//! Intra-module function call resolution via bounded inlining.

use std::collections::HashMap;

use ruff_python_ast::Stmt;

use super::domain::AbstractValue;
use super::domain::Env;
use super::domain::TokenSplit;
use super::eval::process_statements;
use super::eval::CallContext;

/// Maximum call inlining depth. Beyond this, calls return Unknown.
const MAX_CALL_DEPTH: usize = 2;

/// Cache for helper function analysis results.
///
/// When multiple compile functions in the same module call the same helper,
/// the helper is analyzed once and the result is reused. Keyed by
/// (function name, abstract input values).
pub struct HelperCache {
    summaries: HashMap<HelperCacheKey, AbstractValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HelperCacheKey {
    func_name: String,
    args: Vec<AbstractValueKey>,
}

/// A hashable representation of `AbstractValue` for cache keying.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AbstractValueKey {
    Unknown,
    Token,
    Parser,
    SplitResult(TokenSplit),
    SplitElement(crate::types::SplitPosition),
    SplitLength(TokenSplit),
    Int(i64),
    Str(String),
    Other,
}

impl From<&AbstractValue> for AbstractValueKey {
    fn from(v: &AbstractValue) -> Self {
        match v {
            AbstractValue::Unknown => AbstractValueKey::Unknown,
            AbstractValue::Token => AbstractValueKey::Token,
            AbstractValue::Parser => AbstractValueKey::Parser,
            AbstractValue::SplitResult(split) => AbstractValueKey::SplitResult(*split),
            AbstractValue::SplitElement { index } => AbstractValueKey::SplitElement(*index),
            AbstractValue::SplitLength(split) => AbstractValueKey::SplitLength(*split),
            AbstractValue::Int(n) => AbstractValueKey::Int(*n),
            AbstractValue::Str(s) => AbstractValueKey::Str(s.clone()),
            AbstractValue::Tuple(_) => AbstractValueKey::Other,
        }
    }
}

impl HelperCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            summaries: HashMap::new(),
        }
    }

    fn get(&self, func_name: &str, args: &[AbstractValue]) -> Option<&AbstractValue> {
        let key = Self::make_key(func_name, args);
        self.summaries.get(&key)
    }

    fn insert(&mut self, func_name: &str, args: &[AbstractValue], result: AbstractValue) {
        let key = Self::make_key(func_name, args);
        self.summaries.insert(key, result);
    }

    fn make_key(func_name: &str, args: &[AbstractValue]) -> HelperCacheKey {
        HelperCacheKey {
            func_name: func_name.to_string(),
            args: args.iter().map(AbstractValueKey::from).collect(),
        }
    }

    /// Returns the number of cached entries (for testing).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.summaries.len()
    }
}

impl Default for HelperCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a function call to a module-local helper.
///
/// Finds the callee in `module_funcs`, creates a new environment with
/// the callee's parameters bound to the caller's argument values,
/// analyzes the callee's body, and returns the abstract return value.
///
/// Returns `Unknown` if:
/// - The callee is not found in `module_funcs`
/// - `call_depth >= MAX_CALL_DEPTH`
/// - The callee is the same function as the caller (self-recursion guard)
/// - Multiple return statements yield different abstract values
pub fn resolve_call(
    callee_name: &str,
    args: &[AbstractValue],
    ctx: &mut CallContext<'_>,
) -> AbstractValue {
    // Check cache first
    if let Some(cached) = ctx.cache.get(callee_name, args) {
        return cached.clone();
    }

    // Recursion guards
    if ctx.call_depth >= MAX_CALL_DEPTH {
        return AbstractValue::Unknown;
    }
    if callee_name == ctx.caller_name {
        return AbstractValue::Unknown;
    }

    // Find callee in module functions
    let Some(callee) = ctx
        .module_funcs
        .iter()
        .find(|f| f.name.as_str() == callee_name)
    else {
        return AbstractValue::Unknown;
    };

    // Create env binding callee parameters to caller's argument values
    let mut callee_env = Env::default();
    for (i, param) in callee.parameters.args.iter().enumerate() {
        let value = args.get(i).cloned().unwrap_or(AbstractValue::Unknown);
        callee_env.set(param.parameter.name.to_string(), value);
    }

    // Analyze callee body
    let saved_caller = ctx.caller_name;
    let saved_depth = ctx.call_depth;
    ctx.caller_name = callee.name.as_str();
    ctx.call_depth = saved_depth + 1;

    let _callee_result = process_statements(&callee.body, &mut callee_env, ctx);

    ctx.caller_name = saved_caller;
    ctx.call_depth = saved_depth;

    // Extract return value
    let result = extract_return_value(&callee.body, &callee_env);

    // Cache before returning
    ctx.cache.insert(callee_name, args, result.clone());

    result
}

/// Extract the abstract return value from a function body.
///
/// Scans for `return expr` statements. If exactly one return path yields
/// a non-Unknown value, returns that. If multiple yields differ, returns Unknown.
fn extract_return_value(body: &[Stmt], env: &Env) -> AbstractValue {
    let mut returns = Vec::new();
    collect_returns(body, env, &mut returns);

    if returns.is_empty() {
        return AbstractValue::Unknown;
    }

    // If all returns are the same, use that value
    let first = &returns[0];
    if returns.iter().all(|r| r == first) {
        return first.clone();
    }

    // Filter out Unknown values — if a single non-Unknown value remains, use it
    let non_unknown: Vec<_> = returns
        .iter()
        .filter(|r| !matches!(r, AbstractValue::Unknown))
        .collect();

    if non_unknown.len() == 1 {
        return non_unknown[0].clone();
    }
    if non_unknown.len() > 1 && non_unknown.iter().all(|r| *r == non_unknown[0]) {
        return non_unknown[0].clone();
    }

    AbstractValue::Unknown
}

fn collect_returns(stmts: &[Stmt], env: &Env, returns: &mut Vec<AbstractValue>) {
    for stmt in stmts {
        match stmt {
            Stmt::Return(ret) => {
                let value = ret.value.as_deref().map_or(AbstractValue::Unknown, |expr| {
                    super::eval::eval_expr(expr, env)
                });
                returns.push(value);
            }
            Stmt::If(if_stmt) => {
                collect_returns(&if_stmt.body, env, returns);
                for clause in &if_stmt.elif_else_clauses {
                    collect_returns(&clause.body, env, returns);
                }
            }
            Stmt::For(for_stmt) => {
                collect_returns(&for_stmt.body, env, returns);
                collect_returns(&for_stmt.orelse, env, returns);
            }
            Stmt::While(while_stmt) => {
                collect_returns(&while_stmt.body, env, returns);
                collect_returns(&while_stmt.orelse, env, returns);
            }
            Stmt::Try(try_stmt) => {
                collect_returns(&try_stmt.body, env, returns);
                for handler in &try_stmt.handlers {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                    collect_returns(&h.body, env, returns);
                }
                collect_returns(&try_stmt.orelse, env, returns);
                collect_returns(&try_stmt.finalbody, env, returns);
            }
            Stmt::With(with_stmt) => {
                collect_returns(&with_stmt.body, env, returns);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_ast::StmtFunctionDef;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::test_helpers::corpus_source;
    use crate::types::SplitPosition;

    fn parse_module_funcs(source: &str) -> Vec<StmtFunctionDef> {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        module
            .body
            .into_iter()
            .filter_map(|s| {
                if let Stmt::FunctionDef(f) = s {
                    Some(f)
                } else {
                    None
                }
            })
            .collect()
    }

    fn analyze_with_helpers(source: &str) -> (Env, HelperCache) {
        let funcs = parse_module_funcs(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();

        // Find the main compile function: prefer one named do_* or the last with 2+ params
        let main_func = funcs
            .iter()
            .find(|f| f.name.starts_with("do_"))
            .or_else(|| funcs.iter().rfind(|f| f.parameters.args.len() >= 2))
            .unwrap_or(&funcs[0]);

        let parser_param = main_func
            .parameters
            .args
            .first()
            .map_or("parser", |p| p.parameter.name.as_str());
        let token_param = main_func
            .parameters
            .args
            .get(1)
            .map_or("token", |p| p.parameter.name.as_str());

        let mut env = Env::for_compile_function(parser_param, token_param);
        let mut cache = HelperCache::new();
        let mut ctx = CallContext {
            module_funcs: &func_refs,
            caller_name: main_func.name.as_str(),
            call_depth: 0,
            cache: &mut cache,
        };

        process_statements(&main_func.body, &mut env, &mut ctx);
        (env, cache)
    }

    fn analyze_function_with_helpers(source: &str, func_name: &str) -> (Env, HelperCache) {
        let funcs = parse_module_funcs(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();

        let main_func = funcs
            .iter()
            .find(|f| f.name.as_str() == func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in source"));

        let parser_param = main_func
            .parameters
            .args
            .first()
            .map_or("parser", |p| p.parameter.name.as_str());
        let token_param = main_func
            .parameters
            .args
            .get(1)
            .map_or("token", |p| p.parameter.name.as_str());

        let mut env = Env::for_compile_function(parser_param, token_param);
        let mut cache = HelperCache::new();
        let mut ctx = CallContext {
            module_funcs: &func_refs,
            caller_name: main_func.name.as_str(),
            call_depth: 0,
            cache: &mut cache,
        };

        process_statements(&main_func.body, &mut env, &mut ctx);
        (env, cache)
    }

    #[test]
    fn simple_helper_returns_split_contents() {
        let (env, _) = analyze_with_helpers(
            r"
def helper(tok):
    return tok.split_contents()

def do_tag(parser, token):
    bits = helper(token)
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn tuple_return_destructuring() {
        let (env, _) = analyze_with_helpers(
            r"
def parse_tag(tok, prs):
    bits = tok.split_contents()
    tag_name = bits[0]
    return tag_name, bits[1:], prs

def do_tag(parser, token):
    name, args, p = parse_tag(token, parser)
",
        );
        assert_eq!(
            env.get("name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("args"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
        assert_eq!(env.get("p"), &AbstractValue::Parser);
    }

    #[test]
    fn allauth_parse_tag_pattern() {
        // Corpus: allauth's parse_tag builds args via for-loop with conditional appends.
        // do_element calls parse_tag(token, parser) and destructures the result.
        // The for-loop means args remains Unknown → no false positive constraints.
        let source =
            corpus_source("packages/django-allauth/0.63.3/allauth/templatetags/allauth.py");
        let Some(source) = source else {
            eprintln!("skipping allauth_parse_tag_pattern: corpus not synced");
            return;
        };
        let (env, _) = analyze_function_with_helpers(&source, "do_element");
        // tag_name should be SplitElement(Forward(0)) — parse_tag does bits.pop(0)
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        // args and kwargs should be Unknown (built from list/dict operations in parse_tag)
        assert_eq!(env.get("args"), &AbstractValue::Unknown);
        assert_eq!(env.get("kwargs"), &AbstractValue::Unknown);
    }

    #[test]
    fn depth_limit() {
        let (env, _) = analyze_with_helpers(
            r"
def deep3(tok):
    return tok.split_contents()

def deep2(tok):
    return deep3(tok)

def deep1(tok):
    return deep2(tok)

def do_tag(parser, token):
    bits = deep1(token)
",
        );
        // depth 0 → deep1, depth 1 → deep2, depth 2 → deep3 (at limit, returns Unknown)
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
    }

    #[test]
    fn self_recursion() {
        let (env, _) = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = do_tag(parser, token)
",
        );
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
    }

    #[test]
    fn helper_not_found() {
        let (env, _) = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = nonexistent_helper(token)
",
        );
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
    }

    #[test]
    fn token_kwargs_marks_unknown() {
        let (env, _) = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    result = token_kwargs(bits, parser)
",
        );
        // After token_kwargs, bits should be Unknown
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
        assert_eq!(env.get("result"), &AbstractValue::Unknown);
    }

    #[test]
    fn parser_compile_filter() {
        let (env, _) = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    val = parser.compile_filter(bits[1])
",
        );
        assert_eq!(env.get("val"), &AbstractValue::Unknown);
    }

    #[test]
    fn cache_hit_same_args() {
        let (_, cache) = analyze_with_helpers(
            r"
def helper(tok):
    return tok.split_contents()

def do_tag(parser, token):
    a = helper(token)
    b = helper(token)
",
        );
        // Helper called twice with same args → cached, only 1 entry
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_miss_different_args() {
        let source = r"
def helper(x):
    return x

def do_tag(parser, token):
    a = helper(token)
    b = helper(parser)
";
        let funcs = parse_module_funcs(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();

        let main_func = funcs.iter().find(|f| f.name.as_str() == "do_tag").unwrap();

        let mut env = Env::for_compile_function("parser", "token");
        let mut cache = HelperCache::new();
        let mut ctx = CallContext {
            module_funcs: &func_refs,
            caller_name: "do_tag",
            call_depth: 0,
            cache: &mut cache,
        };

        process_statements(&main_func.body, &mut env, &mut ctx);

        // Different arg values → 2 cache entries
        assert_eq!(cache.len(), 2);
        assert_eq!(env.get("a"), &AbstractValue::Token);
        assert_eq!(env.get("b"), &AbstractValue::Parser);
    }

    #[test]
    fn helper_with_pop_and_return() {
        let (env, _) = analyze_with_helpers(
            r"
def get_bits(tok):
    bits = tok.split_contents()
    bits.pop(0)
    return bits

def do_tag(parser, token):
    remaining = get_bits(token)
",
        );
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_front())
        );
    }

    #[test]
    fn helper_call_in_tuple_element() {
        // Verifies that ctx is propagated through tuple element evaluation,
        // allowing module-local helper calls inside tuple literals to resolve.
        let (env, _) = analyze_with_helpers(
            r"
def get_bits(tok):
    return tok.split_contents()

def do_tag(parser, token):
    pair = (get_bits(token), 42)
",
        );
        // The tuple should contain the resolved SplitResult from get_bits
        assert_eq!(
            env.get("pair"),
            &AbstractValue::Tuple(vec![
                AbstractValue::SplitResult(TokenSplit::fresh()),
                AbstractValue::Int(42),
            ])
        );
    }

    #[test]
    fn helper_call_in_subscript_base() {
        // Verifies that ctx is propagated through subscript base evaluation,
        // allowing module-local helper calls as the subscript target to resolve.
        let (env, _) = analyze_with_helpers(
            r"
def get_bits(tok):
    return tok.split_contents()

def do_tag(parser, token):
    first = get_bits(token)[0]
",
        );
        assert_eq!(
            env.get("first"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0),
            }
        );
    }

    #[test]
    fn multiple_helper_calls_in_tuple() {
        // Ensures ctx reborrowing works correctly across multiple tuple elements
        // that each need call resolution.
        let (env, cache) = analyze_with_helpers(
            r"
def get_bits(tok):
    return tok.split_contents()

def identity(x):
    return x

def do_tag(parser, token):
    triple = (get_bits(token), identity(parser), identity(token))
",
        );
        assert_eq!(
            env.get("triple"),
            &AbstractValue::Tuple(vec![
                AbstractValue::SplitResult(TokenSplit::fresh()),
                AbstractValue::Parser,
                AbstractValue::Token,
            ])
        );
        // identity called with 2 different args → 2 cache entries, plus get_bits → 3 total
        assert_eq!(cache.len(), 3);
    }
}
