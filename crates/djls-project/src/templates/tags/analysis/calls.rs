//! Source-backed calls and their effects on argument values.

use ruff_python_ast::Expr;
use ruff_python_ast::ExprCall;

use crate::templates::tags::HelperCall;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::analysis::state::TokenSplit;
use crate::templates::tags::analyze_helper;

/// A hashable representation of `AbstractValue` for Salsa interned keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum AbstractValueKey {
    Unknown,
    Token,
    Parser,
    SplitResult(TokenSplit),
    SplitElement(crate::templates::tags::types::SplitPosition),
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
            AbstractValue::SplitPredicate(_)
            | AbstractValue::AssignmentMap(_)
            | AbstractValue::AssignmentRemainder(_)
            | AbstractValue::Tuple(_) => AbstractValueKey::Other,
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

pub(crate) fn evaluate_source_call(
    expression: &ExprCall,
    args: &[AbstractValue],
    keywords: &[AbstractValue],
    receiver: Option<&AbstractValue>,
    env: &mut Env,
    ctx: Option<&mut CallContext<'_, '_>>,
) -> AbstractValue {
    let mut value = AbstractValue::Unknown;
    if let Some(source) = ctx.and_then(|ctx| ctx.source.as_deref_mut()) {
        let db = source.lookup.db();
        let definition = source.function.statement(db).and_then(|function| {
            source
                .lookup
                .exact_function_at_call(function, &expression.func)
        });
        if let Some(definition) = definition {
            if let Some(native) = crate::templates::tags::analysis::native::classify_native_call(
                &definition,
                expression,
                source,
            ) {
                return native.evaluate(args, keywords, env);
            }
            if keywords.is_empty()
                && !expression
                    .arguments
                    .args
                    .iter()
                    .any(|arg| matches!(arg, Expr::Starred(_)))
            {
                let arg_keys = args.iter().map(AbstractValueKey::from).collect::<Vec<_>>();
                let call = HelperCall::new(db, definition, arg_keys);
                let outcome = analyze_helper(db, call);
                source
                    .lookup
                    .absorb_evidence(&outcome.consulted_files, outcome.recovered_lookups);
                value = outcome.value;
            }
        }
    }
    for argument in args.iter().chain(keywords).chain(receiver) {
        env.forget_aliases(argument);
    }
    value
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use djls_source::FileSystem;
    use djls_source::InMemoryFileSystem;
    use djls_source::path_to_file;
    use ruff_python_ast::Stmt;
    use ruff_python_ast::StmtFunctionDef;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::templates::tags::analysis::CallContext;
    use crate::templates::tags::analysis::statements::process_statements;
    use crate::templates::tags::testing::fixture_source;
    use crate::templates::tags::types::SplitPosition;

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
            match self.fs.lock() {
                Ok(mut fs) => fs.add_file(path.into(), source.to_string()),
                Err(poisoned) => poisoned
                    .into_inner()
                    .add_file(path.into(), source.to_string()),
            }
            path_to_file(self, Utf8Path::new(path))
                .expect("inserted Python fixture should be visible")
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl crate::db::Db for TestDatabase {
        fn project(&self) -> Option<crate::Project> {
            None
        }
    }

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
        let lookup = crate::python::PythonSourceLookup::for_file(&db, file);
        let definition = lookup.definition(main_func);
        let mut source = crate::templates::tags::analysis::TagSourceContext::new(&db, definition);
        let mut ctx = CallContext {
            source: Some(&mut source),
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
            .expect("requested function should exist in source");

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
        let lookup = crate::python::PythonSourceLookup::for_file(&db, file);
        let definition = lookup.definition(main_func);
        let mut source = crate::templates::tags::analysis::TagSourceContext::new(&db, definition);
        let mut ctx = CallContext {
            source: Some(&mut source),
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
        let source = fixture_source("allauth/templatetags/allauth.py").expect("allauth fixture");
        let env = analyze_function_with_helpers(source, "do_element");
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
    fn deep_call_chain_returns_split_contents() {
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
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
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
