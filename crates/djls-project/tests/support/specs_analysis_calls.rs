use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_parser::parse_module;

use super::*;
use crate::specs::analysis::CallContext;
use crate::specs::analysis::statements::process_statements;
use crate::specs::testing::package_source;
use crate::specs::types::SplitPosition;

#[salsa::db]
#[derive(Clone)]
struct TestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<Mutex<InMemoryFileSystem>>,
    source_files: djls_source::SourceFiles,
}

impl TestDatabase {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            source_files: djls_source::SourceFiles::default(),
        }
    }

    fn create_python_file(&self, source: &str) -> djls_source::File {
        let path = "test_module.py";
        self.fs
            .lock()
            .unwrap()
            .add_file(path.into(), source.to_string());
        <Self as djls_source::Db>::get_or_create_file(self, Utf8Path::new(path))
    }
}

#[salsa::db]
impl salsa::Database for TestDatabase {}

#[salsa::db]
impl djls_source::Db for TestDatabase {
    fn files(&self) -> &djls_source::SourceFiles {
        &self.source_files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
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
        .or_else(|| funcs.first())
        .expect("no function definitions found in module");

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
    let source = package_source("django-allauth", "allauth/templatetags/allauth.py").unwrap();
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
