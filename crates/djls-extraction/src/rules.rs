use ruff_python_ast::BoolOp;
use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBoolOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::ExprUnaryOp;
use ruff_python_ast::Number;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::StmtIf;
use ruff_python_ast::StmtRaise;
use ruff_python_ast::StmtWhile;
use ruff_python_ast::UnaryOp;

use crate::context::detect_split_var;
use crate::context::token_delegated_to_helper;
use crate::registry::RegistrationKind;
use crate::types::ArgumentCountConstraint;
use crate::types::ExtractedArg;
use crate::types::ExtractedArgKind;
use crate::types::KnownOptions;
use crate::types::RequiredKeyword;
use crate::types::TagRule;

/// Extract validation rules from a tag's compile function.
///
/// Walks the function body looking for `raise TemplateSyntaxError(...)` statements,
/// extracts guard conditions, and represents them as structured `TagRule` data.
///
/// Uses the dynamically-detected split variable name (from `detect_split_var`)
/// to interpret comparisons like `len(bits) < 4` — never hardcodes `bits`.
///
/// `module_funcs` provides all function definitions in the same module, used
/// to resolve helper functions that the compile function may delegate to
/// (e.g., `parse_tag(token, parser)` calling `token.split_contents()` internally).
#[must_use]
pub fn extract_tag_rule(
    func: &StmtFunctionDef,
    kind: RegistrationKind,
    module_funcs: &[&StmtFunctionDef],
) -> TagRule {
    match kind {
        RegistrationKind::SimpleTag | RegistrationKind::InclusionTag => {
            extract_parse_bits_rule(func)
        }
        RegistrationKind::Tag | RegistrationKind::SimpleBlockTag => {
            extract_compile_function_rule(func, module_funcs)
        }
        RegistrationKind::Filter => TagRule {
            arg_constraints: Vec::new(),
            required_keywords: Vec::new(),
            known_options: None,
            extracted_args: Vec::new(),
        },
    }
}

/// Extract rules from a `@register.tag` or `@register.simple_block_tag` compile function.
///
/// Scans for `raise TemplateSyntaxError(...)` guarded by conditions on the
/// split-contents variable.
fn extract_compile_function_rule(
    func: &StmtFunctionDef,
    module_funcs: &[&StmtFunctionDef],
) -> TagRule {
    let split_var = detect_split_var(func);

    // If no direct split_var is found, check if token is delegated to a
    // helper function that calls split_contents() internally. In that case,
    // the compile function's variables (e.g., `args`, `kwargs`) have
    // transformed semantics and can't be interpreted with split_contents()
    // index rules. Skip constraint extraction to avoid false positives.
    if split_var.is_none() && token_delegated_to_helper(func, module_funcs) {
        return TagRule {
            arg_constraints: Vec::new(),
            required_keywords: Vec::new(),
            known_options: None,
            extracted_args: Vec::new(),
        };
    }

    let mut arg_constraints = Vec::new();
    let mut required_keywords = Vec::new();
    let mut known_options = None;

    // Walk the function body for if-statements guarding TemplateSyntaxError
    extract_from_body(
        &func.body,
        split_var.as_deref(),
        &mut arg_constraints,
        &mut required_keywords,
    );

    // Look for while-loop option parsing
    if let Some(opts) = extract_option_loop(&func.body) {
        known_options = Some(opts);
    }

    // Extract argument names from AST analysis (tuple unpacking, indexed access)
    let extracted_args = extract_args_from_compile_function(
        &func.body,
        split_var.as_deref(),
        &required_keywords,
        &arg_constraints,
    );

    TagRule {
        arg_constraints,
        required_keywords,
        known_options,
        extracted_args,
    }
}

/// Extract rules from a `simple_tag` or `inclusion_tag` function signature.
///
/// These tags use Django's `parse_bits` for argument validation, so we derive
/// constraints from the function signature (required params, optional params,
/// *args, **kwargs).
fn extract_parse_bits_rule(func: &StmtFunctionDef) -> TagRule {
    let params = &func.parameters;

    // Determine if `takes_context` is enabled (from decorator)
    let takes_context = has_takes_context(func);

    // Drop `context` parameter if takes_context
    let skip = usize::from(takes_context);

    // In ruff's AST, each ParameterWithDefault has its own `default` field
    let effective_params: Vec<&ruff_python_ast::ParameterWithDefault> =
        params.args.iter().skip(skip).collect();

    // Count defaults — params with `default.is_some()` have defaults
    let num_defaults = effective_params
        .iter()
        .filter(|p| p.default.is_some())
        .count();
    let num_required = effective_params.len().saturating_sub(num_defaults);

    let has_varargs = params.vararg.is_some();
    let has_kwargs = params.kwarg.is_some();

    let mut arg_constraints = Vec::new();

    if !has_varargs {
        if num_required > 0 {
            arg_constraints.push(ArgumentCountConstraint::Min(num_required + 1));
        }
        if !has_kwargs {
            let max_positional = effective_params.len();
            let kwonly_count = params.kwonlyargs.len();
            arg_constraints.push(ArgumentCountConstraint::Max(
                max_positional + kwonly_count + 1,
            ));
        }
    } else if num_required > 0 {
        arg_constraints.push(ArgumentCountConstraint::Min(num_required + 1));
    }

    // Extract argument structure from function parameters
    let mut extracted_args = Vec::new();
    for (i, param) in effective_params.iter().enumerate() {
        let name = param.parameter.name.to_string();
        let required = param.default.is_none();
        extracted_args.push(ExtractedArg {
            name,
            required,
            kind: ExtractedArgKind::Variable,
            position: i,
        });
    }

    // Add *args if present
    if has_varargs {
        if let Some(vararg) = &params.vararg {
            extracted_args.push(ExtractedArg {
                name: vararg.name.to_string(),
                required: false,
                kind: ExtractedArgKind::VarArgs,
                position: effective_params.len(),
            });
        }
    }

    // Add keyword-only args
    for (i, kwonly) in params.kwonlyargs.iter().enumerate() {
        let name = kwonly.parameter.name.to_string();
        let required = kwonly.default.is_none();
        extracted_args.push(ExtractedArg {
            name,
            required,
            kind: ExtractedArgKind::Keyword,
            position: effective_params.len() + usize::from(has_varargs) + i,
        });
    }

    // simple_tag/inclusion_tag auto-adds `as varname` support
    let as_position = extracted_args.len();
    extracted_args.push(ExtractedArg {
        name: "as".to_string(),
        required: false,
        kind: ExtractedArgKind::Literal("as".to_string()),
        position: as_position,
    });
    extracted_args.push(ExtractedArg {
        name: "varname".to_string(),
        required: false,
        kind: ExtractedArgKind::Variable,
        position: as_position + 1,
    });

    TagRule {
        arg_constraints,
        required_keywords: Vec::new(),
        known_options: None,
        extracted_args,
    }
}

/// Extract argument names from a manual `@register.tag` compile function.
///
/// Attempts to reconstruct argument structure from:
/// 1. Tuple unpacking: `tag_name, item, _in, iterable = bits`
/// 2. Indexed access: `format_string = bits[1]`
/// 3. `RequiredKeyword` positions give literal positions
/// 4. Falls back to generic `arg1`, `arg2` names
fn extract_args_from_compile_function(
    body: &[Stmt],
    split_var: Option<&str>,
    required_keywords: &[RequiredKeyword],
    arg_constraints: &[ArgumentCountConstraint],
) -> Vec<ExtractedArg> {
    let mut args: Vec<ExtractedArg> = Vec::new();

    // Try tuple unpacking first: `tag_name, item, _in, iterable = bits`
    if let Some(names) = find_tuple_unpacking(body, split_var) {
        // First element is always the tag name, skip it
        for (i, name) in names.iter().enumerate().skip(1) {
            let kind =
                find_keyword_kind_at(required_keywords, i).unwrap_or(ExtractedArgKind::Variable);

            args.push(ExtractedArg {
                name: name.clone(),
                required: true,
                kind,
                position: i - 1, // 0-based, excluding tag name
            });
        }
        return args;
    }

    // Try indexed access: `format_string = bits[1]`
    let indexed = find_indexed_access(body, split_var);
    if !indexed.is_empty() {
        let max_index = indexed.iter().map(|(idx, _)| *idx).max().unwrap_or(0);
        for pos in 1..=max_index {
            if let Some(kind) = find_keyword_kind_at(required_keywords, pos) {
                if let ExtractedArgKind::Literal(ref val) = kind {
                    args.push(ExtractedArg {
                        name: val.clone(),
                        required: true,
                        kind,
                        position: pos - 1,
                    });
                }
            } else if let Some((_, name)) = indexed.iter().find(|(idx, _)| *idx == pos) {
                args.push(ExtractedArg {
                    name: name.clone(),
                    required: true,
                    kind: ExtractedArgKind::Variable,
                    position: pos - 1,
                });
            } else {
                args.push(ExtractedArg {
                    name: format!("arg{pos}"),
                    required: true,
                    kind: ExtractedArgKind::Variable,
                    position: pos - 1,
                });
            }
        }
        return args;
    }

    // Fall back to generic names based on arg constraints
    if let Some(count) = infer_arg_count(arg_constraints) {
        // count includes tag name (split_contents indices), so args = count - 1
        for pos in 1..count {
            if let Some(kind) = find_keyword_kind_at(required_keywords, pos) {
                if let ExtractedArgKind::Literal(ref val) = kind {
                    args.push(ExtractedArg {
                        name: val.clone(),
                        required: true,
                        kind,
                        position: pos - 1,
                    });
                }
            } else {
                args.push(ExtractedArg {
                    name: format!("arg{pos}"),
                    required: true,
                    kind: ExtractedArgKind::Variable,
                    position: pos - 1,
                });
            }
        }
    }

    args
}

/// Check if a `RequiredKeyword` exists at the given position, returning
/// the `Literal` kind if so.
fn find_keyword_kind_at(
    required_keywords: &[RequiredKeyword],
    position: usize,
) -> Option<ExtractedArgKind> {
    let pos_i64 = i64::try_from(position).ok()?;
    required_keywords
        .iter()
        .find(|rk| rk.position == pos_i64)
        .map(|rk| ExtractedArgKind::Literal(rk.value.clone()))
}

/// Find tuple unpacking of the split variable: `tag_name, x, y = bits`
fn find_tuple_unpacking(body: &[Stmt], split_var: Option<&str>) -> Option<Vec<String>> {
    for stmt in body {
        if let Stmt::Assign(assign) = stmt {
            // Check if RHS is the split variable
            if !is_split_var_name(&assign.value, split_var) {
                continue;
            }
            // Check if LHS is a tuple
            if assign.targets.len() == 1 {
                if let Expr::Tuple(tuple) = &assign.targets[0] {
                    let names: Vec<String> = tuple
                        .elts
                        .iter()
                        .filter_map(|elt| {
                            if let Expr::Name(ExprName { id, .. }) = elt {
                                Some(id.to_string())
                            } else if let Expr::Starred(_) = elt {
                                // `*rest` — skip starred for tuple unpacking
                                None
                            } else {
                                None
                            }
                        })
                        .collect();
                    if names.len() >= 2 {
                        return Some(names);
                    }
                }
            }
        }
    }
    None
}

/// Check if an expression is the split variable name.
fn is_split_var_name(expr: &Expr, split_var: Option<&str>) -> bool {
    let Expr::Name(ExprName { id, .. }) = expr else {
        return false;
    };
    let var_name = id.as_str();
    if let Some(sv) = split_var {
        return var_name == sv;
    }
    matches!(var_name, "bits" | "args" | "parts" | "tokens")
}

/// Find indexed access patterns: `name = bits[N]` → (N, name)
fn find_indexed_access(body: &[Stmt], split_var: Option<&str>) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    for stmt in body {
        if let Stmt::Assign(assign) = stmt {
            if assign.targets.len() != 1 {
                continue;
            }
            let Expr::Name(ExprName {
                id: target_name, ..
            }) = &assign.targets[0]
            else {
                continue;
            };
            if let Expr::Subscript(ExprSubscript { value, slice, .. }) = assign.value.as_ref() {
                if is_split_var_name(value, split_var) {
                    if let Some(idx) = extract_int_constant(slice) {
                        if idx > 0 {
                            results.push((idx, target_name.to_string()));
                        }
                    }
                }
            }
        }
    }
    results
}

/// Infer the expected argument count from constraints.
///
/// Returns the exact count if determinable (for generating generic arg names).
fn infer_arg_count(constraints: &[ArgumentCountConstraint]) -> Option<usize> {
    for c in constraints {
        if let ArgumentCountConstraint::Exact(n) = c {
            return Some(*n);
        }
    }
    // If only Min constraint, use that as a hint for minimum arg positions
    for c in constraints {
        if let ArgumentCountConstraint::Min(n) = c {
            return Some(*n);
        }
    }
    None
}

/// Check if a function's decorator includes `takes_context=True`.
fn has_takes_context(func: &StmtFunctionDef) -> bool {
    for decorator in &func.decorator_list {
        if let Expr::Call(ExprCall { arguments, .. }) = &decorator.expression {
            for kw in &arguments.keywords {
                if let Some(arg) = &kw.arg {
                    if arg.as_str() == "takes_context" && is_true_constant(&kw.value) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check if an expression is a boolean `True` constant.
fn is_true_constant(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::BooleanLiteral(lit) if lit.value
    )
}

/// Recursively walk statements looking for `if condition: raise TemplateSyntaxError`.
fn extract_from_body(
    body: &[Stmt],
    split_var: Option<&str>,
    arg_constraints: &mut Vec<ArgumentCountConstraint>,
    required_keywords: &mut Vec<RequiredKeyword>,
) {
    for stmt in body {
        if let Stmt::If(if_stmt) = stmt {
            extract_from_if(if_stmt, split_var, arg_constraints, required_keywords);
        }
    }
}

/// Extract rules from an if-statement that guards a `raise TemplateSyntaxError(...)`.
fn extract_from_if(
    if_stmt: &StmtIf,
    split_var: Option<&str>,
    arg_constraints: &mut Vec<ArgumentCountConstraint>,
    required_keywords: &mut Vec<RequiredKeyword>,
) {
    // Check if the if-body contains a TemplateSyntaxError raise
    if body_raises_template_syntax_error(&if_stmt.body) {
        extract_from_condition(&if_stmt.test, split_var, arg_constraints, required_keywords);
    }

    // Recurse into the if-body for nested if statements
    extract_from_body(&if_stmt.body, split_var, arg_constraints, required_keywords);

    // Recurse into elif/else clauses
    for clause in &if_stmt.elif_else_clauses {
        if body_raises_template_syntax_error(&clause.body) {
            if let Some(test) = &clause.test {
                extract_from_condition(test, split_var, arg_constraints, required_keywords);
            }
        }
        extract_from_body(&clause.body, split_var, arg_constraints, required_keywords);
    }
}

/// Extract constraints from a condition expression.
fn extract_from_condition(
    condition: &Expr,
    split_var: Option<&str>,
    arg_constraints: &mut Vec<ArgumentCountConstraint>,
    required_keywords: &mut Vec<RequiredKeyword>,
) {
    match condition {
        // `or`: `len(bits) != 3 or bits[1] != "as"`
        // Error when either is true → valid requires NOT(A) AND NOT(B)
        // → each sub-condition is an independent constraint.
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::Or,
            values,
            ..
        }) => {
            for value in values {
                extract_from_condition(value, split_var, arg_constraints, required_keywords);
            }
        }
        // `and`: `len(tokens) > 1 and tokens[1] != "as"` (guard pattern)
        // Error when both true → valid when either is false. The length
        // check is typically a guard protecting an index access, not a real
        // constraint. Extract keywords (useful for completions) but discard
        // length constraints (they're protective, not prescriptive).
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::And,
            values,
            ..
        }) => {
            let mut discarded = Vec::new();
            for value in values {
                extract_from_condition(value, split_var, &mut discarded, required_keywords);
            }
        }
        // Comparison: `len(bits) < 4` or `bits[2] != "as"`
        Expr::Compare(compare) => {
            extract_from_compare(compare, split_var, arg_constraints, required_keywords);
        }
        // Negation: `not 2 <= len(bits) <= 4`
        Expr::UnaryOp(ExprUnaryOp {
            op: UnaryOp::Not,
            operand,
            ..
        }) => {
            if let Expr::Compare(compare) = operand.as_ref() {
                extract_from_negated_compare(
                    compare,
                    split_var,
                    arg_constraints,
                    required_keywords,
                );
            }
        }
        _ => {}
    }
}

/// Extract constraints from a comparison expression.
fn extract_from_compare(
    compare: &ExprCompare,
    split_var: Option<&str>,
    arg_constraints: &mut Vec<ArgumentCountConstraint>,
    required_keywords: &mut Vec<RequiredKeyword>,
) {
    if compare.ops.is_empty() || compare.comparators.is_empty() {
        return;
    }

    // Handle range comparisons: `2 <= len(bits) <= 4` (but inverted since error guard)
    if compare.ops.len() == 2 && compare.comparators.len() == 2 {
        if let Some(constraint) = extract_range_constraint(compare, split_var, false) {
            arg_constraints.extend(constraint);
            return;
        }
    }

    let op = &compare.ops[0];
    let comparator = &compare.comparators[0];
    let left = &compare.left;

    // len(var) comparisons
    if is_len_call_on(left, split_var).is_some() {
        if let Some(n) = extract_int_constant(comparator) {
            let constraint = match op {
                // `len(bits) != N` → error when len != N → valid when len == N
                CmpOp::NotEq => Some(ArgumentCountConstraint::Exact(n)),
                // `len(bits) < N` → error when len < N → needs at least N
                CmpOp::Lt => Some(ArgumentCountConstraint::Min(n)),
                // `len(bits) <= N` → error when len <= N → needs at least N+1
                CmpOp::LtE => Some(ArgumentCountConstraint::Min(n + 1)),
                // `len(bits) > N` → error when len > N → needs at most N
                CmpOp::Gt => Some(ArgumentCountConstraint::Max(n)),
                // `len(bits) >= N` → error when len >= N → needs at most N-1
                CmpOp::GtE if n > 0 => Some(ArgumentCountConstraint::Max(n - 1)),
                // `len(bits) == N` → error when len IS N → valid when len != N
                // We can't express "not exactly N" as a constraint, skip.
                // CmpOp::Eq and all other operators fall through here.
                _ => None,
            };
            if let Some(c) = constraint {
                arg_constraints.push(c);
            }
            return;
        }

        // `len(bits) not in (2, 3, 4)` → valid counts are 2, 3, 4
        if matches!(op, CmpOp::NotIn) {
            if let Some(values) = extract_int_collection(comparator) {
                arg_constraints.push(ArgumentCountConstraint::OneOf(values));
                return;
            }
        }

        // `len(bits) in (2, 3)` → error when IN set → valid when NOT in set
        // We can't express "not in set" as a constraint, skip
        return;
    }

    // Check if comparator side has len() — `N < len(bits)` (reversed)
    if is_len_call_on(comparator, split_var).is_some() {
        if let Some(n) = extract_int_constant(left) {
            let constraint = match op {
                // `N < len(bits)` → error when N < len → needs at most N
                CmpOp::Lt => Some(ArgumentCountConstraint::Max(n)),
                // `N <= len(bits)` → error when N <= len → needs at most N-1
                CmpOp::LtE if n > 0 => Some(ArgumentCountConstraint::Max(n - 1)),
                // `N > len(bits)` → error when N > len → needs at least N
                CmpOp::Gt => Some(ArgumentCountConstraint::Min(n)),
                // `N >= len(bits)` → error when N >= len → needs at least N+1
                CmpOp::GtE => Some(ArgumentCountConstraint::Min(n + 1)),
                _ => None,
            };
            if let Some(c) = constraint {
                arg_constraints.push(c);
            }
        }
        return;
    }

    // Subscript comparisons: `bits[N] != "keyword"` or `bits[N] == "keyword"`
    if let Expr::Subscript(ExprSubscript { value, slice, .. }) = left.as_ref() {
        if is_split_var_name(value, split_var) {
            if let Some(position) = extract_signed_int_constant(slice) {
                if let Some(keyword) = extract_string_constant(comparator) {
                    // `bits[N] != "keyword"` → keyword required at position N
                    // `bits[N] == "keyword"` in error guard is unusual but still record it
                    required_keywords.push(RequiredKeyword {
                        position,
                        value: keyword,
                    });
                }
            }
        }
    }
}

/// Extract constraints from a negated comparison: `not (2 <= len(bits) <= 4)`.
fn extract_from_negated_compare(
    compare: &ExprCompare,
    split_var: Option<&str>,
    arg_constraints: &mut Vec<ArgumentCountConstraint>,
    _required_keywords: &mut Vec<RequiredKeyword>,
) {
    // Range: `not (2 <= len(bits) <= 4)` → need 2..=4
    if compare.ops.len() == 2 && compare.comparators.len() == 2 {
        if let Some(constraint) = extract_range_constraint(compare, split_var, true) {
            arg_constraints.extend(constraint);
            return;
        }
    }

    // Simple: `not len(bits) == 3` → need exactly 3
    if compare.ops.len() == 1 && compare.comparators.len() == 1 {
        let op = &compare.ops[0];
        let left = &compare.left;
        let comparator = &compare.comparators[0];

        if is_len_call_on(left, split_var).is_some() {
            if let Some(n) = extract_int_constant(comparator) {
                let constraint = match op {
                    // `not len(bits) == 3` → need exactly 3
                    CmpOp::Eq => Some(ArgumentCountConstraint::Exact(n)),
                    // `not len(bits) < N` → need max N-1
                    CmpOp::Lt if n > 0 => Some(ArgumentCountConstraint::Max(n - 1)),
                    // `not len(bits) > N` → need min N+1
                    CmpOp::Gt => Some(ArgumentCountConstraint::Min(n + 1)),
                    _ => None,
                };
                if let Some(c) = constraint {
                    arg_constraints.push(c);
                }
            }
        }
    }
}

/// Extract range constraint from `CONST <=/<  len(var) <=/<  CONST`.
///
/// If `negated` is true, the range represents valid values (the error fires when
/// the value is OUTSIDE the range). If `negated` is false, the range represents
/// error values.
fn extract_range_constraint(
    compare: &ExprCompare,
    split_var: Option<&str>,
    negated: bool,
) -> Option<Vec<ArgumentCountConstraint>> {
    if compare.ops.len() != 2 || compare.comparators.len() != 2 {
        return None;
    }

    let mid = &compare.comparators[0];
    is_len_call_on(mid, split_var)?;

    let lower = extract_int_constant(&compare.left)?;
    let upper = extract_int_constant(&compare.comparators[1])?;

    let op1 = &compare.ops[0];
    let op2 = &compare.ops[1];

    // Only handle <= and < operators
    if !matches!(op1, CmpOp::Lt | CmpOp::LtE) || !matches!(op2, CmpOp::Lt | CmpOp::LtE) {
        return None;
    }

    let min_val = if matches!(op1, CmpOp::LtE) {
        lower
    } else {
        lower + 1
    };
    let max_val = if matches!(op2, CmpOp::LtE) {
        upper
    } else {
        upper - 1
    };

    if negated {
        // `not (min <= len <= max)` → valid range is min..=max
        Some(vec![
            ArgumentCountConstraint::Min(min_val),
            ArgumentCountConstraint::Max(max_val),
        ])
    } else {
        // `min <= len <= max` in error guard means this range triggers error
        // This is unusual — record the range bounds anyway
        Some(vec![
            ArgumentCountConstraint::Min(min_val),
            ArgumentCountConstraint::Max(max_val),
        ])
    }
}

/// Check if an expression is `len(var)` where `var` matches the split variable.
///
/// Returns the variable name if matched.
fn is_len_call_on<'a>(expr: &'a Expr, split_var: Option<&str>) -> Option<&'a str> {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return None;
    };
    let Expr::Name(ExprName { id, .. }) = func.as_ref() else {
        return None;
    };
    if id.as_str() != "len" {
        return None;
    }
    if arguments.args.len() != 1 {
        return None;
    }
    let Expr::Name(ExprName { id: var_id, .. }) = &arguments.args[0] else {
        return None;
    };

    let var_name = var_id.as_str();

    // If we have a known split variable, only match that
    if let Some(sv) = split_var {
        if var_name == sv {
            return Some(var_name);
        }
        return None;
    }

    // Without a known split variable, match common names
    if matches!(var_name, "bits" | "args" | "parts" | "tokens") {
        return Some(var_name);
    }

    None
}

/// Extract an integer constant from an expression.
fn extract_int_constant(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::NumberLiteral(lit) => match &lit.value {
            Number::Int(int_val) => {
                let val = int_val.as_u64()?;
                usize::try_from(val).ok()
            }
            _ => None,
        },
        Expr::UnaryOp(ExprUnaryOp {
            op: UnaryOp::USub,
            operand,
            ..
        }) => {
            // Negative numbers — we can't represent them as usize, skip
            let _ = operand;
            None
        }
        _ => None,
    }
}

/// Extract a signed integer constant from an expression (for subscript indices).
fn extract_signed_int_constant(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::NumberLiteral(lit) => match &lit.value {
            Number::Int(int_val) => int_val.as_i64(),
            _ => None,
        },
        Expr::UnaryOp(ExprUnaryOp {
            op: UnaryOp::USub,
            operand,
            ..
        }) => {
            if let Expr::NumberLiteral(lit) = operand.as_ref() {
                if let Number::Int(int_val) = &lit.value {
                    return int_val.as_i64().map(|v| -v);
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract a string constant from an expression.
fn extract_string_constant(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = expr {
        return Some(value.to_str().to_string());
    }
    None
}

/// Extract a collection of integer constants from a tuple, list, or set.
fn extract_int_collection(expr: &Expr) -> Option<Vec<usize>> {
    let elements = match expr {
        Expr::Tuple(t) => &t.elts,
        Expr::List(l) => &l.elts,
        Expr::Set(s) => &s.elts,
        _ => return None,
    };

    let mut values = Vec::new();
    for elt in elements {
        values.push(extract_int_constant(elt)?);
    }
    Some(values)
}

/// Check if a statement body contains `raise TemplateSyntaxError(...)`.
fn body_raises_template_syntax_error(body: &[Stmt]) -> bool {
    for stmt in body {
        if let Stmt::Raise(StmtRaise { exc: Some(exc), .. }) = stmt {
            if is_template_syntax_error_call(exc) {
                return true;
            }
        }
    }
    false
}

/// Check if an expression is a `TemplateSyntaxError(...)` call.
fn is_template_syntax_error_call(expr: &Expr) -> bool {
    let Expr::Call(ExprCall { func, .. }) = expr else {
        return false;
    };
    match func.as_ref() {
        Expr::Name(ExprName { id, .. }) => id.as_str() == "TemplateSyntaxError",
        Expr::Attribute(ExprAttribute { attr, .. }) => attr.as_str() == "TemplateSyntaxError",
        _ => false,
    }
}

/// Extract option-loop metadata from while loops in the function body.
///
/// Detects patterns like:
/// ```python
/// while remaining_bits:
///     option = remaining_bits.pop(0)
///     if option == "with":
///         ...
///     elif option == "only":
///         ...
///     else:
///         raise TemplateSyntaxError(...)
/// ```
fn extract_option_loop(body: &[Stmt]) -> Option<KnownOptions> {
    for stmt in body {
        if let Stmt::While(while_stmt) = stmt {
            if let Some(opts) = extract_from_while(while_stmt) {
                return Some(opts);
            }
        }
    }
    None
}

/// Extract options from a while loop.
fn extract_from_while(while_stmt: &StmtWhile) -> Option<KnownOptions> {
    // The loop variable should be a name (e.g., `remaining_bits`, `bits`, `args`)
    let Expr::Name(ExprName { id: loop_var, .. }) = &*while_stmt.test else {
        return None;
    };
    let loop_var = loop_var.as_str();

    // Look for `option = loop_var.pop(0)` in the body
    let option_var = find_option_var(&while_stmt.body, loop_var)?;

    // Now scan if-statements for option checks
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

/// Find the variable assigned from `loop_var.pop(0)`.
fn find_option_var(body: &[Stmt], loop_var: &str) -> Option<String> {
    for stmt in body {
        if let Stmt::Assign(assign) = stmt {
            if assign.targets.len() == 1 {
                if let Expr::Name(ExprName { id, .. }) = &assign.targets[0] {
                    if is_pop_zero(&assign.value, loop_var) {
                        return Some(id.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Check if an expression is `var.pop(0)`.
fn is_pop_zero(expr: &Expr, var_name: &str) -> bool {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return false;
    };
    let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = func.as_ref()
    else {
        return false;
    };
    if attr.as_str() != "pop" {
        return false;
    }
    let Expr::Name(ExprName { id, .. }) = obj.as_ref() else {
        return false;
    };
    if id.as_str() != var_name {
        return false;
    }
    if arguments.args.len() != 1 {
        return false;
    }
    extract_int_constant(&arguments.args[0]) == Some(0)
}

/// Extract option names from if-elif-else chains checking the option variable.
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
        // Continue into elif/else for more checks
    } else if let Some(opt_name) = extract_option_equality(&if_stmt.test, option_var) {
        // `if option == "with": ...`
        if !values.contains(&opt_name) {
            values.push(opt_name);
        }
    }

    // Process elif/else clauses
    for clause in &if_stmt.elif_else_clauses {
        if let Some(test) = &clause.test {
            // elif
            if is_duplicate_check(test, option_var) {
                *allow_duplicates = false;
            } else if let Some(opt_name) = extract_option_equality(test, option_var) {
                if !values.contains(&opt_name) {
                    values.push(opt_name);
                }
            }
        } else {
            // else branch — if it raises TemplateSyntaxError, unknown options are rejected
            if body_raises_template_syntax_error(&clause.body) {
                *rejects_unknown = true;
            }
        }
    }
}

/// Check if a condition is a duplicate check: `option in seen_set`.
fn is_duplicate_check(test: &Expr, option_var: &str) -> bool {
    let Expr::Compare(compare) = test else {
        return false;
    };
    if compare.ops.len() != 1 || compare.comparators.len() != 1 {
        return false;
    }
    if !matches!(compare.ops[0], CmpOp::In) {
        return false;
    }
    let Expr::Name(ExprName { id, .. }) = compare.left.as_ref() else {
        return false;
    };
    if id.as_str() != option_var {
        return false;
    }
    // The comparator should be a name (the seen-set variable)
    matches!(compare.comparators[0], Expr::Name(_))
}

/// Extract option name from `option == "name"`.
fn extract_option_equality(test: &Expr, option_var: &str) -> Option<String> {
    let Expr::Compare(compare) = test else {
        return None;
    };
    if compare.ops.len() != 1 || compare.comparators.len() != 1 {
        return None;
    }
    if !matches!(compare.ops[0], CmpOp::Eq) {
        return None;
    }
    let Expr::Name(ExprName { id, .. }) = compare.left.as_ref() else {
        return None;
    };
    if id.as_str() != option_var {
        return None;
    }
    extract_string_constant(&compare.comparators[0])
}

#[cfg(test)]
mod tests {
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

    // =========================================================================
    // Argument count checks
    // =========================================================================

    #[test]
    fn len_less_than() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError('too few args')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    #[test]
    fn len_less_than_equal() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) <= 1:
        raise TemplateSyntaxError('too few args')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    #[test]
    fn len_greater_than() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) > 4:
        raise TemplateSyntaxError('too many args')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Max(4)]);
    }

    #[test]
    fn len_greater_than_equal() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) >= 5:
        raise TemplateSyntaxError('too many args')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Max(4)]);
    }

    #[test]
    fn len_not_equal() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError('exactly 3 args required')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(3)]
        );
    }

    #[test]
    fn len_not_in_set() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) not in (2, 3, 4):
        raise TemplateSyntaxError('2, 3, or 4 args')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::OneOf(vec![2, 3, 4])]
        );
    }

    #[test]
    fn negated_range() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if not (2 <= len(bits) <= 4):
        raise TemplateSyntaxError('2 to 4 args')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.arg_constraints,
            vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(4),
            ]
        );
    }

    // =========================================================================
    // Keyword position checks
    // =========================================================================

    #[test]
    fn keyword_at_position() {
        let source = r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[1] != "as":
        raise TemplateSyntaxError('second arg must be "as"')
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.required_keywords,
            vec![RequiredKeyword {
                position: 1,
                value: "as".to_string()
            }]
        );
    }

    #[test]
    fn keyword_at_position_equal() {
        // `bits[2] == "invalid"` in error guard — still records keyword
        let source = r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[2] == "invalid":
        raise TemplateSyntaxError('invalid keyword')
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.required_keywords,
            vec![RequiredKeyword {
                position: 2,
                value: "invalid".to_string()
            }]
        );
    }

    // =========================================================================
    // Compound conditions
    // =========================================================================

    #[test]
    fn compound_or_condition() {
        let source = r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3 or bits[1] != "as":
        raise TemplateSyntaxError('expected: tag as name')
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(3)]
        );
        assert_eq!(
            rule.required_keywords,
            vec![RequiredKeyword {
                position: 1,
                value: "as".to_string()
            }]
        );
    }

    // =========================================================================
    // Non-`bits` variable names
    // =========================================================================

    #[test]
    fn uses_args_variable() {
        let source = r"
def do_tag(parser, token):
    args = token.split_contents()
    if len(args) < 3:
        raise TemplateSyntaxError('too few')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Min(3)]);
    }

    #[test]
    fn uses_parts_variable() {
        let source = r"
def do_tag(parser, token):
    parts = token.split_contents()
    if len(parts) != 2:
        raise TemplateSyntaxError('exactly 2')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
    }

    #[test]
    fn uses_custom_variable() {
        let source = r"
def do_tag(parser, token):
    my_tokens = token.split_contents()
    if len(my_tokens) < 2:
        raise TemplateSyntaxError('too few')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        // Should still extract because detect_split_var returns "my_tokens"
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    // =========================================================================
    // Option loop extraction
    // =========================================================================

    #[test]
    fn option_loop_basic() {
        let source = r#"
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
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert!(opts.rejects_unknown);
    }

    #[test]
    fn option_loop_with_duplicate_check() {
        let source = r#"
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
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["silent".to_string(), "cache".to_string()]);
        assert!(opts.rejects_unknown);
        assert!(!opts.allow_duplicates);
    }

    #[test]
    fn option_loop_allows_unknown() {
        let source = r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while remaining:
        option = remaining.pop(0)
        if option == "noescape":
            pass
        elif option == "trimmed":
            pass
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(
            opts.values,
            vec!["noescape".to_string(), "trimmed".to_string()]
        );
        assert!(!opts.rejects_unknown);
        assert!(opts.allow_duplicates);
    }

    // =========================================================================
    // simple_tag / inclusion_tag parameter analysis
    // =========================================================================

    #[test]
    fn simple_tag_no_params() {
        let source = r"
@register.simple_tag
def now():
    return datetime.now()
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::SimpleTag, &[]);
        // No required params, no varargs
        assert!(rule
            .arg_constraints
            .iter()
            .all(|c| matches!(c, ArgumentCountConstraint::Max(_))));
    }

    #[test]
    fn simple_tag_required_params() {
        let source = r"
@register.simple_tag
def greeting(name, title):
    return f'Hello {title} {name}'
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::SimpleTag, &[]);
        assert!(rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Min(3)));
    }

    #[test]
    fn simple_tag_with_defaults() {
        let source = r#"
@register.simple_tag
def greeting(name, title="Mr"):
    return f'Hello {title} {name}'
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::SimpleTag, &[]);
        // 1 required param (name) + tag name = Min(2)
        assert!(rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Min(2)));
    }

    #[test]
    fn simple_tag_with_varargs() {
        let source = r"
@register.simple_tag
def concat(*args):
    return ''.join(str(a) for a in args)
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::SimpleTag, &[]);
        // With *args, no max constraint
        assert!(!rule
            .arg_constraints
            .iter()
            .any(|c| matches!(c, ArgumentCountConstraint::Max(_))));
    }

    #[test]
    fn simple_tag_takes_context() {
        let source = r"
@register.simple_tag(takes_context=True)
def show_user(context, name):
    return f'{context} {name}'
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::SimpleTag, &[]);
        // `context` is skipped, only `name` is required → Min(2)
        assert!(rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Min(2)));
    }

    // =========================================================================
    // Multiple raise statements
    // =========================================================================

    #[test]
    fn multiple_raises() {
        let source = r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError('too few')
    if len(bits) > 5:
        raise TemplateSyntaxError('too many')
    if bits[1] != "as":
        raise TemplateSyntaxError('missing as')
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.arg_constraints,
            vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(5),
            ]
        );
        assert_eq!(
            rule.required_keywords,
            vec![RequiredKeyword {
                position: 1,
                value: "as".to_string()
            }]
        );
    }

    // =========================================================================
    // Edge cases
    // =========================================================================

    #[test]
    fn no_split_contents() {
        let source = r"
def do_tag(parser, token):
    name = token.contents
    return SomeNode(name)
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert!(rule.arg_constraints.is_empty());
        assert!(rule.required_keywords.is_empty());
        assert!(rule.known_options.is_none());
    }

    #[test]
    fn no_raise_in_function() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    return SomeNode(bits)
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert!(rule.arg_constraints.is_empty());
        assert!(rule.required_keywords.is_empty());
    }

    #[test]
    fn raise_without_template_syntax_error() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise ValueError('not a TemplateSyntaxError')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        // Should NOT extract — the raise is not TemplateSyntaxError
        assert!(rule.arg_constraints.is_empty());
    }

    #[test]
    fn template_syntax_error_via_attribute() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise template.TemplateSyntaxError('error')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    #[test]
    fn filter_kind_returns_empty_rule() {
        let source = r"
def my_filter(value, arg):
    return value.replace(arg, '')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Filter, &[]);
        assert!(rule.arg_constraints.is_empty());
        assert!(rule.required_keywords.is_empty());
        assert!(rule.known_options.is_none());
    }

    #[test]
    fn reversed_comparison_n_less_than_len() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if 4 < len(bits):
        raise TemplateSyntaxError('too many args')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        // `4 < len(bits)` → error when len > 4 → Max(4)
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Max(4)]);
    }

    #[test]
    fn nested_if_raises() {
        let source = r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) >= 3:
        if bits[2] != "as":
            raise TemplateSyntaxError('expected as')
"#;
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.required_keywords,
            vec![RequiredKeyword {
                position: 2,
                value: "as".to_string()
            }]
        );
    }

    #[test]
    fn elif_branch_raises() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError('too few')
    elif len(bits) > 4:
        raise TemplateSyntaxError('too many')
";
        let func = parse_function(source);
        let rule = extract_tag_rule(&func, RegistrationKind::Tag, &[]);
        assert_eq!(
            rule.arg_constraints,
            vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(4),
            ]
        );
    }
}
