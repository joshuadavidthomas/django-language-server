use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprFString;
use ruff_python_ast::FStringPart;
use ruff_python_ast::InterpolatedStringElement;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtReturn;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::Env;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::analysis::process_statements;
use crate::templates::tags::blocks::EndTagEvidence;
use crate::templates::tags::blocks::ExtractedBlockSpec;
use crate::templates::tags::blocks::is_parser_receiver;
use crate::templates::tags::blocks::is_tag_name_value;

/// Detect dynamic end-tag patterns: `parser.parse((f"end{tag_name}",))`.
///
/// Returns `SelfNamed` only when the dynamic expression is evidenced as the
/// compile function's own tag name. Other dynamic end-tag expressions remain
/// `Unknown` so callers don't synthesize a closer from convention alone.
pub(super) fn detect(
    body: &[Stmt],
    parser_var: &str,
    token_var: &str,
) -> Option<ExtractedBlockSpec> {
    let env = analyze_env(body, parser_var, token_var);
    let assignments = collect_simple_assignments(body);
    let end_tag = find_dynamic_end_parse_call(body, parser_var, &assignments, &env)?;
    Some(ExtractedBlockSpec {
        end_tag,
        intermediates: Vec::new(),
        opaque: false,
    })
}

fn analyze_env(body: &[Stmt], parser_var: &str, token_var: &str) -> Env {
    let mut env = Env::for_compile_function(parser_var, token_var);
    let mut ctx = CallContext {
        db: None,
        file: None,
    };
    let _result = process_statements(body, &mut env, &mut ctx);
    env
}

fn collect_simple_assignments(body: &[Stmt]) -> Vec<(String, &Expr)> {
    let mut assignments = Vec::new();
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        if let Stmt::Assign(StmtAssign { targets, value, .. }) = stmt
            && targets.len() == 1
            && let Some(name) = targets[0].name_target()
        {
            assignments.push((name.to_string(), value.as_ref()));
        }
        ControlFlow::Continue(())
    });
    assignments
}

fn find_dynamic_end_parse_call(
    body: &[Stmt],
    parser_var: &str,
    assignments: &[(String, &Expr)],
    env: &Env,
) -> Option<EndTagEvidence> {
    let mut found = None;
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        let evidence = match stmt {
            Stmt::Expr(expr_stmt) => {
                dynamic_end_parse_call_evidence(&expr_stmt.value, parser_var, assignments, env)
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                dynamic_end_parse_call_evidence(value, parser_var, assignments, env)
            }
            Stmt::Return(StmtReturn {
                value: Some(val), ..
            }) => dynamic_end_parse_call_evidence(val, parser_var, assignments, env),
            Stmt::FunctionDef(_)
            | Stmt::ClassDef(_)
            | Stmt::Return(_)
            | Stmt::Delete(_)
            | Stmt::TypeAlias(_)
            | Stmt::AugAssign(_)
            | Stmt::AnnAssign(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Raise(_)
            | Stmt::Try(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => None,
        };
        if let Some(evidence) = evidence {
            found = Some(evidence);
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    found
}

/// Check if an expression is `parser.parse((f"end{...}",))` or uses one
/// intermediate variable assigned from that dynamic expression.
fn dynamic_end_parse_call_evidence(
    expr: &Expr,
    parser_var: &str,
    assignments: &[(String, &Expr)],
    env: &Env,
) -> Option<EndTagEvidence> {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return None;
    };
    let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = func.as_ref()
    else {
        return None;
    };
    if attr.as_str() != "parse" {
        return None;
    }
    if !is_parser_receiver(obj, parser_var) {
        return None;
    }
    if arguments.args.is_empty() {
        return None;
    }

    let seq = &arguments.args[0];
    let elements = match seq {
        Expr::Tuple(t) => &t.elts,
        Expr::List(l) => &l.elts,
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Compare(_)
        | Expr::Call(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Attribute(_)
        | Expr::Subscript(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => return None,
    };

    for elt in elements {
        if let Some(evidence) = dynamic_end_expr_evidence(elt, assignments, env) {
            return Some(evidence);
        }
    }
    None
}

fn dynamic_end_expr_evidence(
    expr: &Expr,
    assignments: &[(String, &Expr)],
    env: &Env,
) -> Option<EndTagEvidence> {
    direct_dynamic_end_expr_evidence(expr, env).or_else(|| {
        let name = expr.name_target()?;
        let (_, value) = assignments
            .iter()
            .rev()
            .find(|(assigned, _)| assigned == name)?;
        match direct_dynamic_end_expr_evidence(value, env) {
            Some(EndTagEvidence::SelfNamed) => Some(EndTagEvidence::SelfNamed),
            Some(EndTagEvidence::Literal(_) | EndTagEvidence::Unknown) | None => None,
        }
    })
}

/// Check for dynamic end-tag format strings in a `next_token()` function body.
pub(super) fn dynamic_end_tag_format_evidence(
    body: &[Stmt],
    parser_var: &str,
    token_var: &str,
) -> Option<EndTagEvidence> {
    let env = analyze_env(body, parser_var, token_var);
    let mut found = None;
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        let evidence = match stmt {
            Stmt::Expr(expr_stmt) => direct_dynamic_end_expr_evidence(&expr_stmt.value, &env),
            Stmt::Assign(StmtAssign { value, .. }) => direct_dynamic_end_expr_evidence(value, &env),
            Stmt::Return(StmtReturn {
                value: Some(val), ..
            }) => direct_dynamic_end_expr_evidence(val, &env),
            Stmt::FunctionDef(_)
            | Stmt::ClassDef(_)
            | Stmt::Return(_)
            | Stmt::Delete(_)
            | Stmt::TypeAlias(_)
            | Stmt::AugAssign(_)
            | Stmt::AnnAssign(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Raise(_)
            | Stmt::Try(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => None,
        };
        if let Some(evidence) = evidence {
            found = Some(evidence);
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    found
}

fn direct_dynamic_end_expr_evidence(expr: &Expr, env: &Env) -> Option<EndTagEvidence> {
    percent_format_evidence(expr, env).or_else(|| fstring_evidence(expr, env))
}

fn percent_format_evidence(expr: &Expr, env: &Env) -> Option<EndTagEvidence> {
    let Expr::BinOp(ExprBinOp {
        left,
        op: Operator::Mod,
        right,
        ..
    }) = expr
    else {
        return None;
    };
    let format = left.string_literal()?;
    if !format.starts_with("end") || !format.contains('%') {
        return None;
    }
    if format == "end%s" && expr_resolves_to_tag_name(right, env) {
        Some(EndTagEvidence::SelfNamed)
    } else {
        Some(EndTagEvidence::Unknown)
    }
}

fn fstring_evidence(expr: &Expr, env: &Env) -> Option<EndTagEvidence> {
    let Expr::FString(ExprFString { value, .. }) = expr else {
        return None;
    };

    if let Some(interpolation) = exact_end_fstring_interpolation(expr)
        && expr_resolves_to_tag_name(interpolation, env)
    {
        return Some(EndTagEvidence::SelfNamed);
    }

    let has_dynamic_end_prefix = value.iter().any(|part| {
        let FStringPart::FString(fstr) = part else {
            return false;
        };
        let starts_with_end = matches!(
            fstr.elements.first(),
            Some(InterpolatedStringElement::Literal(lit)) if lit.value.starts_with("end")
        );
        starts_with_end
            && fstr
                .elements
                .iter()
                .any(|element| matches!(element, InterpolatedStringElement::Interpolation(_)))
    });

    has_dynamic_end_prefix.then_some(EndTagEvidence::Unknown)
}

fn exact_end_fstring_interpolation(expr: &Expr) -> Option<&Expr> {
    let Expr::FString(ExprFString { value, .. }) = expr else {
        return None;
    };
    let [FStringPart::FString(fstr)] = value.as_slice() else {
        return None;
    };
    let [
        InterpolatedStringElement::Literal(prefix),
        InterpolatedStringElement::Interpolation(interpolation),
    ] = &*fstr.elements
    else {
        return None;
    };
    (prefix.value.as_ref() == "end").then_some(interpolation.expression.as_ref())
}

fn expr_resolves_to_tag_name(expr: &Expr, env: &Env) -> bool {
    let mut env = env.clone();
    is_tag_name_value(&eval_expr(expr, &mut env))
}
