use ruff_python_ast::Expr;
use ruff_python_ast::ExprBoolOp;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;

use super::effects::apply_pop_mutation;
use super::effects::try_extract_option_loop;
use super::effects::try_extract_pop_call;
use super::expressions::eval_expr;
use super::expressions::eval_expr_with_ctx;
use super::match_arms::extract_match_constraints;
use super::AnalysisContext;
use super::AnalysisResult;
use crate::dataflow::domain::AbstractValue;
use crate::dataflow::domain::Env;
use crate::types::SplitPosition;

/// Process a list of statements, updating the environment.
pub fn process_statements(stmts: &[Stmt], env: &mut Env, ctx: &mut AnalysisContext<'_>) {
    for stmt in stmts {
        let result = process_statement(stmt, env, ctx);
        ctx.constraints.extend(result.constraints);
        if result.known_options.is_some() {
            ctx.known_options = result.known_options;
        }
    }
}

/// Process statements and return the accumulated results as an `AnalysisResult`
/// instead of merging them into `ctx.constraints`/`ctx.known_options`.
///
/// Temporarily swaps `ctx` accumulator fields so that recursive calls through
/// `process_statements` accumulate into a fresh set, which is then captured
/// and the originals restored.
fn collect_statements_result(
    stmts: &[Stmt],
    env: &mut Env,
    ctx: &mut AnalysisContext<'_>,
) -> AnalysisResult {
    use std::mem;

    let saved_constraints = mem::take(&mut ctx.constraints);
    let saved_options = ctx.known_options.take();

    process_statements(stmts, env, ctx);

    AnalysisResult {
        constraints: mem::replace(&mut ctx.constraints, saved_constraints),
        known_options: mem::replace(&mut ctx.known_options, saved_options),
    }
}

fn process_statement(stmt: &Stmt, env: &mut Env, ctx: &mut AnalysisContext<'_>) -> AnalysisResult {
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
                .extend(crate::dataflow::constraints::extract_from_if_inline(
                    stmt_if, env,
                ));

            // Collect body results separately so we can discard conditional
            // keywords without reaching into ctx.constraints.
            let mut body_result = collect_statements_result(&stmt_if.body, env, ctx);

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
                let mut clause_result = collect_statements_result(&clause.body, env, ctx);
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
                let body_result = collect_statements_result(&while_stmt.body, env, ctx);
                result.constraints.extend(body_result.constraints);
                if body_result.known_options.is_some() {
                    result.known_options = body_result.known_options;
                }
            }
        }

        Stmt::Match(match_stmt) => {
            // Extract constraints at the point in code where the match appears
            if let Some(match_constraints) = extract_match_constraints(match_stmt, env) {
                result.constraints.extend(match_constraints);
            }
            // Process match bodies for env updates, capturing results
            for case in &match_stmt.cases {
                let body_result = collect_statements_result(&case.body, env, ctx);
                result.constraints.extend(body_result.constraints);
                if body_result.known_options.is_some() {
                    result.known_options = body_result.known_options;
                }
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
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitResult(star_split),
                        );
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
