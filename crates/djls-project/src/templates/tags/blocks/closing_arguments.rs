use std::ops::ControlFlow;

use ruff_python_ast::CmpOp;
use ruff_python_ast::ConversionFlag;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprFString;
use ruff_python_ast::ExprList;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::FStringPart;
use ruff_python_ast::InterpolatedStringElement;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtIf;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::analysis::AbstractValue;
use crate::templates::tags::analysis::Env;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::blocks::dynamic_end;
use crate::templates::tags::blocks::is_parser_receiver;
use crate::templates::tags::blocks::is_token_contents_expr;
use crate::templates::tags::blocks::next_token::is_next_token_call;
use crate::templates::tags::types::ClosingTagArguments;
use crate::templates::tags::types::SplitPosition;

/// Detect a compile function that rejects closing tags not repeating an opener argument.
///
/// Recognizes the closing-token check in Django's `do_block` and `partialdef_func`:
///
/// ```python
/// endblock = parser.next_token()
/// acceptable_endblocks = ("endblock", "endblock %s" % block_name)
/// if endblock.contents not in acceptable_endblocks:
///     parser.invalid_block_tag(endblock, "endblock", acceptable_endblocks)
/// ```
///
/// The accepted contents must be exactly the bare closing tag and the closing tag
/// followed by one opener argument; anything else stays `Unchecked`.
pub(super) fn detect(
    body: &[Stmt],
    parser_var: &str,
    token_var: &str,
    end_tag: &str,
) -> ClosingTagArguments {
    let closing_tokens = collect_next_token_targets(body, parser_var);
    if closing_tokens.is_empty() {
        return ClosingTagArguments::Unchecked;
    }
    let env = dynamic_end::analyze_env(body, parser_var, token_var);
    let assignments = dynamic_end::collect_simple_assignments(body);

    let mut found = ClosingTagArguments::Unchecked;
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        if let Stmt::If(if_stmt) = stmt
            && let Some(accepted) = rejected_closing_contents(if_stmt, &closing_tokens, parser_var)
            && let Some(position) = repeated_opener_argument(accepted, &assignments, &env, end_tag)
        {
            found = ClosingTagArguments::RepeatsOpenerArgument { position };
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    found
}

/// Names bound to `parser.next_token()`.
fn collect_next_token_targets(body: &[Stmt], parser_var: &str) -> Vec<String> {
    let mut targets = Vec::new();
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        if let Stmt::Assign(StmtAssign {
            targets: t, value, ..
        }) = stmt
            && t.len() == 1
            && let Some(name) = t[0].name_target()
            && is_next_token_call(value, parser_var)
        {
            targets.push(name.to_string());
        }
        ControlFlow::Continue(())
    });
    targets
}

/// Match `if closing.contents not in ACCEPTED: <reject>` and return `ACCEPTED`.
fn rejected_closing_contents<'a>(
    if_stmt: &'a StmtIf,
    closing_tokens: &[String],
    parser_var: &str,
) -> Option<&'a Expr> {
    let Expr::Compare(ExprCompare {
        left,
        ops,
        comparators,
        ..
    }) = if_stmt.test.as_ref()
    else {
        return None;
    };
    let ([CmpOp::NotIn], [accepted]) = (&**ops, &**comparators) else {
        return None;
    };
    let Expr::Attribute(ExprAttribute { attr, .. }) = left.as_ref() else {
        return None;
    };
    if attr.as_str() != "contents"
        || !closing_tokens
            .iter()
            .any(|name| is_token_contents_expr(left, Some(name)))
    {
        return None;
    }
    if_stmt
        .body
        .iter()
        .any(|stmt| is_rejection(stmt, parser_var))
        .then_some(accepted)
}

fn is_rejection(stmt: &Stmt, parser_var: &str) -> bool {
    if matches!(stmt, Stmt::Raise(_)) {
        return true;
    }
    let Stmt::Expr(expr_stmt) = stmt else {
        return false;
    };
    let Expr::Call(ExprCall { func, .. }) = expr_stmt.value.as_ref() else {
        return false;
    };
    matches!(
        func.as_ref(),
        Expr::Attribute(ExprAttribute { attr, value, .. })
            if attr.as_str() == "invalid_block_tag" && is_parser_receiver(value, parser_var)
    )
}

/// Return the opener split position when `accepted` is exactly
/// `(end_tag, "<end_tag> <opener argument>")` in either order.
fn repeated_opener_argument(
    accepted: &Expr,
    assignments: &[(String, &Expr)],
    env: &Env,
    end_tag: &str,
) -> Option<usize> {
    let accepted = match accepted.name_target() {
        Some(name) => {
            let mut bound = assignments
                .iter()
                .filter(|(assigned, _)| assigned == name)
                .map(|(_, value)| *value);
            let (Some(value), None) = (bound.next(), bound.next()) else {
                return None;
            };
            value
        }
        None => accepted,
    };
    let (Expr::Tuple(ExprTuple { elts, .. }) | Expr::List(ExprList { elts, .. })) = accepted else {
        return None;
    };
    let [first, second] = elts.as_slice() else {
        return None;
    };
    let (bare, named) = if first.string_literal() == Some(end_tag) {
        (first, second)
    } else {
        (second, first)
    };
    if bare.string_literal() != Some(end_tag) {
        return None;
    }
    let argument = closing_argument_expr(named, end_tag)?;
    let mut env = env.clone();
    let AbstractValue::SplitElement {
        index: SplitPosition::Forward(position),
    } = eval_expr(argument, &mut env)
    else {
        return None;
    };
    (position > 0).then_some(position)
}

/// Match `"<end_tag> %s" % argument` or `f"<end_tag> {argument}"`.
fn closing_argument_expr<'a>(expr: &'a Expr, end_tag: &str) -> Option<&'a Expr> {
    let prefix = format!("{end_tag} ");
    if let Expr::BinOp(ExprBinOp {
        left,
        op: Operator::Mod,
        right,
        ..
    }) = expr
    {
        return (left.string_literal()? == format!("{prefix}%s")).then_some(right.as_ref());
    }
    let Expr::FString(ExprFString { value, .. }) = expr else {
        return None;
    };
    let [FStringPart::FString(fstring)] = value.as_slice() else {
        return None;
    };
    let [
        InterpolatedStringElement::Literal(literal),
        InterpolatedStringElement::Interpolation(interpolation),
    ] = &*fstring.elements
    else {
        return None;
    };
    (literal.value.as_ref() == prefix
        && interpolation.conversion == ConversionFlag::None
        && interpolation.format_spec.is_none())
    .then_some(interpolation.expression.as_ref())
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::templates::tags::testing::django_function;

    fn detect_in(source: &str, end_tag: &str) -> ClosingTagArguments {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        let Some(Stmt::FunctionDef(func)) = module.body.into_iter().next() else {
            panic!("expected a function definition");
        };
        detect(&func.body, "parser", "token", end_tag)
    }

    #[test]
    fn delete_first_token_leaves_closing_arguments_unchecked() {
        let func = django_function("django/template/defaulttags.py", "do_for")
            .expect("expected Django fixture function should exist");
        assert_eq!(
            detect(&func.body, "parser", "token", "endfor"),
            ClosingTagArguments::Unchecked
        );
    }

    #[test]
    fn check_against_other_end_tag_is_unchecked() {
        let source = r#"
def do_section(parser, token):
    bits = token.split_contents()
    nodelist = parser.parse(("endsection",))
    closing = parser.next_token()
    if closing.contents not in ("endsection", "endother %s" % bits[1]):
        raise TemplateSyntaxError("bad closer")
    return SectionNode(nodelist)
"#;
        assert_eq!(
            detect_in(source, "endsection"),
            ClosingTagArguments::Unchecked
        );
    }

    #[test]
    fn check_against_tag_name_is_unchecked() {
        let source = r#"
def do_section(parser, token):
    bits = token.split_contents()
    nodelist = parser.parse(("endsection",))
    closing = parser.next_token()
    if closing.contents not in ("endsection", "endsection %s" % bits[0]):
        raise TemplateSyntaxError("bad closer")
    return SectionNode(nodelist)
"#;
        assert_eq!(
            detect_in(source, "endsection"),
            ClosingTagArguments::Unchecked
        );
    }

    #[test]
    fn check_without_rejection_is_unchecked() {
        let source = r#"
def do_section(parser, token):
    bits = token.split_contents()
    nodelist = parser.parse(("endsection",))
    closing = parser.next_token()
    if closing.contents not in ("endsection", "endsection %s" % bits[1]):
        warn("mismatched closer")
    return SectionNode(nodelist)
"#;
        assert_eq!(
            detect_in(source, "endsection"),
            ClosingTagArguments::Unchecked
        );
    }

    #[test]
    fn second_argument_position_is_recorded() {
        let source = r#"
def do_section(parser, token):
    bits = token.split_contents()
    label = bits[2]
    nodelist = parser.parse(("endsection",))
    closing = parser.next_token()
    if closing.contents not in [f"endsection {label}", "endsection"]:
        parser.invalid_block_tag(closing, "endsection", ())
    return SectionNode(nodelist)
"#;
        assert_eq!(
            detect_in(source, "endsection"),
            ClosingTagArguments::RepeatsOpenerArgument { position: 2 }
        );
    }
}
