//! Intra-module function call resolution via Salsa tracked functions.

use ruff_python_ast::Stmt;

use super::domain::AbstractValue;
use super::domain::Env;
use super::domain::TokenSplit;
use super::eval::CallContext;
use crate::parse::analyze_helper;
use crate::parse::HelperCall;

/// A hashable representation of `AbstractValue` for Salsa interned keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AbstractValueKey {
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

impl From<&AbstractValueKey> for AbstractValue {
    fn from(k: &AbstractValueKey) -> Self {
        match k {
            AbstractValueKey::Unknown | AbstractValueKey::Other => AbstractValue::Unknown,
            AbstractValueKey::Token => AbstractValue::Token,
            AbstractValueKey::Parser => AbstractValue::Parser,
            AbstractValueKey::SplitResult(split) => AbstractValue::SplitResult(*split),
            AbstractValueKey::SplitElement(index) => AbstractValue::SplitElement { index: *index },
            AbstractValueKey::SplitLength(split) => AbstractValue::SplitLength(*split),
            AbstractValueKey::Int(n) => AbstractValue::Int(*n),
            AbstractValueKey::Str(s) => AbstractValue::Str(s.clone()),
        }
    }
}

/// Resolve a function call to a module-local helper.
///
/// When a Salsa database and file are available (`ctx.db` and `ctx.file`
/// are `Some`), constructs a `HelperCall` interned value and delegates to
/// `analyze_helper` — a Salsa tracked function with cycle recovery and
/// automatic memoization. This replaces manual caching, depth limits, and
/// self-recursion guards.
///
/// When running without Salsa (standalone extraction), returns `Unknown`
/// for all helper calls.
///
/// Returns `Unknown` if:
/// - No Salsa database is available (standalone extraction)
/// - The callee is not found in the parsed module
/// - Multiple return statements yield different abstract values
/// - A cycle is detected (Salsa cycle recovery returns `Unknown`)
pub fn resolve_call(
    callee_name: &str,
    args: &[AbstractValue],
    ctx: &mut CallContext<'_>,
) -> AbstractValue {
    // When Salsa is available, use tracked function with cycle recovery
    if let (Some(db), Some(file)) = (ctx.db, ctx.file) {
        let arg_keys: Vec<AbstractValueKey> = args.iter().map(AbstractValueKey::from).collect();
        let call = HelperCall::new(db, file, callee_name.to_string(), arg_keys);
        return analyze_helper(db, call);
    }

    // No Salsa database — cannot resolve helper calls
    AbstractValue::Unknown
}

/// Extract the abstract return value from a function body.
///
/// Scans for `return expr` statements. If exactly one return path yields
/// a non-Unknown value, returns that. If multiple yields differ, returns Unknown.
pub(crate) fn extract_return_value(body: &[Stmt], env: &Env) -> AbstractValue {
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
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use ruff_python_ast::Stmt;
    use ruff_python_ast::StmtFunctionDef;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::dataflow::eval::process_statements;
    use crate::dataflow::eval::CallContext;
    use crate::test_helpers::corpus_source;
    use crate::types::SplitPosition;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        files: Arc<Mutex<std::collections::HashMap<String, String>>>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                files: Arc::new(Mutex::new(std::collections::HashMap::new())),
            }
        }

        fn create_python_file(&self, source: &str) -> djls_source::File {
            let path = "test_module.py";
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), source.to_string());
            djls_source::File::new(self, path.into(), 0)
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_source::Db for TestDatabase {
        fn create_file(&self, path: &Utf8Path) -> djls_source::File {
            djls_source::File::new(self, path.to_owned(), 0)
        }

        fn get_file(&self, _path: &Utf8Path) -> Option<djls_source::File> {
            None
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            self.files
                .lock()
                .unwrap()
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
        }
    }

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

    /// Analyze a module with helper resolution via Salsa.
    fn analyze_with_helpers(source: &str) -> Env {
        let db = TestDatabase::new();
        let file = db.create_python_file(source);

        let funcs = parse_module_funcs(source);

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
        let mut ctx = CallContext {
            db: Some(&db),
            file: Some(file),
        };

        process_statements(&main_func.body, &mut env, &mut ctx);
        env
    }

    /// Analyze a specific function with helper resolution via Salsa.
    fn analyze_function_with_helpers(source: &str, func_name: &str) -> Env {
        let db = TestDatabase::new();
        let file = db.create_python_file(source);

        let funcs = parse_module_funcs(source);

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
        let mut ctx = CallContext {
            db: Some(&db),
            file: Some(file),
        };

        process_statements(&main_func.body, &mut env, &mut ctx);
        env
    }

    #[test]
    fn simple_helper_returns_split_contents() {
        let env = analyze_with_helpers(
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
        let env = analyze_with_helpers(
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
        let source =
            corpus_source("packages/django-allauth/0.63.3/allauth/templatetags/allauth.py");
        let Some(source) = source else {
            eprintln!("skipping allauth_parse_tag_pattern: corpus not synced");
            return;
        };
        let env = analyze_function_with_helpers(&source, "do_element");
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(env.get("args"), &AbstractValue::Unknown);
        assert_eq!(env.get("kwargs"), &AbstractValue::Unknown);
    }

    #[test]
    fn deep_call_chain_returns_unknown() {
        // Deep chains (A calls B calls C) return Unknown because
        // `extract_return_value` uses `eval_expr` without ctx, so
        // function calls in return expressions can't resolve nested
        // helpers. Only direct (non-chained) helper calls resolve.
        let env = analyze_with_helpers(
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
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
    }

    #[test]
    fn self_recursion() {
        // Self-recursion: Salsa cycle recovery returns Unknown.
        let env = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = do_tag(parser, token)
",
        );
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
    }

    #[test]
    fn helper_not_found() {
        let env = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = nonexistent_helper(token)
",
        );
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
    }

    #[test]
    fn token_kwargs_marks_unknown() {
        let env = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    result = token_kwargs(bits, parser)
",
        );
        assert_eq!(env.get("bits"), &AbstractValue::Unknown);
        assert_eq!(env.get("result"), &AbstractValue::Unknown);
    }

    #[test]
    fn parser_compile_filter() {
        let env = analyze_with_helpers(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    val = parser.compile_filter(bits[1])
",
        );
        assert_eq!(env.get("val"), &AbstractValue::Unknown);
    }

    #[test]
    fn helper_called_twice_same_args() {
        // Salsa memoizes: calling the same helper twice yields the same result.
        let env = analyze_with_helpers(
            r"
def helper(tok):
    return tok.split_contents()

def do_tag(parser, token):
    a = helper(token)
    b = helper(token)
",
        );
        assert_eq!(
            env.get("a"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
        assert_eq!(
            env.get("b"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn helper_called_with_different_args() {
        let env = analyze_with_helpers(
            r"
def helper(x):
    return x

def do_tag(parser, token):
    a = helper(token)
    b = helper(parser)
",
        );
        assert_eq!(env.get("a"), &AbstractValue::Token);
        assert_eq!(env.get("b"), &AbstractValue::Parser);
    }

    #[test]
    fn helper_with_pop_and_return() {
        let env = analyze_with_helpers(
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
        let env = analyze_with_helpers(
            r"
def get_bits(tok):
    return tok.split_contents()

def do_tag(parser, token):
    pair = (get_bits(token), 42)
",
        );
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
        let env = analyze_with_helpers(
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
        let env = analyze_with_helpers(
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
    }
}
