use ruff_python_ast::Expr;
use ruff_python_ast::ExprBoolOp;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use super::expressions::eval_expr;
use super::expressions::eval_expr_with_ctx;
use super::match_arms::extract_match_constraints;
use super::mutations::apply_pop_mutation;
use super::mutations::try_extract_option_loop;
use super::mutations::try_extract_pop_call;
use super::AnalysisResult;
use super::CallContext;
use crate::analysis::state::AbstractValue;
use crate::analysis::state::Env;
use crate::types::SplitPosition;

/// Process a list of statements, updating the environment and returning
/// accumulated analysis results.
pub fn process_statements(
    stmts: &[Stmt],
    env: &mut Env,
    ctx: &mut CallContext<'_>,
) -> AnalysisResult {
    let mut combined = AnalysisResult::default();
    for stmt in stmts {
        let result = process_statement(stmt, env, ctx);
        combined.extend(result);
    }
    combined
}

fn process_statement(stmt: &Stmt, env: &mut Env, ctx: &mut CallContext<'_>) -> AnalysisResult {
    let mut result = AnalysisResult::default();

    match stmt {
        Stmt::Assign(StmtAssign { targets, value, .. }) => {
            // Check for token_kwargs side effect: marks the bits arg as Unknown
            if let Some(var_name) = try_extract_token_kwargs_call(value) {
                env.set(var_name, AbstractValue::Unknown);
            }

            // Check if RHS is a pop call that needs side effects
            if let Some(pop_info) = try_extract_pop_call(value) {
                let rhs = eval_expr_with_ctx(value, env, Some(ctx));
                apply_pop_mutation(env, &pop_info);
                if targets.len() == 1 {
                    process_assignment_target(&targets[0], &rhs, env);
                }
            } else {
                let rhs = eval_expr_with_ctx(value, env, Some(ctx));
                if targets.len() == 1 {
                    process_assignment_target(&targets[0], &rhs, env);
                }
            }
        }

        Stmt::If(stmt_if) => {
            result
                .constraints
                .extend(crate::analysis::rules::extract_from_if_inline(
                    stmt_if, env,
                ));

            // Collect body results separately so we can discard conditional
            // keywords without reaching into ctx.constraints.
            let mut body_result = process_statements(&stmt_if.body, env, ctx);

            // When an if-condition checks a specific element value
            // (e.g. `if args[-3] == "as"`), keyword constraints extracted
            // from its body are conditional on that value and can't be
            // expressed in our flat model. Discard them.
            // Length guards (`if len(bits) >= 3`) are fine — the keyword
            // only applies when the position exists, which the evaluator
            // handles via bounds checking.
            if condition_involves_element_check(&stmt_if.test, env) {
                body_result.constraints.required_keywords.clear();
            }
            result.constraints.extend(body_result.constraints);
            if body_result.known_options.is_some() {
                result.known_options = body_result.known_options;
            }

            for clause in &stmt_if.elif_else_clauses {
                let mut clause_result = process_statements(&clause.body, env, ctx);
                if clause
                    .test
                    .as_ref()
                    .is_some_and(|t| condition_involves_element_check(t, env))
                {
                    clause_result.constraints.required_keywords.clear();
                }
                result.constraints.extend(clause_result.constraints);
                if clause_result.known_options.is_some() {
                    result.known_options = clause_result.known_options;
                }
            }
        }

        Stmt::For(stmt_for) => {
            result.extend(process_statements(&stmt_for.body, env, ctx));
            result.extend(process_statements(&stmt_for.orelse, env, ctx));
        }

        Stmt::Try(stmt_try) => {
            result.extend(process_statements(&stmt_try.body, env, ctx));
            for handler in &stmt_try.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                result.extend(process_statements(&h.body, env, ctx));
            }
            result.extend(process_statements(&stmt_try.orelse, env, ctx));
            result.extend(process_statements(&stmt_try.finalbody, env, ctx));
        }

        Stmt::With(stmt_with) => {
            result.extend(process_statements(&stmt_with.body, env, ctx));
        }

        // Expression statement: handle side effects like bits.pop(0)
        Stmt::Expr(stmt_expr) => {
            if let Some(pop_info) = try_extract_pop_call(&stmt_expr.value) {
                apply_pop_mutation(env, &pop_info);
            }
            if let Some(var_name) = try_extract_token_kwargs_call(&stmt_expr.value) {
                env.set(var_name, AbstractValue::Unknown);
            }
        }

        Stmt::While(while_stmt) => {
            if let Some(opts) = try_extract_option_loop(while_stmt, env) {
                // Option loop fully analyzed by extraction; skip body processing
                // to avoid false positives (loop variables like `option` would
                // appear as positional args).
                result.known_options = Some(opts);
            } else {
                // Non-option while loop: collect body results for assignments
                // and side effects (e.g. pop mutations, nested constraints).
                result.extend(process_statements(&while_stmt.body, env, ctx));
            }
        }

        Stmt::Match(match_stmt) => {
            // Extract constraints at the point in code where the match appears
            if let Some(match_constraints) = extract_match_constraints(match_stmt, env) {
                result.constraints.extend(match_constraints);
            }
            // Process match bodies for env updates, capturing results
            for case in &match_stmt.cases {
                result.extend(process_statements(&case.body, env, ctx));
            }
        }

        _ => {}
    }

    result
}

/// Try to detect `token_kwargs(bits, parser)` calls and return the first
/// argument name so we can mark it as Unknown (`token_kwargs` mutates bits).
fn try_extract_token_kwargs_call(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Name(ExprName { id, .. }) = call.func.as_ref() else {
        return None;
    };
    if id.as_str() != "token_kwargs" {
        return None;
    }
    // First argument is the bits variable that gets mutated
    if let Some(Expr::Name(ExprName { id: arg_name, .. })) = call.arguments.args.first() {
        return Some(arg_name.to_string());
    }
    None
}

/// Check if a condition expression involves comparing a `SplitElement` value.
///
/// Used to detect guards like `if args[-3] == "as"` where nested keyword
/// constraints would be conditional on the element's value.
fn condition_involves_element_check(expr: &Expr, env: &Env) -> bool {
    match expr {
        Expr::Compare(compare) => {
            let left = eval_expr(&compare.left, env);
            if matches!(left, AbstractValue::SplitElement { .. }) {
                return true;
            }
            compare
                .comparators
                .iter()
                .any(|c| matches!(eval_expr(c, env), AbstractValue::SplitElement { .. }))
        }
        Expr::BoolOp(ExprBoolOp { values, .. }) => values
            .iter()
            .any(|v| condition_involves_element_check(v, env)),
        Expr::UnaryOp(unary) => condition_involves_element_check(&unary.operand, env),
        _ => false,
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

        AbstractValue::SplitResult(split) => {
            let split = *split;

            // Find starred target index
            let star_index = targets.iter().position(|t| matches!(t, Expr::Starred(_)));

            if let Some(si) = star_index {
                // Elements before the star
                for (i, target) in targets[..si].iter().enumerate() {
                    if let Expr::Name(ExprName { id, .. }) = target {
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitElement {
                                index: split.resolve_index(i),
                            },
                        );
                    }
                }

                // Elements after the star (indexed from end)
                let after_star = targets.len() - si - 1;

                // The star target captures everything between pre-star and post-star elements.
                // Its back_offset must include the trailing targets it doesn't contain.
                if let Expr::Starred(starred) = &targets[si] {
                    if let Expr::Name(ExprName { id, .. }) = starred.value.as_ref() {
                        // Start from the current split sliced past the pre-star targets,
                        // which preserves the original back_offset.
                        let mut star_split = split.after_slice_from(si);
                        // Add trailing targets as additional back pops
                        for _ in 0..after_star {
                            star_split = star_split.after_pop_back();
                        }
                        env.set(id.to_string(), AbstractValue::SplitResult(star_split));
                    }
                }
                for (j, target) in targets[si + 1..].iter().enumerate() {
                    if let Expr::Name(ExprName { id, .. }) = target {
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitElement {
                                index: SplitPosition::Backward(after_star - j),
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
                                index: split.resolve_index(i),
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
    use ruff_python_ast::StmtFunctionDef;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::analysis::state::AbstractValue;
    use crate::analysis::state::Env;
    use crate::analysis::state::TokenSplit;
    use crate::testing::django_function;
    use crate::types::SplitPosition;

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
        let mut ctx = CallContext {
            db: None,
            file: None,
        };
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
            &AbstractValue::SplitResult(TokenSplit::fresh())
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
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    // Fabricated: tests `parser.token.split_contents()` pattern (classytags-
    // style). Real Django compile functions use `token.split_contents()` but
    // third-party libraries access token via parser. Keep as unit test. (b)
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
            &AbstractValue::SplitResult(TokenSplit::fresh())
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
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(2)
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
                index: SplitPosition::Backward(1)
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
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
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
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(2))
        );
        assert_eq!(
            env.get("more"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(3))
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
            &AbstractValue::SplitLength(TokenSplit::fresh())
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
            &AbstractValue::SplitResult(TokenSplit::fresh())
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
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
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
                index: SplitPosition::Forward(0)
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
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(1)
            }
        );
        assert_eq!(
            env.get("connector"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(2)
            }
        );
        assert_eq!(
            env.get("varname"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(3)
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
                index: SplitPosition::Forward(1)
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
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
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
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
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
                index: SplitPosition::Forward(0)
            }
        );
        // middle = original[1:-1], so base_offset=1 and pops_from_end=1
        // (the trailing `last` element is accounted for in pops_from_end)
        assert_eq!(
            env.get("middle"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1).after_pop_back())
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Backward(1)
            }
        );
    }

    #[test]
    fn pop_0_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
    }

    #[test]
    fn pop_0_with_assignment() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    tag_name = bits.pop(0)
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(0)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(1))
        );
    }

    #[test]
    fn pop_from_end() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_back())
        );
    }

    #[test]
    fn pop_from_end_with_assignment() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    last = bits.pop()
",
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Backward(1)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_back())
        );
    }

    #[test]
    fn multiple_pops() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    bits.pop()
    bits.pop()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(
                TokenSplit::fresh()
                    .after_pop_front()
                    .after_pop_back()
                    .after_pop_back()
            )
        );
    }

    #[test]
    fn len_after_pop() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength(TokenSplit::fresh().after_pop_front())
        );
    }

    #[test]
    fn len_after_end_pop() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
    bits.pop()
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength(TokenSplit::fresh().after_pop_back().after_pop_back())
        );
    }

    fn analyze(source: &str) -> crate::types::TagRule {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        let func = module
            .body
            .into_iter()
            .find_map(|s| {
                if let Stmt::FunctionDef(f) = s {
                    Some(f)
                } else {
                    None
                }
            })
            .expect("no function found");
        crate::analysis::analyze_compile_function(&func)
    }

    fn analyze_func(func: &StmtFunctionDef) -> crate::types::TagRule {
        crate::analysis::analyze_compile_function(func)
    }

    // Fabricated: simple option loop without duplicate check. No corpus
    // function has an option loop that allows duplicates — real Django tags
    // always check for duplicates via `if option in options:` or `if option
    // in seen:`. Keep as unit test for the simpler code path. (b)
    #[test]
    fn option_loop_basic() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining_bits = bits[2:]
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option == "with":
            pass
        elif option == "only":
            pass
        else:
            raise TemplateSyntaxError("unknown option")
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert!(opts.rejects_unknown);
        assert!(opts.allow_duplicates);
    }

    // Corpus: do_translate in i18n.py — option loop with `seen = set()`
    // duplicate check. Options: "noop", "context", "as". Rejects unknown.
    #[test]
    fn option_loop_with_duplicate_check() {
        let func = django_function("django/templatetags/i18n.py", "do_translate").unwrap();
        let rule = analyze_func(&func);
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(
            opts.values,
            vec!["noop".to_string(), "context".to_string(), "as".to_string()]
        );
        assert!(opts.rejects_unknown);
        assert!(!opts.allow_duplicates);
    }

    // Fabricated: option loop without else/raise — allows unknown options.
    // No corpus function has this pattern (real Django tags always reject
    // unknown options). Keep as unit test for permissive code path. (b)
    #[test]
    fn option_loop_allows_unknown() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while remaining:
        option = remaining.pop(0)
        if option == "noescape":
            pass
        elif option == "trimmed":
            pass
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(
            opts.values,
            vec!["noescape".to_string(), "trimmed".to_string()]
        );
        assert!(!opts.rejects_unknown);
        assert!(opts.allow_duplicates);
    }

    // Corpus: do_include in loader_tags.py — option loop with dict-based
    // duplicate check (`if option in options:`). Options: "with", "only".
    // Rejects unknown, rejects duplicates.
    #[test]
    fn option_loop_include_pattern() {
        let func = django_function("django/template/loader_tags.py", "do_include").unwrap();
        let rule = analyze_func(&func);
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert!(opts.rejects_unknown);
        assert!(!opts.allow_duplicates);
    }

    #[test]
    fn no_option_loop_returns_none() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError("err")
"#,
        );
        assert!(rule.known_options.is_none());
    }

    // Corpus: partialdef_func in defaulttags.py — match statement with
    // multiple case arms of different lengths (2 and 3 elements), producing
    // OneOf([2, 3]) constraint. Django 6.0+ match-based tag parsing.
    #[test]
    fn match_partialdef_pattern() {
        let func = django_function("django/template/defaulttags.py", "partialdef_func").unwrap();
        let rule = analyze_func(&func);
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::OneOf(vec![2, 3])),
            "expected OneOf([2, 3]), got {:?}",
            rule.arg_constraints
        );
    }

    // Corpus: partial_func in defaulttags.py — match statement with a
    // single fixed-length case (2 elements) + wildcard error, producing
    // Exact(2) constraint.
    #[test]
    fn match_partial_exact() {
        let func = django_function("django/template/defaulttags.py", "partial_func").unwrap();
        let rule = analyze_func(&func);
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::Exact(2)),
            "expected Exact(2), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_non_split_result_no_constraints() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    x = something()
    match x:
        case "a":
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints.is_empty(),
            "non-SplitResult match should produce no constraints"
        );
    }

    // Fabricated: match with star pattern (`case "tag", *rest:`). No corpus
    // function uses star patterns in match arms currently. Keep as unit test
    // for variable-length match handling. (b)
    #[test]
    fn match_star_pattern_variable_length() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", *rest:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::Min(1)),
            "expected Min(1), got {:?}",
            rule.arg_constraints
        );
    }

    // Fabricated: match with multiple fixed-length non-error arms of
    // different sizes (2 and 4 elements). Tests OneOf constraint from
    // match. No corpus function has this exact pattern. (b)
    #[test]
    fn match_multiple_valid_lengths() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a:
            pass
        case "tag", a, b, c:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::OneOf(vec![2, 4])),
            "expected OneOf([2, 4]), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_all_error_cases_no_constraints() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag":
            raise TemplateSyntaxError("bad")
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints.is_empty(),
            "all-error match should produce no constraints, got {:?}",
            rule.arg_constraints
        );
    }

    // Fabricated: wildcard match arm overrides variable-length minimum.
    // Tests that `case _: pass` (non-error) removes Min constraint. (b)
    #[test]
    fn match_wildcard_overrides_variable_min_to_zero() {
        // When a Variable arm (min_len=2) appears before a non-error Wildcard,
        // the wildcard should unconditionally set the minimum to 0 since it
        // matches anything including zero-length inputs.
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a, *rest:
            pass
        case _:
            pass
"#,
        );
        // Wildcard `case _:` is a valid (non-error) arm that matches anything,
        // so there should be no Min constraint at all (min is effectively 0).
        assert!(
            !rule
                .arg_constraints
                .iter()
                .any(|c| matches!(c, crate::types::ArgumentCountConstraint::Min(_))),
            "wildcard should override variable min to 0 (no Min constraint), got {:?}",
            rule.arg_constraints
        );
    }

    // Fabricated: non-error wildcard after fixed-length arm prevents Min
    // constraint. Tests wildcard catch-all semantics in match. (b)
    #[test]
    fn match_wildcard_after_fixed_produces_no_min() {
        // A non-error wildcard means any length is valid, so even fixed-length
        // arms shouldn't produce exact/range constraints when a wildcard is present.
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a, b:
            pass
        case _:
            pass
"#,
        );
        // The wildcard is non-error, so it acts as a variable-length catch-all.
        // With min=0, no Min constraint should be emitted.
        assert!(
            !rule
                .arg_constraints
                .iter()
                .any(|c| matches!(c, crate::types::ArgumentCountConstraint::Min(m) if *m > 0)),
            "non-error wildcard should prevent Min constraint > 0, got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_env_updates_propagate() {
        let env = eval_body(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", name:
            result = name
"#,
        );
        // The match body should have processed assignments
        assert_eq!(env.get("result"), &AbstractValue::Unknown);
    }

    #[test]
    fn while_body_assignments_propagate() {
        // Non-option while loop: body should be processed for env updates
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while some_condition:
        val = remaining.pop(0)
",
        );
        // The pop(0) assignment inside the while body should be processed
        assert_eq!(
            env.get("val"),
            &AbstractValue::SplitElement {
                index: SplitPosition::Forward(1)
            }
        );
        // The pop(0) side effect should also mutate `remaining`
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(2))
        );
    }

    #[test]
    fn while_body_pop_side_effects() {
        // Non-option while loop: pop side effects should be tracked
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[2:]
    while some_condition:
        remaining.pop(0)
",
        );
        // The pop(0) inside the while body should mutate `remaining`
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(3))
        );
    }

    #[test]
    fn contents_split_none_2_is_not_tuple() {
        // split(None, 2) should NOT be modeled as a 2-tuple;
        // only split(None, 1) has the special 2-tuple treatment.
        let env = eval_body(
            r"
def do_tag(parser, token):
    result = token.contents.split(None, 2)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn contents_split_none_0_is_not_tuple() {
        // split(None, 0) should NOT be modeled as a 2-tuple.
        let env = eval_body(
            r"
def do_tag(parser, token):
    result = token.contents.split(None, 0)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn contents_split_none_variable_is_not_tuple() {
        // split(None, some_var) should NOT be modeled as a 2-tuple.
        let env = eval_body(
            r"
def do_tag(parser, token):
    n = 1
    result = token.contents.split(None, n)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult(TokenSplit::fresh())
        );
    }

    #[test]
    fn while_option_loop_skips_body_processing() {
        // Option loop pattern: body should NOT be processed to avoid
        // the loop variable appearing as a false positional argument
        let env = eval_body(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[2:]
    while remaining:
        option = remaining.pop(0)
        if option == "with":
            pass
        elif option == "only":
            pass
        else:
            raise TemplateSyntaxError("unknown")
"#,
        );
        // `option` should NOT have a SplitElement value since the
        // option loop body is not processed (to avoid false positives)
        assert_eq!(env.get("option"), &AbstractValue::Unknown);
        // `remaining` should keep its pre-loop value
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_slice_from(2))
        );
    }
}
