use std::ops::ControlFlow;

use ruff_python_ast::Arguments;
use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtIf;
use ruff_python_ast::StmtWhile;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::analysis::exceptions::direct_raise_exception;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::types::KnownOptions;
use crate::templates::tags::types::OptionRejection;

/// Info about a `bits.pop(...)` call for mutation tracking.
pub(super) struct PopInfo {
    /// The variable name being popped from (e.g., "bits")
    var_name: String,
    position: PopPosition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PopPosition {
    Front,
    Back,
    Untracked,
}

impl PopPosition {
    pub(super) fn from_arguments(args: &Arguments) -> Self {
        if !args.keywords.is_empty() {
            return Self::Untracked;
        }
        match args.args.as_ref() {
            [] => Self::Back,
            [arg] if arg.non_negative_integer() == Some(0) || arg.negative_integer() == Some(0) => {
                Self::Front
            }
            [arg] if arg.negative_integer() == Some(1) => Self::Back,
            _ => Self::Untracked,
        }
    }
}

/// Try to extract pop call info from an expression, without evaluating it.
pub(super) fn try_extract_pop_call(expr: &Expr) -> Option<PopInfo> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref() else {
        return None;
    };
    if attr.as_str() != "pop" {
        return None;
    }
    let var_name = value.name_target()?;

    Some(PopInfo {
        var_name: var_name.to_string(),
        position: PopPosition::from_arguments(&call.arguments),
    })
}

/// Apply the mutation side effect of a pop call to the environment.
pub(super) fn apply_pop_mutation(env: &mut Env, pop_info: &PopInfo) {
    env.mutate(&pop_info.var_name, |v| {
        if let AbstractValue::SplitResult(split) = v {
            match pop_info.position {
                PopPosition::Front => *split = split.after_pop_front(),
                PopPosition::Back => *split = split.after_pop_back(),
                PopPosition::Untracked => *v = AbstractValue::Unknown,
            }
        }
    });
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
pub(super) fn try_extract_option_loop(while_stmt: &StmtWhile, env: &Env) -> Option<KnownOptions> {
    // The loop test must be a simple name that resolves to SplitResult (or derivative)
    let loop_var = while_stmt.test.name_target()?;
    let loop_value = env.get(loop_var);
    if !matches!(
        loop_value,
        AbstractValue::SplitResult(_) | AbstractValue::Unknown
    ) {
        return None;
    }

    // Look for `option = loop_var.pop(0)` in the body
    let option_var = find_option_pop_var(&while_stmt.body, loop_var)?;

    // Scan if/elif/else chains for option value checks
    let mut options = KnownOptions {
        values: Vec::new(),
        duplicate_rejection: OptionRejection::NotDetected,
        unknown_rejection: OptionRejection::NotDetected,
    };

    for stmt in &while_stmt.body {
        if let Stmt::If(if_stmt) = stmt {
            extract_option_checks(if_stmt, &option_var, &mut options);
        }
    }

    if options.values.is_empty() {
        return None;
    }

    Some(options)
}

/// Find the variable assigned from `loop_var.pop(0)` in a while-loop body.
fn find_option_pop_var(body: &[Stmt], loop_var: &str) -> Option<String> {
    let mut option_var = None;
    walk_stmts(body, Recurse::Flat, |stmt| {
        if let Stmt::Assign(assign) = stmt
            && assign.targets.len() == 1
            && let Some(name) = assign.targets[0].name_target()
        {
            let is_pop_zero = try_extract_pop_call(&assign.value).is_some_and(|info| {
                info.var_name == loop_var && info.position == PopPosition::Front
            });
            if is_pop_zero {
                option_var = Some(name.to_string());
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    });
    option_var
}

/// Extract option names from if/elif/else chains checking the option variable.
fn extract_option_checks(if_stmt: &StmtIf, option_var: &str, options: &mut KnownOptions) {
    let clauses = std::iter::once((Some(if_stmt.test.as_ref()), if_stmt.body.as_slice())).chain(
        if_stmt
            .elif_else_clauses
            .iter()
            .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
    );

    for (test, body) in clauses {
        if let Some(test) = test {
            if is_duplicate_check(test, option_var) && direct_raise_exception(body).is_some() {
                options.duplicate_rejection = OptionRejection::Detected;
            } else if let Some(opt_name) = extract_option_equality(test, option_var)
                && !options.values.contains(&opt_name)
            {
                options.values.push(opt_name);
            }
        } else if direct_raise_exception(body).is_some() {
            options.unknown_rejection = OptionRejection::Detected;
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
    if left.name_target() != Some(option_var) {
        return false;
    }
    comparators[0].name_target().is_some()
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
    if left.name_target() != Some(option_var) {
        return None;
    }
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = &comparators[0] {
        return Some(value.to_str().to_string());
    }
    None
}
