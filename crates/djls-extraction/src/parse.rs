use djls_source::File;
use djls_source::FileKind;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;

use crate::dataflow::domain::AbstractValue;
use crate::dataflow::domain::Env;
use crate::dataflow::eval::process_statements;
use crate::dataflow::eval::CallContext;
use crate::dataflow::extract_return_value;
use crate::dataflow::AbstractValueKey;

/// Parsed Python module AST, cached by Salsa.
///
/// Wraps Ruff's statement list in a tracked struct. The parsed AST is
/// invalidated when the source file changes.
#[salsa::tracked]
pub struct ParsedPythonModule<'db> {
    #[tracked]
    #[returns(ref)]
    pub body: Vec<Stmt>,
}

/// Interned key for a helper function call.
///
/// Salsa uses interning to deduplicate identical helper calls: same file,
/// same callee name, same abstract argument values produce the same
/// `HelperCall` identity, enabling Salsa's built-in memoization.
#[salsa::interned]
pub struct HelperCall<'db> {
    pub file: File,
    #[returns(ref)]
    pub callee_name: String,
    #[returns(ref)]
    pub args: Vec<AbstractValueKey>,
}

/// Parse a Python source file into a cached AST.
///
/// Returns `None` for non-Python files or files that fail to parse.
/// The parsed AST is cached by Salsa and invalidated when
/// `file.source(db)` changes.
#[salsa::tracked]
pub fn parse_python_module(db: &dyn djls_source::Db, file: File) -> Option<ParsedPythonModule<'_>> {
    let source = file.source(db);
    if *source.kind() != FileKind::Python {
        return None;
    }

    let parsed = ruff_python_parser::parse_module(source.as_ref());
    let module = match parsed {
        Ok(parsed) => parsed.into_syntax(),
        Err(_) => return None,
    };

    Some(ParsedPythonModule::new(db, module.body))
}

/// Analyze a helper function call and return its abstract return value.
///
/// This is a Salsa tracked function with cycle recovery: if A calls B
/// which calls A (directly or transitively), the cycle resolves to
/// `AbstractValue::Unknown` instead of panicking.
///
/// Looks up the callee by name in the parsed module's AST, binds
/// parameters to the abstract argument values from `HelperCall`, runs
/// the dataflow evaluator on the callee body, and extracts the return
/// value.
#[salsa::tracked(cycle_fn=analyze_helper_cycle_recover, cycle_initial=analyze_helper_cycle_initial)]
pub fn analyze_helper(db: &dyn djls_source::Db, call: HelperCall<'_>) -> AbstractValue {
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

    extract_return_value(&callee.body, &callee_env)
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
