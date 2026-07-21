use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtIf;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::blocks::EndTagEvidence;
use crate::templates::tags::blocks::ExtractedBlockSpec;
use crate::templates::tags::blocks::extract_string_sequence;
use crate::templates::tags::blocks::is_parser_receiver;
use crate::templates::tags::blocks::is_token_contents_expr;

/// Detect block structure from `parser.parse((...))` calls with control flow analysis.
///
/// Collects all stop-tokens from parse calls, then classifies them as intermediates
/// or end-tags based on whether they lead to further parse calls (intermediate) or
/// return/construction (end-tag).
pub(super) fn detect(
    body: &[Stmt],
    parser_var: &str,
    token_var: &str,
) -> Option<ExtractedBlockSpec> {
    let parse_calls = collect_parser_parse_calls(body, parser_var);

    if parse_calls.is_empty() {
        return None;
    }

    classify_stop_tokens(body, parser_var, token_var, &parse_calls)
}

/// Information about a single `parser.parse((...))` call site.
#[derive(Debug)]
struct ParseCallInfo {
    stop_tokens: Vec<String>,
}

/// Collect all `parser.parse((...))` calls in a statement body.
fn collect_parser_parse_calls(body: &[Stmt], parser_var: &str) -> Vec<ParseCallInfo> {
    let mut calls = Vec::new();
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if let Some(info) = extract_parse_call_info(&expr_stmt.value, parser_var) {
                    calls.push(info);
                }
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                if let Some(info) = extract_parse_call_info(value, parser_var) {
                    calls.push(info);
                }
            }
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
            | Stmt::IpyEscapeCommand(_) => {}
        }
        ControlFlow::Continue(())
    });
    calls
}

/// Check if an expression is a `parser.parse((...))` call and extract stop-tokens.
fn extract_parse_call_info(expr: &Expr, parser_var: &str) -> Option<ParseCallInfo> {
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

    let stop_tokens = extract_string_sequence(&arguments.args[0]);
    if stop_tokens.is_empty() {
        return None;
    }

    Some(ParseCallInfo { stop_tokens })
}

/// Classify stop-tokens into end-tags and intermediates using control flow analysis.
///
/// Strategy: Walk the function body looking for the sequential `parser.parse()`
/// pattern. When we find `parser.parse((tokens...))` followed by a condition
/// that checks which token was matched, we can classify:
/// - Tokens that lead to another `parser.parse()` call → intermediate
/// - Tokens that lead to return or node construction → terminal (end-tag)
fn classify_stop_tokens(
    body: &[Stmt],
    parser_var: &str,
    token_var: &str,
    parse_calls: &[ParseCallInfo],
) -> Option<ExtractedBlockSpec> {
    let mut all_tokens: Vec<String> = Vec::new();
    for call in parse_calls {
        for token in &call.stop_tokens {
            if !all_tokens.contains(token) {
                all_tokens.push(token.clone());
            }
        }
    }

    if all_tokens.is_empty() {
        return None;
    }

    let Classification {
        mut intermediates,
        mut end_tags,
    } = classify_in_body(body, parser_var, token_var, &all_tokens);

    // After flow analysis: any token that was found in stop-token lists but NOT
    // classified as intermediate is a candidate end-tag.
    if !intermediates.is_empty() {
        for token in &all_tokens {
            if !intermediates.contains(token) && !end_tags.contains(token) {
                end_tags.push(token.clone());
            }
        }
    }

    // If flow analysis couldn't classify anything, try structural fallbacks
    if intermediates.is_empty() && end_tags.is_empty() {
        if let Some((last_call, earlier_calls)) = parse_calls.split_last()
            && !earlier_calls.is_empty()
        {
            for token in &last_call.stop_tokens {
                if !end_tags.contains(token) {
                    end_tags.push(token.clone());
                }
            }
            for call in earlier_calls {
                for token in &call.stop_tokens {
                    if !end_tags.contains(token) && !intermediates.contains(token) {
                        intermediates.push(token.clone());
                    }
                }
            }
        } else if let [call] = parse_calls {
            let tokens = &call.stop_tokens;
            if tokens.len() == 1 {
                end_tags.push(tokens[0].clone());
            } else {
                return None;
            }
        }
    }

    intermediates.retain(|t| !end_tags.contains(t));

    if end_tags.is_empty() && intermediates.is_empty() {
        return None;
    }

    let end_tag = match end_tags.len() {
        1 => EndTagEvidence::Literal(end_tags[0].clone()),
        _ => EndTagEvidence::Unknown,
    };

    intermediates.sort();

    Some(ExtractedBlockSpec {
        end_tag,
        intermediates,
        opaque: false,
    })
}

/// Result of classifying stop-tokens into intermediates and end-tags.
#[derive(Debug, Default)]
struct Classification {
    intermediates: Vec<String>,
    end_tags: Vec<String>,
}

impl Classification {
    fn merge(&mut self, other: Classification) {
        for t in other.intermediates {
            if !self.intermediates.contains(&t) {
                self.intermediates.push(t);
            }
        }
        for t in other.end_tags {
            if !self.end_tags.contains(&t) {
                self.end_tags.push(t);
            }
        }
    }

    fn add_intermediate(&mut self, token: String) {
        if !self.intermediates.contains(&token) {
            self.intermediates.push(token);
        }
    }

    fn add_end_tag(&mut self, token: String) {
        if !self.end_tags.contains(&token) {
            self.end_tags.push(token);
        }
    }
}

/// Walk body statements classifying tokens based on control flow patterns.
fn classify_in_body(
    body: &[Stmt],
    parser_var: &str,
    token_var: &str,
    all_tokens: &[String],
) -> Classification {
    let mut result = Classification::default();

    for (i, stmt) in body.iter().enumerate() {
        if let Stmt::If(if_stmt) = stmt {
            result.merge(classify_from_if_chain(
                if_stmt, parser_var, token_var, all_tokens,
            ));
        }

        if let Stmt::While(while_stmt) = stmt {
            if let Some(token) = extract_token_check(&while_stmt.test, token_var, all_tokens)
                .or_else(|| extract_startswith_check(&while_stmt.test, token_var, all_tokens))
            {
                if body_has_parse_call(&while_stmt.body, parser_var)
                    || body_has_parse_call(&while_stmt.orelse, parser_var)
                {
                    result.add_intermediate(token);
                } else {
                    result.add_end_tag(token);
                }
            }
            result.merge(classify_in_body(
                &while_stmt.body,
                parser_var,
                token_var,
                all_tokens,
            ));
            result.merge(classify_in_body(
                &while_stmt.orelse,
                parser_var,
                token_var,
                all_tokens,
            ));
        }

        if let Stmt::For(for_stmt) = stmt {
            result.merge(classify_in_body(
                &for_stmt.body,
                parser_var,
                token_var,
                all_tokens,
            ));
            result.merge(classify_in_body(
                &for_stmt.orelse,
                parser_var,
                token_var,
                all_tokens,
            ));
        }

        if let Stmt::Try(try_stmt) = stmt {
            result.merge(classify_in_body(
                &try_stmt.body,
                parser_var,
                token_var,
                all_tokens,
            ));
            for handler in &try_stmt.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                result.merge(classify_in_body(&h.body, parser_var, token_var, all_tokens));
            }
            result.merge(classify_in_body(
                &try_stmt.orelse,
                parser_var,
                token_var,
                all_tokens,
            ));
            result.merge(classify_in_body(
                &try_stmt.finalbody,
                parser_var,
                token_var,
                all_tokens,
            ));
        }

        let has_parse_call = if let Stmt::Expr(expr_stmt) = stmt {
            extract_parse_call_info(&expr_stmt.value, parser_var).is_some()
        } else if let Stmt::Assign(StmtAssign { value, .. }) = stmt {
            extract_parse_call_info(value, parser_var).is_some()
        } else {
            false
        };
        if has_parse_call
            && let Some(Stmt::If(if_stmt)) = body.get(i + 1).or_else(|| body.get(i + 2))
        {
            result.merge(classify_from_if_chain(
                if_stmt, parser_var, token_var, all_tokens,
            ));
        }
    }

    result
}

/// Classify tokens from an if/elif/else chain.
fn classify_from_if_chain(
    if_stmt: &StmtIf,
    parser_var: &str,
    token_var: &str,
    all_tokens: &[String],
) -> Classification {
    let mut result = Classification::default();

    if let Some(token) = extract_token_check(&if_stmt.test, token_var, all_tokens) {
        if body_has_parse_call(&if_stmt.body, parser_var) {
            result.add_intermediate(token);
        } else {
            result.add_end_tag(token);
        }
    }

    for clause in &if_stmt.elif_else_clauses {
        if let Some(test) = &clause.test
            && let Some(token) = extract_token_check(test, token_var, all_tokens)
        {
            if body_has_parse_call(&clause.body, parser_var) {
                result.add_intermediate(token);
            } else {
                result.add_end_tag(token);
            }
        }
    }

    result.merge(classify_in_body(
        &if_stmt.body,
        parser_var,
        token_var,
        all_tokens,
    ));
    for clause in &if_stmt.elif_else_clauses {
        result.merge(classify_in_body(
            &clause.body,
            parser_var,
            token_var,
            all_tokens,
        ));
    }

    result
}

/// Check if a condition expression checks a token string against known stop-tokens.
fn extract_token_check(expr: &Expr, token_var: &str, known_tokens: &[String]) -> Option<String> {
    if let Expr::Compare(compare) = expr
        && compare.ops.len() == 1
        && compare.comparators.len() == 1
    {
        let left = &compare.left;
        let right = &compare.comparators[0];

        if is_token_contents_expr(left, Some(token_var))
            && let Some(s) = right.string_literal()
        {
            let cmd = s.split_whitespace().next().unwrap_or("").to_string();
            if known_tokens.contains(&cmd) {
                return Some(cmd);
            }
        }
        if is_token_contents_expr(right, Some(token_var))
            && let Some(s) = left.string_literal()
        {
            let cmd = s.split_whitespace().next().unwrap_or("").to_string();
            if known_tokens.contains(&cmd) {
                return Some(cmd);
            }
        }
    }
    None
}

/// Check if a condition is a `startswith` check against known tokens.
fn extract_startswith_check(
    expr: &Expr,
    token_var: &str,
    known_tokens: &[String],
) -> Option<String> {
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
    if attr.as_str() != "startswith" {
        return None;
    }
    if !is_token_contents_expr(obj, Some(token_var)) {
        return None;
    }
    if arguments.args.is_empty() {
        return None;
    }
    let s = arguments.args[0].string_literal()?;
    let cmd = s.split_whitespace().next().unwrap_or("").to_string();
    if known_tokens.contains(&cmd) {
        Some(cmd)
    } else {
        None
    }
}

/// Check if a statement body contains a `parser.parse(...)` call.
fn body_has_parse_call(body: &[Stmt], parser_var: &str) -> bool {
    let mut found = false;
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        let has_parse_call = match stmt {
            Stmt::Expr(expr_stmt) => {
                extract_parse_call_info(&expr_stmt.value, parser_var).is_some()
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                extract_parse_call_info(value, parser_var).is_some()
            }
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
            | Stmt::IpyEscapeCommand(_) => false,
        };
        if has_parse_call {
            found = true;
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    found
}
