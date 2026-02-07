//! Expression evaluation and statement processing for the dataflow analyzer.

use ruff_python_ast::Arguments;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprNumberLiteral;
use ruff_python_ast::ExprSlice;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::Number;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtFunctionDef;

use super::domain::AbstractValue;
use super::domain::Env;
use super::domain::Index;

/// Context for the dataflow analysis, threading through shared state.
#[allow(dead_code)]
pub struct AnalysisContext<'a> {
    pub module_funcs: &'a [&'a StmtFunctionDef],
    pub caller_name: &'a str,
    pub call_depth: usize,
}

/// Evaluate a Python expression against the abstract environment.
pub fn eval_expr(expr: &Expr, env: &Env) -> AbstractValue {
    match expr {
        Expr::Name(ExprName { id, .. }) => env.get(id.as_str()).clone(),

        Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) => int_val
            .as_i64()
            .map_or(AbstractValue::Unknown, AbstractValue::Int),

        Expr::StringLiteral(ExprStringLiteral { value, .. }) => {
            AbstractValue::Str(value.to_string())
        }

        Expr::Tuple(ExprTuple { elts, .. }) => {
            AbstractValue::Tuple(elts.iter().map(|e| eval_expr(e, env)).collect())
        }

        Expr::Call(call) => eval_call(call, env),

        Expr::Subscript(ExprSubscript { value, slice, .. }) => {
            let base = eval_expr(value, env);
            eval_subscript(&base, slice, env)
        }

        _ => AbstractValue::Unknown,
    }
}

/// Evaluate a function/method call expression.
fn eval_call(call: &ExprCall, env: &Env) -> AbstractValue {
    if let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref() {
        let obj = eval_expr(value, env);
        let method = attr.as_str();

        // token.split_contents()
        if matches!((&obj, method), (AbstractValue::Token, "split_contents")) {
            return AbstractValue::SplitResult { base_offset: 0 };
        }

        // parser.token.split_contents()
        if method == "split_contents" {
            if let Expr::Attribute(ExprAttribute {
                value: inner_value,
                attr: inner_attr,
                ..
            }) = value.as_ref()
            {
                let inner_obj = eval_expr(inner_value, env);
                if matches!(inner_obj, AbstractValue::Parser) && inner_attr.as_str() == "token" {
                    return AbstractValue::SplitResult { base_offset: 0 };
                }
            }
        }

        // token.contents.split(...)
        if method == "split" {
            if let Expr::Attribute(ExprAttribute {
                value: inner_value,
                attr: inner_attr,
                ..
            }) = value.as_ref()
            {
                let inner_obj = eval_expr(inner_value, env);
                if matches!(inner_obj, AbstractValue::Token) && inner_attr.as_str() == "contents" {
                    return eval_contents_split(&call.arguments);
                }
            }
        }

        return AbstractValue::Unknown;
    }

    // Builtin calls: len(), list()
    if let Expr::Name(ExprName { id, .. }) = call.func.as_ref() {
        if let Some(arg) = call.arguments.args.first() {
            let val = eval_expr(arg, env);
            match id.as_str() {
                "len" => {
                    if let AbstractValue::SplitResult { base_offset } = val {
                        return AbstractValue::SplitLength { base_offset };
                    }
                }
                "list" => {
                    if matches!(val, AbstractValue::SplitResult { .. }) {
                        return val;
                    }
                }
                _ => {}
            }
        }
    }

    AbstractValue::Unknown
}

/// Handle `token.contents.split(...)` patterns.
fn eval_contents_split(args: &Arguments) -> AbstractValue {
    if args.args.is_empty() {
        return AbstractValue::SplitResult { base_offset: 0 };
    }

    // token.contents.split(None, 1) → Tuple of [SplitElement(Forward(0)), Unknown]
    if args.args.len() == 2 {
        if let Expr::NoneLiteral(_) = &args.args[0] {
            return AbstractValue::Tuple(vec![
                AbstractValue::SplitElement {
                    index: Index::Forward(0),
                },
                AbstractValue::Unknown,
            ]);
        }
    }

    AbstractValue::SplitResult { base_offset: 0 }
}

/// Convert an i64 to an `AbstractValue` index element based on sign.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn i64_to_index_element(n: i64, base_offset: usize) -> AbstractValue {
    if n >= 0 {
        AbstractValue::SplitElement {
            index: Index::Forward(base_offset + n as usize),
        }
    } else {
        AbstractValue::SplitElement {
            index: Index::Backward((-n) as usize),
        }
    }
}

/// Evaluate subscript access on an abstract value.
fn eval_subscript(base: &AbstractValue, slice: &Expr, env: &Env) -> AbstractValue {
    let AbstractValue::SplitResult { base_offset } = base else {
        return AbstractValue::Unknown;
    };

    match slice {
        // bits[N] or bits[-N]
        Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) => int_val
            .as_i64()
            .map_or(AbstractValue::Unknown, |n| {
                i64_to_index_element(n, *base_offset)
            }),

        // bits[unary -N]
        Expr::UnaryOp(unary) if matches!(unary.op, ruff_python_ast::UnaryOp::USub) => {
            if let Expr::NumberLiteral(ExprNumberLiteral {
                value: Number::Int(int_val),
                ..
            }) = unary.operand.as_ref()
            {
                if let Some(n) = int_val.as_i64() {
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    return AbstractValue::SplitElement {
                        index: Index::Backward(n as usize),
                    };
                }
            }
            AbstractValue::Unknown
        }

        // bits[N:], bits[:N], bits[:-N]
        Expr::Slice(ExprSlice {
            lower, upper, step, ..
        }) => {
            if step.is_some() {
                return AbstractValue::Unknown;
            }

            match (lower.as_deref(), upper.as_deref()) {
                // bits[N:] — slice from N onwards
                (Some(lower_expr), None) => {
                    if let Some(n) = expr_as_positive_usize(lower_expr) {
                        return AbstractValue::SplitResult {
                            base_offset: base_offset + n,
                        };
                    }
                    AbstractValue::Unknown
                }
                // bits[:N], bits[:-N], or bits[:] — truncation, preserve offset
                (None, _) => AbstractValue::SplitResult {
                    base_offset: *base_offset,
                },
                _ => AbstractValue::Unknown,
            }
        }

        // bits[variable]
        Expr::Name(_) => {
            let idx = eval_expr(slice, env);
            if let AbstractValue::Int(n) = idx {
                i64_to_index_element(n, *base_offset)
            } else {
                AbstractValue::Unknown
            }
        }

        _ => AbstractValue::Unknown,
    }
}

/// Try to extract a non-negative integer from a literal expression.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn expr_as_positive_usize(expr: &Expr) -> Option<usize> {
    if let Expr::NumberLiteral(ExprNumberLiteral {
        value: Number::Int(int_val),
        ..
    }) = expr
    {
        if let Some(n) = int_val.as_i64() {
            if n >= 0 {
                return Some(n as usize);
            }
        }
    }
    None
}

/// Process a list of statements, updating the environment.
pub fn process_statements(stmts: &[Stmt], env: &mut Env, ctx: &mut AnalysisContext<'_>) {
    for stmt in stmts {
        process_statement(stmt, env, ctx);
    }
}

fn process_statement(stmt: &Stmt, env: &mut Env, ctx: &mut AnalysisContext<'_>) {
    match stmt {
        Stmt::Assign(StmtAssign { targets, value, .. }) => {
            let rhs = eval_expr(value, env);
            if targets.len() == 1 {
                process_assignment_target(&targets[0], &rhs, env);
            }
        }

        Stmt::If(stmt_if) => {
            process_statements(&stmt_if.body, env, ctx);
            for clause in &stmt_if.elif_else_clauses {
                process_statements(&clause.body, env, ctx);
            }
        }

        Stmt::For(stmt_for) => {
            process_statements(&stmt_for.body, env, ctx);
        }

        Stmt::Try(stmt_try) => {
            process_statements(&stmt_try.body, env, ctx);
            for handler in &stmt_try.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                process_statements(&h.body, env, ctx);
            }
            process_statements(&stmt_try.orelse, env, ctx);
            process_statements(&stmt_try.finalbody, env, ctx);
        }

        Stmt::With(stmt_with) => {
            process_statements(&stmt_with.body, env, ctx);
        }

        // Expression statements (side effects like bits.pop(0)) — Phase 4
        // While loops (option parsing) — Phase 6
        // Match statements — Phase 7
        _ => {}
    }
}

/// Process an assignment target with the evaluated RHS value.
fn process_assignment_target(target: &Expr, value: &AbstractValue, env: &mut Env) {
    match target {
        Expr::Name(ExprName { id, .. }) => {
            env.set(id.to_string(), value.clone());
        }
        Expr::Tuple(ExprTuple { elts, .. }) => {
            process_tuple_unpack(elts, value, env);
        }
        _ => {}
    }
}

/// Handle tuple unpacking assignment.
fn process_tuple_unpack(targets: &[Expr], value: &AbstractValue, env: &mut Env) {
    match value {
        AbstractValue::Tuple(elements) => {
            for (i, target) in targets.iter().enumerate() {
                let elem = elements.get(i).cloned().unwrap_or(AbstractValue::Unknown);
                if let Expr::Name(ExprName { id, .. }) = target {
                    env.set(id.to_string(), elem);
                }
            }
        }

        AbstractValue::SplitResult { base_offset } => {
            let base_offset = *base_offset;

            // Find starred target index
            let star_index = targets
                .iter()
                .position(|t| matches!(t, Expr::Starred(_)));

            if let Some(si) = star_index {
                // Elements before the star
                for (i, target) in targets[..si].iter().enumerate() {
                    if let Expr::Name(ExprName { id, .. }) = target {
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitElement {
                                index: Index::Forward(base_offset + i),
                            },
                        );
                    }
                }

                // The star target
                if let Expr::Starred(starred) = &targets[si] {
                    if let Expr::Name(ExprName { id, .. }) = starred.value.as_ref() {
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitResult {
                                base_offset: base_offset + si,
                            },
                        );
                    }
                }

                // Elements after the star (indexed from end)
                let after_star = targets.len() - si - 1;
                for (j, target) in targets[si + 1..].iter().enumerate() {
                    if let Expr::Name(ExprName { id, .. }) = target {
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitElement {
                                index: Index::Backward(after_star - j),
                            },
                        );
                    }
                }
            } else {
                // No star: each target gets a SplitElement at its position
                for (i, target) in targets.iter().enumerate() {
                    if let Expr::Name(ExprName { id, .. }) = target {
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitElement {
                                index: Index::Forward(base_offset + i),
                            },
                        );
                    }
                }
            }
        }

        _ => {
            for target in targets {
                if let Expr::Name(ExprName { id, .. }) = target {
                    env.set(id.to_string(), AbstractValue::Unknown);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;

    fn parse_function(source: &str) -> StmtFunctionDef {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        for stmt in module.body {
            if let Stmt::FunctionDef(func_def) = stmt {
                return func_def;
            }
        }
        panic!("no function definition found in source");
    }

    fn make_ctx() -> AnalysisContext<'static> {
        AnalysisContext {
            module_funcs: &[],
            caller_name: "test",
            call_depth: 0,
        }
    }

    fn eval_body(source: &str) -> Env {
        let func = parse_function(source);
        let parser_param = func
            .parameters
            .args
            .first()
            .map_or("parser", |p| p.parameter.name.as_str());
        let token_param = func
            .parameters
            .args
            .get(1)
            .map_or("token", |p| p.parameter.name.as_str());
        let mut env = Env::for_compile_function(parser_param, token_param);
        let mut ctx = make_ctx();
        process_statements(&func.body, &mut env, &mut ctx);
        env
    }

    #[test]
    fn env_initialization() {
        let source = "def do_tag(parser, token): pass";
        let func = parse_function(source);
        let env = Env::for_compile_function(
            func.parameters.args[0].parameter.name.as_str(),
            func.parameters.args[1].parameter.name.as_str(),
        );
        assert_eq!(env.get("parser"), &AbstractValue::Parser);
        assert_eq!(env.get("token"), &AbstractValue::Token);
        assert_eq!(env.get("nonexistent"), &AbstractValue::Unknown);
    }

    #[test]
    fn split_contents_binding() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult { base_offset: 0 }
        );
    }

    #[test]
    fn contents_split_binding() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    args = token.contents.split()
",
        );
        assert_eq!(
            env.get("args"),
            &AbstractValue::SplitResult { base_offset: 0 }
        );
    }

    #[test]
    fn parser_token_split_contents() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = parser.token.split_contents()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult { base_offset: 0 }
        );
    }

    #[test]
    fn subscript_forward() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    tag_name = bits[0]
    item = bits[2]
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: Index::Forward(2)
            }
        );
    }

    #[test]
    fn subscript_negative() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    last = bits[-1]
",
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: Index::Backward(1)
            }
        );
    }

    #[test]
    fn slice_from_start() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult { base_offset: 1 }
        );
    }

    #[test]
    fn slice_with_existing_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[2:]
    more = rest[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult { base_offset: 2 }
        );
        assert_eq!(
            env.get("more"),
            &AbstractValue::SplitResult { base_offset: 3 }
        );
    }

    #[test]
    fn len_of_split_result() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength { base_offset: 0 }
        );
    }

    #[test]
    fn list_wrapping() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits = list(bits)
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult { base_offset: 0 }
        );
    }

    #[test]
    fn star_unpack() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, *rest = token.split_contents()
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult { base_offset: 1 }
        );
    }

    #[test]
    fn tuple_unpack() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    a, b, c = (1, 'x', None)
",
        );
        assert_eq!(env.get("a"), &AbstractValue::Int(1));
        assert_eq!(env.get("b"), &AbstractValue::Str("x".to_string()));
        assert_eq!(env.get("c"), &AbstractValue::Unknown);
    }

    #[test]
    fn contents_split_none_1() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, rest = token.contents.split(None, 1)
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(env.get("rest"), &AbstractValue::Unknown);
    }

    #[test]
    fn unknown_variable() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    x = some_function()
",
        );
        assert_eq!(env.get("x"), &AbstractValue::Unknown);
    }

    #[test]
    fn split_result_tuple_unpack_no_star() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, item, connector, varname = token.split_contents()
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: Index::Forward(1)
            }
        );
        assert_eq!(
            env.get("connector"),
            &AbstractValue::SplitElement {
                index: Index::Forward(2)
            }
        );
        assert_eq!(
            env.get("varname"),
            &AbstractValue::SplitElement {
                index: Index::Forward(3)
            }
        );
    }

    #[test]
    fn subscript_with_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[1:]
    second = rest[0]
",
        );
        assert_eq!(
            env.get("second"),
            &AbstractValue::SplitElement {
                index: Index::Forward(1)
            }
        );
    }

    #[test]
    fn if_branch_updates_env() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    if True:
        rest = bits[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult { base_offset: 1 }
        );
    }

    #[test]
    fn integer_literal() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    n = 42
",
        );
        assert_eq!(env.get("n"), &AbstractValue::Int(42));
    }

    #[test]
    fn string_literal() {
        let env = eval_body(
            r#"
def do_tag(parser, token):
    s = "hello"
"#,
        );
        assert_eq!(env.get("s"), &AbstractValue::Str("hello".to_string()));
    }

    #[test]
    fn slice_truncation_preserves_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits = bits[1:]
    truncated = bits[:3]
",
        );
        assert_eq!(
            env.get("truncated"),
            &AbstractValue::SplitResult { base_offset: 1 }
        );
    }

    #[test]
    fn star_unpack_with_trailing() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    first, *middle, last = token.split_contents()
",
        );
        assert_eq!(
            env.get("first"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("middle"),
            &AbstractValue::SplitResult { base_offset: 1 }
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: Index::Backward(1)
            }
        );
    }
}
