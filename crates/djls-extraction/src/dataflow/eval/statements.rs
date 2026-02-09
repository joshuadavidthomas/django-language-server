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
use crate::dataflow::domain::AbstractValue;
use crate::dataflow::domain::Env;
use crate::dataflow::domain::Index;

/// Process a list of statements, updating the environment.
pub fn process_statements(stmts: &[Stmt], env: &mut Env, ctx: &mut AnalysisContext<'_>) {
    for stmt in stmts {
        process_statement(stmt, env, ctx);
    }
}

fn process_statement(stmt: &Stmt, env: &mut Env, ctx: &mut AnalysisContext<'_>) {
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
            ctx.constraints.extend(
                crate::dataflow::constraints::extract_from_if_inline(stmt_if, env),
            );

            // When an if-condition checks a specific element value
            // (e.g. `if args[-3] == "as"`), keyword constraints extracted
            // from its body are conditional on that value and can't be
            // expressed in our flat model. Discard them.
            // Length guards (`if len(bits) >= 3`) are fine — the keyword
            // only applies when the position exists, which the evaluator
            // handles via bounds checking.
            let kw_before = ctx.constraints.required_keywords.len();
            process_statements(&stmt_if.body, env, ctx);
            if condition_involves_element_check(&stmt_if.test, env) {
                ctx.constraints.required_keywords.truncate(kw_before);
            }

            for clause in &stmt_if.elif_else_clauses {
                let kw_before_clause = ctx.constraints.required_keywords.len();
                process_statements(&clause.body, env, ctx);
                if clause
                    .test
                    .as_ref()
                    .is_some_and(|t| condition_involves_element_check(t, env))
                {
                    ctx.constraints.required_keywords.truncate(kw_before_clause);
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
                ctx.known_options = Some(opts);
            } else {
                // Non-option while loop: process body for assignments and
                // side effects (e.g. pop mutations, nested constraints).
                process_statements(&while_stmt.body, env, ctx);
            }
        }

        Stmt::Match(match_stmt) => {
            // Extract constraints at the point in code where the match appears
            if let Some(match_constraints) = extract_match_constraints(match_stmt, env) {
                ctx.constraints.extend(match_constraints);
            }
            // Process match bodies for env updates
            for case in &match_stmt.cases {
                process_statements(&case.body, env, ctx);
            }
        }

        _ => {}
    }
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

        AbstractValue::SplitResult {
            base_offset,
            pops_from_end,
        } => {
            let base_offset = *base_offset;
            let pops_from_end = *pops_from_end;

            // Find starred target index
            let star_index = targets.iter().position(|t| matches!(t, Expr::Starred(_)));

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

                // Elements after the star (indexed from end)
                let after_star = targets.len() - si - 1;

                // The star target captures everything between pre-star and post-star elements.
                // Its pops_from_end must include the trailing targets it doesn't contain.
                if let Expr::Starred(starred) = &targets[si] {
                    if let Expr::Name(ExprName { id, .. }) = starred.value.as_ref() {
                        env.set(
                            id.to_string(),
                            AbstractValue::SplitResult {
                                base_offset: base_offset + si,
                                pops_from_end: pops_from_end + after_star,
                            },
                        );
                    }
                }
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
