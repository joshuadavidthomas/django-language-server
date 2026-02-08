//! Expression evaluation and statement processing for the dataflow analyzer.

use ruff_python_ast::Arguments;
use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBoolOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprNumberLiteral;
use ruff_python_ast::ExprSlice;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::MatchCase;
use ruff_python_ast::Number;
use ruff_python_ast::Pattern;
use ruff_python_ast::PatternMatchAs;
use ruff_python_ast::PatternMatchSequence;
use ruff_python_ast::PatternMatchValue;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::StmtIf;
use ruff_python_ast::StmtMatch;
use ruff_python_ast::StmtWhile;

use super::calls::resolve_call;
use super::calls::HelperCache;
use super::domain::AbstractValue;
use super::domain::Env;
use super::domain::Index;
use crate::types::ArgumentCountConstraint;
use crate::types::KnownOptions;
use crate::types::RequiredKeyword;

/// Context for the dataflow analysis, threading through shared state.
pub struct AnalysisContext<'a> {
    pub module_funcs: &'a [&'a StmtFunctionDef],
    pub caller_name: &'a str,
    pub call_depth: usize,
    pub cache: &'a mut HelperCache,
    pub known_options: Option<KnownOptions>,
    pub constraints: super::constraints::Constraints,
}

/// Evaluate a Python expression against the abstract environment.
///
/// When `ctx` is provided, function calls can be resolved to module-local
/// helpers via bounded inlining.
pub fn eval_expr(expr: &Expr, env: &Env) -> AbstractValue {
    eval_expr_with_ctx(expr, env, None)
}

/// Evaluate a Python expression with optional analysis context for call resolution.
fn eval_expr_with_ctx(
    expr: &Expr,
    env: &Env,
    ctx: Option<&mut AnalysisContext<'_>>,
) -> AbstractValue {
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

        Expr::Call(call) => eval_call_with_ctx(call, env, ctx),

        Expr::Subscript(ExprSubscript { value, slice, .. }) => {
            let base = eval_expr(value, env);
            eval_subscript(&base, slice, env)
        }

        _ => AbstractValue::Unknown,
    }
}

/// Evaluate a function/method call expression with optional context.
fn eval_call_with_ctx(
    call: &ExprCall,
    env: &Env,
    mut ctx: Option<&mut AnalysisContext<'_>>,
) -> AbstractValue {
    if let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref() {
        let obj = eval_expr(value, env);
        let method = attr.as_str();

        // token.split_contents()
        if matches!((&obj, method), (AbstractValue::Token, "split_contents")) {
            return AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0,
            };
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
                    return AbstractValue::SplitResult {
                        base_offset: 0,
                        pops_from_end: 0,
                    };
                }
            }
        }

        // bits.pop(0) or bits.pop()
        if method == "pop" && matches!(obj, AbstractValue::SplitResult { .. }) {
            return eval_pop_return(&obj, &call.arguments);
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

        // Hardcoded external summaries for parser methods
        if matches!(obj, AbstractValue::Parser) {
            match method {
                "compile_filter" | "parse" | "delete_first_token" => {
                    return AbstractValue::Unknown;
                }
                _ => {}
            }
        }

        return AbstractValue::Unknown;
    }

    // Builtin calls: len(), list()
    if let Expr::Name(ExprName { id, .. }) = call.func.as_ref() {
        let name = id.as_str();

        // len() and list() with single argument
        if let Some(arg) = call.arguments.args.first() {
            let val = eval_expr(arg, env);
            match name {
                "len" => {
                    if let AbstractValue::SplitResult {
                        base_offset,
                        pops_from_end,
                    } = val
                    {
                        return AbstractValue::SplitLength {
                            base_offset,
                            pops_from_end,
                        };
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

        // Hardcoded external summary: token_kwargs(bits, parser)
        // Mutates bits → mark it Unknown, return Unknown
        if name == "token_kwargs" {
            return AbstractValue::Unknown;
        }

        // Try module-local function resolution
        if let Some(ctx) = ctx.as_mut() {
            let args: Vec<AbstractValue> = call
                .arguments
                .args
                .iter()
                .map(|a| eval_expr(a, env))
                .collect();
            return resolve_call(name, &args, ctx);
        }
    }

    AbstractValue::Unknown
}

/// Handle `token.contents.split(...)` patterns.
fn eval_contents_split(args: &Arguments) -> AbstractValue {
    if args.args.is_empty() {
        return AbstractValue::SplitResult {
            base_offset: 0,
            pops_from_end: 0,
        };
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

    AbstractValue::SplitResult {
        base_offset: 0,
        pops_from_end: 0,
    }
}

/// Evaluate the return value of `split_result.pop(0)` or `split_result.pop()`.
///
/// This only computes the return value — the mutation of the split result
/// is handled in `process_pop_statement`.
fn eval_pop_return(obj: &AbstractValue, args: &Arguments) -> AbstractValue {
    let AbstractValue::SplitResult {
        base_offset,
        pops_from_end,
    } = obj
    else {
        return AbstractValue::Unknown;
    };

    if let Some(arg) = args.args.first() {
        // bits.pop(0) — return element at base_offset
        if let Some(0) = expr_as_positive_usize(arg) {
            return AbstractValue::SplitElement {
                index: Index::Forward(*base_offset),
            };
        }
    } else {
        // bits.pop() — return last element (before pop)
        return AbstractValue::SplitElement {
            index: Index::Backward(*pops_from_end + 1),
        };
    }

    AbstractValue::Unknown
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
    let AbstractValue::SplitResult { base_offset, .. } = base else {
        return AbstractValue::Unknown;
    };

    match slice {
        // bits[N] or bits[-N]
        Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) => int_val.as_i64().map_or(AbstractValue::Unknown, |n| {
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

            let pops = if let AbstractValue::SplitResult { pops_from_end, .. } = base {
                *pops_from_end
            } else {
                0
            };
            match (lower.as_deref(), upper.as_deref()) {
                // bits[N:] — slice from N onwards
                (Some(lower_expr), None) => {
                    if let Some(n) = expr_as_positive_usize(lower_expr) {
                        return AbstractValue::SplitResult {
                            base_offset: base_offset + n,
                            pops_from_end: pops,
                        };
                    }
                    AbstractValue::Unknown
                }
                // bits[:N], bits[:-N], or bits[:] — truncation, preserve offset
                (None, _) => AbstractValue::SplitResult {
                    base_offset: *base_offset,
                    pops_from_end: pops,
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

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn expr_as_positive_usize(expr: &Expr) -> Option<usize> {
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
            super::constraints::extract_from_if_inline(stmt_if, env, &mut ctx.constraints);

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
            // Try to extract option loop pattern; result stored in ctx
            if let Some(opts) = try_extract_option_loop(while_stmt, env) {
                ctx.known_options = Some(opts);
            }
        }

        Stmt::Match(match_stmt) => {
            // Extract constraints at the point in code where the match appears
            if let Some((arg_constraints, keywords)) = extract_match_constraints(match_stmt, env) {
                ctx.constraints.arg_constraints.extend(arg_constraints);
                ctx.constraints.required_keywords.extend(keywords);
            }
            // Process match bodies for env updates
            for case in &match_stmt.cases {
                process_statements(&case.body, env, ctx);
            }
        }

        _ => {}
    }
}

/// Info about a `bits.pop(...)` call for mutation tracking.
struct PopInfo {
    /// The variable name being popped from (e.g., "bits")
    var_name: String,
    /// Whether this is `pop(0)` (from front) or `pop()` (from end)
    from_front: bool,
}

/// Try to extract pop call info from an expression, without evaluating it.
fn try_extract_pop_call(expr: &Expr) -> Option<PopInfo> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref() else {
        return None;
    };
    if attr.as_str() != "pop" {
        return None;
    }
    let Expr::Name(ExprName { id, .. }) = value.as_ref() else {
        return None;
    };

    let from_front = if let Some(arg) = call.arguments.args.first() {
        expr_as_positive_usize(arg) == Some(0)
    } else {
        false
    };

    Some(PopInfo {
        var_name: id.to_string(),
        from_front,
    })
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

/// Apply the mutation side effect of a pop call to the environment.
fn apply_pop_mutation(env: &mut Env, pop_info: &PopInfo) {
    env.mutate(&pop_info.var_name, |v| {
        if let AbstractValue::SplitResult {
            base_offset,
            pops_from_end,
        } = v
        {
            if pop_info.from_front {
                *base_offset += 1;
            } else {
                *pops_from_end += 1;
            }
        }
    });
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

/// Try to extract a `KnownOptions` from a `while remaining:` option-parsing loop.
///
/// Detects the pattern:
/// ```python
/// while remaining:
///     option = remaining.pop(0)
///     if option == "with":
///         ...
///     elif option == "only":
///         ...
///     else:
///         raise TemplateSyntaxError("unknown option")
/// ```
fn try_extract_option_loop(while_stmt: &StmtWhile, env: &Env) -> Option<KnownOptions> {
    // The loop test must be a simple name that resolves to SplitResult (or derivative)
    let Expr::Name(ExprName { id: loop_var, .. }) = &*while_stmt.test else {
        return None;
    };
    let loop_value = env.get(loop_var.as_str());
    if !matches!(
        loop_value,
        AbstractValue::SplitResult { .. } | AbstractValue::Unknown
    ) {
        return None;
    }

    // Look for `option = loop_var.pop(0)` in the body
    let option_var = find_option_pop_var(&while_stmt.body, loop_var.as_str())?;

    // Scan if/elif/else chains for option value checks
    let mut values = Vec::new();
    let mut rejects_unknown = false;
    let mut allow_duplicates = true;

    for stmt in &while_stmt.body {
        if let Stmt::If(if_stmt) = stmt {
            extract_option_checks(
                if_stmt,
                &option_var,
                &mut values,
                &mut rejects_unknown,
                &mut allow_duplicates,
            );
        }
    }

    if values.is_empty() {
        return None;
    }

    Some(KnownOptions {
        values,
        allow_duplicates,
        rejects_unknown,
    })
}

/// Find the variable assigned from `loop_var.pop(0)` in a while-loop body.
fn find_option_pop_var(body: &[Stmt], loop_var: &str) -> Option<String> {
    for stmt in body {
        if let Stmt::Assign(assign) = stmt {
            if assign.targets.len() == 1 {
                if let Expr::Name(ExprName { id, .. }) = &assign.targets[0] {
                    let is_pop_zero = try_extract_pop_call(&assign.value)
                        .is_some_and(|info| info.var_name == loop_var && info.from_front);
                    if is_pop_zero {
                        return Some(id.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Extract option names from if/elif/else chains checking the option variable.
fn extract_option_checks(
    if_stmt: &StmtIf,
    option_var: &str,
    values: &mut Vec<String>,
    rejects_unknown: &mut bool,
    allow_duplicates: &mut bool,
) {
    // Check for duplicate detection: `if option in seen_options`
    if is_duplicate_check(&if_stmt.test, option_var) {
        *allow_duplicates = false;
    } else if let Some(opt_name) = extract_option_equality(&if_stmt.test, option_var) {
        if !values.contains(&opt_name) {
            values.push(opt_name);
        }
    }

    // Process elif/else clauses
    for clause in &if_stmt.elif_else_clauses {
        if let Some(test) = &clause.test {
            if is_duplicate_check(test, option_var) {
                *allow_duplicates = false;
            } else if let Some(opt_name) = extract_option_equality(test, option_var) {
                if !values.contains(&opt_name) {
                    values.push(opt_name);
                }
            }
        } else {
            // else branch — if it raises TemplateSyntaxError, unknown options are rejected
            if super::constraints::body_raises_template_syntax_error(&clause.body) {
                *rejects_unknown = true;
            }
        }
    }
}

/// Check if a condition is `option in seen_set` (duplicate detection).
fn is_duplicate_check(test: &Expr, option_var: &str) -> bool {
    let Expr::Compare(ExprCompare {
        left,
        ops,
        comparators,
        ..
    }) = test
    else {
        return false;
    };
    if ops.len() != 1 || comparators.len() != 1 {
        return false;
    }
    if !matches!(ops[0], CmpOp::In) {
        return false;
    }
    let Expr::Name(ExprName { id, .. }) = left.as_ref() else {
        return false;
    };
    if id.as_str() != option_var {
        return false;
    }
    matches!(comparators[0], Expr::Name(_))
}

/// Extract option name from `option == "name"`.
fn extract_option_equality(test: &Expr, option_var: &str) -> Option<String> {
    let Expr::Compare(ExprCompare {
        left,
        ops,
        comparators,
        ..
    }) = test
    else {
        return None;
    };
    if ops.len() != 1 || comparators.len() != 1 {
        return None;
    }
    if !matches!(ops[0], CmpOp::Eq) {
        return None;
    }
    let Expr::Name(ExprName { id, .. }) = left.as_ref() else {
        return None;
    };
    if id.as_str() != option_var {
        return None;
    }
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = &comparators[0] {
        return Some(value.to_str().to_string());
    }
    None
}

/// Extract argument constraints from a match statement whose subject is a `SplitResult`.
///
/// Analyzes `match token.split_contents(): case ...:` patterns from Django 6.0+.
/// Collects valid case shapes (cases whose body does NOT raise `TemplateSyntaxError`)
/// and derives argument count constraints and required keywords from them.
fn extract_match_constraints(
    match_stmt: &StmtMatch,
    env: &Env,
) -> Option<(Vec<ArgumentCountConstraint>, Vec<RequiredKeyword>)> {
    let subject = eval_expr(&match_stmt.subject, env);
    if !matches!(subject, AbstractValue::SplitResult { .. }) {
        return None;
    }

    let mut valid_lengths: Vec<usize> = Vec::new();
    let mut has_variable_length = false;
    let mut min_variable_length: Option<usize> = None;

    for case in &match_stmt.cases {
        if any_path_raises_template_syntax_error(&case.body) {
            continue;
        }

        match analyze_case_pattern(&case.pattern) {
            PatternShape::Fixed(len) => {
                if !valid_lengths.contains(&len) {
                    valid_lengths.push(len);
                }
            }
            PatternShape::Variable { min_len } => {
                has_variable_length = true;
                match min_variable_length {
                    Some(current) if min_len < current => min_variable_length = Some(min_len),
                    None => min_variable_length = Some(min_len),
                    _ => {}
                }
            }
            PatternShape::Wildcard => {
                // Wildcard/irrefutable pattern — no length constraint from this case
                has_variable_length = true;
                if min_variable_length.is_none() {
                    min_variable_length = Some(0);
                }
            }
            PatternShape::Unknown => {}
        }
    }

    if valid_lengths.is_empty() && !has_variable_length {
        return None;
    }

    let mut constraints = Vec::new();

    if has_variable_length {
        // Variable-length patterns: only Min constraint from the shortest
        if let Some(min) = min_variable_length {
            let fixed_min = valid_lengths.iter().copied().min();
            let overall_min = match fixed_min {
                Some(fm) if fm < min => fm,
                _ => min,
            };
            if overall_min > 0 {
                constraints.push(ArgumentCountConstraint::Min(overall_min));
            }
        }
    } else {
        // Only fixed-length patterns
        valid_lengths.sort_unstable();
        if valid_lengths.len() == 1 {
            constraints.push(ArgumentCountConstraint::Exact(valid_lengths[0]));
        } else {
            // Check if contiguous range → Min + Max
            let min = valid_lengths[0];
            let max = valid_lengths[valid_lengths.len() - 1];
            let is_contiguous = max - min + 1 == valid_lengths.len();
            if is_contiguous && valid_lengths.len() > 2 {
                constraints.push(ArgumentCountConstraint::Min(min));
                constraints.push(ArgumentCountConstraint::Max(max));
            } else {
                constraints.push(ArgumentCountConstraint::OneOf(valid_lengths));
            }
        }
    }

    // Extract required keywords from valid cases
    let keywords = extract_keywords_from_valid_cases(&match_stmt.cases);

    Some((constraints, keywords))
}

/// Shape determined from analyzing a match case pattern.
enum PatternShape {
    /// Fixed number of elements (from `PatternMatchSequence` without star)
    Fixed(usize),
    /// Variable number of elements (from `PatternMatchSequence` with star)
    Variable { min_len: usize },
    /// Wildcard/irrefutable pattern (`case _:` or `case x:`)
    Wildcard,
    /// Unrecognized pattern
    Unknown,
}

/// Analyze a case pattern to determine its shape.
fn analyze_case_pattern(pattern: &Pattern) -> PatternShape {
    match pattern {
        Pattern::MatchSequence(PatternMatchSequence { patterns, .. }) => {
            let has_star = patterns.iter().any(|p| matches!(p, Pattern::MatchStar(_)));
            if has_star {
                // Count non-star elements for minimum length
                let fixed_count = patterns
                    .iter()
                    .filter(|p| !matches!(p, Pattern::MatchStar(_)))
                    .count();
                PatternShape::Variable {
                    min_len: fixed_count,
                }
            } else {
                PatternShape::Fixed(patterns.len())
            }
        }
        // `case _:` or `case x:` — wildcard/capture, matches anything
        Pattern::MatchAs(PatternMatchAs { pattern: None, .. }) => PatternShape::Wildcard,
        // `case pattern as x:` — delegate to inner pattern
        Pattern::MatchAs(PatternMatchAs {
            pattern: Some(inner),
            ..
        }) => analyze_case_pattern(inner),
        _ => PatternShape::Unknown,
    }
}

/// Extract required keyword literals from valid (non-error) match cases.
///
/// When ALL valid cases of the same length agree on a literal at a specific position,
/// that position has a required keyword.
fn extract_keywords_from_valid_cases(cases: &[MatchCase]) -> Vec<RequiredKeyword> {
    // Collect fixed-length valid cases grouped by length
    let mut by_length: std::collections::HashMap<usize, Vec<Vec<Option<String>>>> =
        std::collections::HashMap::new();

    for case in cases {
        if any_path_raises_template_syntax_error(&case.body) {
            continue;
        }
        if let Pattern::MatchSequence(PatternMatchSequence { patterns, .. }) = &case.pattern {
            if patterns.iter().any(|p| matches!(p, Pattern::MatchStar(_))) {
                continue; // Skip variable-length patterns for keyword extraction
            }
            let literals: Vec<Option<String>> = patterns.iter().map(pattern_literal).collect();
            by_length.entry(patterns.len()).or_default().push(literals);
        }
    }

    let mut keywords = Vec::new();
    for cases_at_len in by_length.values() {
        if cases_at_len.is_empty() {
            continue;
        }
        let num_positions = cases_at_len[0].len();
        for pos in 0..num_positions {
            // Check if ALL cases agree on the same literal at this position
            let first_literal = &cases_at_len[0][pos];
            if let Some(lit) = first_literal {
                if cases_at_len
                    .iter()
                    .all(|c| c.get(pos).and_then(|v| v.as_ref()) == Some(lit))
                {
                    // Skip position 0 — that's the tag name, not a user argument
                    if pos > 0 {
                        #[allow(clippy::cast_possible_wrap)]
                        keywords.push(RequiredKeyword {
                            position: pos as i64,
                            value: lit.clone(),
                        });
                    }
                }
            }
        }
    }

    keywords
}

/// Extract a string literal from a pattern element, if it is one.
fn pattern_literal(pattern: &Pattern) -> Option<String> {
    match pattern {
        Pattern::MatchValue(PatternMatchValue { value, .. }) => {
            if let Expr::StringLiteral(ExprStringLiteral { value: s, .. }) = value.as_ref() {
                Some(s.to_str().to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Check if any code path in a body contains `raise TemplateSyntaxError(...)`.
///
/// Unlike `constraints::body_raises_template_syntax_error` (which only checks
/// direct raises), this recurses into if/elif/else branches. Used for match
/// case classification where any raise in any branch means the case can error.
fn any_path_raises_template_syntax_error(body: &[Stmt]) -> bool {
    use ruff_python_ast::StmtRaise;

    for stmt in body {
        match stmt {
            Stmt::Raise(StmtRaise { exc: Some(exc), .. }) => {
                if super::constraints::is_template_syntax_error_call(exc) {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if any_path_raises_template_syntax_error(&if_stmt.body) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if any_path_raises_template_syntax_error(&clause.body) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
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
        let mut cache = crate::dataflow::HelperCache::new();
        let mut ctx = AnalysisContext {
            module_funcs: &[],
            caller_name: "test",
            call_depth: 0,
            cache: &mut cache,
            known_options: None,
            constraints: crate::dataflow::constraints::Constraints::default(),
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
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 2,
                pops_from_end: 0
            }
        );
        assert_eq!(
            env.get("more"),
            &AbstractValue::SplitResult {
                base_offset: 3,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitLength {
                base_offset: 0,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
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
        // middle = original[1:-1], so base_offset=1 and pops_from_end=1
        // (the trailing `last` element is accounted for in pops_from_end)
        assert_eq!(
            env.get("middle"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 1
            }
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: Index::Backward(1)
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
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
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
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 1
            }
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
                index: Index::Backward(1)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 1
            }
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
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 2
            }
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
            &AbstractValue::SplitLength {
                base_offset: 1,
                pops_from_end: 0
            }
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
            &AbstractValue::SplitLength {
                base_offset: 0,
                pops_from_end: 2
            }
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
        crate::dataflow::analyze_compile_function(&func, &[])
    }

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

    #[test]
    fn option_loop_with_duplicate_check() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining_bits = bits[2:]
    seen = set()
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option in seen:
            raise TemplateSyntaxError("duplicate option")
        elif option == "silent":
            pass
        elif option == "cache":
            pass
        else:
            raise TemplateSyntaxError("unknown option")
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["silent".to_string(), "cache".to_string()]);
        assert!(opts.rejects_unknown);
        assert!(!opts.allow_duplicates);
    }

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

    #[test]
    fn option_loop_include_pattern() {
        let rule = analyze(
            r#"
def do_include(parser, token):
    bits = token.split_contents()
    options = {}
    remaining_bits = bits[2:]
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option == "with":
            value = remaining_bits.pop(0)
        elif option == "only":
            options["only"] = True
        else:
            raise TemplateSyntaxError("unknown option")
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert!(opts.rejects_unknown);
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

    #[test]
    fn match_partialdef_pattern() {
        let rule = analyze(
            r#"
def partialdef_func(parser, token):
    match token.split_contents():
        case "partialdef", partial_name, "inline":
            inline = True
        case "partialdef", partial_name, _:
            raise TemplateSyntaxError("bad")
        case "partialdef", partial_name:
            inline = False
        case ["partialdef"]:
            raise TemplateSyntaxError("bad")
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::OneOf(vec![2, 3])),
            "expected OneOf([2, 3]), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_partial_exact() {
        let rule = analyze(
            r#"
def partial_func(parser, token):
    match token.split_contents():
        case "partial", partial_name:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::Exact(2)),
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
                .contains(&ArgumentCountConstraint::Min(1)),
            "expected Min(1), got {:?}",
            rule.arg_constraints
        );
    }

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
                .contains(&ArgumentCountConstraint::OneOf(vec![2, 4])),
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
}
