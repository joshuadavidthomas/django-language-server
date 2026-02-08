use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprFString;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::FStringElement;
use ruff_python_ast::FStringPart;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtFor;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::StmtIf;
use ruff_python_ast::StmtReturn;

use crate::types::BlockTagSpec;

/// Extract a block spec from a tag's compile function.
///
/// Finds calls to `parser.parse((...))` with tuple arguments containing
/// stop-token strings. Determines end-tag vs intermediate from control flow:
/// - If a stop-token leads to another `parser.parse()` call → intermediate
/// - If a stop-token leads to return/node construction → terminal (end-tag)
///
/// Also detects opaque blocks via `parser.skip_past(...)` patterns.
///
/// Returns `None` when no block structure is detected or inference is ambiguous.
#[must_use]
pub fn extract_block_spec(func: &StmtFunctionDef) -> Option<BlockTagSpec> {
    let parser_var = func
        .parameters
        .args
        .first()
        .map(|p| p.parameter.name.to_string())?;

    // Check for opaque block patterns first: parser.skip_past("endtag")
    let mut skip_past_tokens = Vec::new();
    collect_skip_past_tokens(&func.body, &parser_var, &mut skip_past_tokens);
    if !skip_past_tokens.is_empty() {
        let end_tag = if skip_past_tokens.len() == 1 {
            Some(skip_past_tokens[0].clone())
        } else {
            None
        };
        return Some(BlockTagSpec {
            end_tag,
            intermediates: Vec::new(),
            opaque: true,
        });
    }

    // Collect all stop-tokens from parser.parse((...)) calls
    let mut parse_calls = Vec::new();
    collect_parser_parse_calls(&func.body, &parser_var, &mut parse_calls);

    if parse_calls.is_empty() {
        // Try dynamic end-tag patterns: parser.parse((f"end{tag_name}",))
        if has_dynamic_end_in_body(&func.body, &parser_var) {
            return Some(BlockTagSpec {
                end_tag: None,
                intermediates: Vec::new(),
                opaque: false,
            });
        }

        // Try parser.next_token() loop patterns (e.g., blocktrans/blocktranslate)
        if let Some(spec) = extract_next_token_loop_spec(&func.body, &parser_var) {
            return Some(spec);
        }

        return None;
    }

    // Classify tokens as intermediate vs terminal using control flow analysis
    classify_stop_tokens(&func.body, &parser_var, &parse_calls)
}

/// Information about a single `parser.parse((...))` call site.
#[derive(Debug)]
struct ParseCallInfo {
    /// The stop-token strings extracted from the tuple argument.
    stop_tokens: Vec<String>,
}

/// Collect all `parser.parse((...))` calls in a statement body (recursively).
fn collect_parser_parse_calls(body: &[Stmt], parser_var: &str, calls: &mut Vec<ParseCallInfo>) {
    for stmt in body {
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
            Stmt::If(if_stmt) => {
                collect_parser_parse_calls(&if_stmt.body, parser_var, calls);
                for clause in &if_stmt.elif_else_clauses {
                    collect_parser_parse_calls(&clause.body, parser_var, calls);
                }
            }
            Stmt::For(for_stmt) => {
                collect_parser_parse_calls(&for_stmt.body, parser_var, calls);
            }
            Stmt::While(while_stmt) => {
                collect_parser_parse_calls(&while_stmt.body, parser_var, calls);
            }
            Stmt::Try(try_stmt) => {
                collect_parser_parse_calls(&try_stmt.body, parser_var, calls);
                for handler in &try_stmt.handlers {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                    collect_parser_parse_calls(&h.body, parser_var, calls);
                }
            }
            Stmt::With(with_stmt) => {
                collect_parser_parse_calls(&with_stmt.body, parser_var, calls);
            }
            _ => {}
        }
    }
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

/// Check if an expression is the parser variable (or `self.parser`).
fn is_parser_receiver(expr: &Expr, parser_var: &str) -> bool {
    // Direct: `parser.parse(...)`
    if let Expr::Name(ExprName { id, .. }) = expr {
        if id.as_str() == parser_var {
            return true;
        }
    }
    // Indirect: `self.parser.parse(...)` (classytags-like pattern)
    if let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = expr
    {
        if attr.as_str() == "parser" {
            if let Expr::Name(ExprName { id, .. }) = obj.as_ref() {
                if id.as_str() == parser_var || id.as_str() == "self" {
                    return true;
                }
            }
        }
    }
    false
}

/// Extract string constants from a tuple/list expression.
///
/// Handles:
/// - `("endif", "else", "elif")`
/// - `("endif",)`
/// - Variable references resolved from known constant assignments nearby
fn extract_string_sequence(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::Tuple(t) => t.elts.iter().filter_map(extract_string_value).collect(),
        Expr::List(l) => l.elts.iter().filter_map(extract_string_value).collect(),
        Expr::Set(s) => s.elts.iter().filter_map(extract_string_value).collect(),
        _ => Vec::new(),
    }
}

/// Extract a string value from a single expression.
fn extract_string_value(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = expr {
        let s = value.to_str();
        // Django's Parser.parse() compares against `command = token.contents.split()[0]`,
        // so only the first word of a stop-token string matters.
        let cmd = s.split_whitespace().next().unwrap_or("");
        if cmd.is_empty() {
            return None;
        }
        return Some(cmd.to_string());
    }
    None
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
    parse_calls: &[ParseCallInfo],
) -> Option<BlockTagSpec> {
    // Gather all unique stop-tokens across all parse calls
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

    // Classify tokens by analyzing control flow after each parser.parse() call
    let mut intermediates: Vec<String> = Vec::new();
    let mut end_tags: Vec<String> = Vec::new();

    classify_in_body(
        body,
        parser_var,
        &all_tokens,
        &mut intermediates,
        &mut end_tags,
    );

    // After flow analysis: any token that was found in stop-token lists but NOT
    // classified as intermediate is a candidate end-tag. This handles the common
    // case where "endif" appears in stop-tokens but is never checked in an
    // if-condition (because it's the final/terminal token).
    if !intermediates.is_empty() {
        for token in &all_tokens {
            if !intermediates.contains(token) && !end_tags.contains(token) {
                end_tags.push(token.clone());
            }
        }
    }

    // If flow analysis couldn't classify anything, try structural fallbacks
    if intermediates.is_empty() && end_tags.is_empty() {
        if parse_calls.len() >= 2 {
            // Multi-parse pattern: tokens from the LAST parse call are likely
            // end-tags, everything else is intermediate
            let last_call = parse_calls.last().unwrap();
            for token in &last_call.stop_tokens {
                if !end_tags.contains(token) {
                    end_tags.push(token.clone());
                }
            }
            for call in &parse_calls[..parse_calls.len() - 1] {
                for token in &call.stop_tokens {
                    if !end_tags.contains(token) && !intermediates.contains(token) {
                        intermediates.push(token.clone());
                    }
                }
            }
        } else if parse_calls.len() == 1 {
            let tokens = &parse_calls[0].stop_tokens;
            if tokens.len() == 1 {
                // Single stop-token in single parse call → end-tag
                end_tags.push(tokens[0].clone());
            } else {
                // Multiple tokens in single parse call — ambiguous without flow analysis
                // Use convention as tie-breaker only: `end*` tokens are likely end-tags
                for token in tokens {
                    if token.starts_with("end") {
                        end_tags.push(token.clone());
                    } else {
                        intermediates.push(token.clone());
                    }
                }
                // If no end-tag found via convention, result is ambiguous → None
                if end_tags.is_empty() {
                    return None;
                }
            }
        }
    }

    // Remove intermediates that also appear as end-tags
    intermediates.retain(|t| !end_tags.contains(t));

    if end_tags.is_empty() && intermediates.is_empty() {
        return None;
    }

    // If we have intermediates but no end-tag, that's ambiguous
    let end_tag = match end_tags.len() {
        1 => Some(end_tags[0].clone()),
        // Multiple end-tag candidates or none — ambiguous
        _ => None,
    };

    intermediates.sort();

    Some(BlockTagSpec {
        end_tag,
        intermediates,
        opaque: false,
    })
}

/// Walk body statements classifying tokens based on control flow patterns.
///
/// Looks for the pattern:
/// ```python
/// nodelist = parser.parse(("else", "endif"))
/// token = parser.next_token()
/// if token.contents == "else":
///     nodelist_else = parser.parse(("endif",))
///     ...
/// ```
///
/// Where a token leads to another `parse()` call → intermediate,
/// and a token leads to return/construction → end-tag.
fn classify_in_body(
    body: &[Stmt],
    parser_var: &str,
    all_tokens: &[String],
    intermediates: &mut Vec<String>,
    end_tags: &mut Vec<String>,
) {
    for (i, stmt) in body.iter().enumerate() {
        // Look for if-statements that check token contents after a parse() call
        if let Stmt::If(if_stmt) = stmt {
            classify_from_if_chain(if_stmt, parser_var, all_tokens, intermediates, end_tags);
        }

        // Check while-loops for token classification (e.g., Django's if-tag
        // uses `while token.contents.startswith("elif"):`)
        if let Stmt::While(while_stmt) = stmt {
            if let Some(token) = extract_token_check(&while_stmt.test, all_tokens)
                .or_else(|| extract_startswith_check(&while_stmt.test, all_tokens))
            {
                if body_has_parse_call(&while_stmt.body, parser_var) {
                    if !intermediates.contains(&token) {
                        intermediates.push(token);
                    }
                } else if !end_tags.contains(&token) {
                    end_tags.push(token);
                }
            }
            classify_in_body(
                &while_stmt.body,
                parser_var,
                all_tokens,
                intermediates,
                end_tags,
            );
        }

        // Check for for-loops
        if let Stmt::For(for_stmt) = stmt {
            classify_in_body(
                &for_stmt.body,
                parser_var,
                all_tokens,
                intermediates,
                end_tags,
            );
        }

        // Recurse into try blocks
        if let Stmt::Try(try_stmt) = stmt {
            classify_in_body(
                &try_stmt.body,
                parser_var,
                all_tokens,
                intermediates,
                end_tags,
            );
        }

        // Check sequential pattern: parse() call followed by if-check
        let has_parse_call = match stmt {
            Stmt::Expr(expr_stmt) => {
                extract_parse_call_info(&expr_stmt.value, parser_var).is_some()
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                extract_parse_call_info(value, parser_var).is_some()
            }
            _ => false,
        };
        if has_parse_call {
            // Look ahead for an if-statement checking the token
            if let Some(Stmt::If(if_stmt)) = body.get(i + 1).or_else(|| body.get(i + 2)) {
                classify_from_if_chain(if_stmt, parser_var, all_tokens, intermediates, end_tags);
            }
        }
    }
}

/// Classify tokens from an if/elif/else chain.
///
/// For each branch that checks a token string:
/// - If the branch body contains another `parser.parse()` call → the checked
///   token is an intermediate
/// - If the branch body does NOT contain a `parser.parse()` call → the checked
///   token is a potential end-tag
fn classify_from_if_chain(
    if_stmt: &StmtIf,
    parser_var: &str,
    all_tokens: &[String],
    intermediates: &mut Vec<String>,
    end_tags: &mut Vec<String>,
) {
    // Check the main `if` branch
    if let Some(token) = extract_token_check(&if_stmt.test, all_tokens) {
        if body_has_parse_call(&if_stmt.body, parser_var) {
            if !intermediates.contains(&token) {
                intermediates.push(token);
            }
        } else if !end_tags.contains(&token) {
            end_tags.push(token);
        }
    }

    // Check elif/else branches
    for clause in &if_stmt.elif_else_clauses {
        if let Some(test) = &clause.test {
            if let Some(token) = extract_token_check(test, all_tokens) {
                if body_has_parse_call(&clause.body, parser_var) {
                    if !intermediates.contains(&token) {
                        intermediates.push(token);
                    }
                } else if !end_tags.contains(&token) {
                    end_tags.push(token);
                }
            }
        }
    }

    // Recurse into the if-body for nested patterns
    classify_in_body(
        &if_stmt.body,
        parser_var,
        all_tokens,
        intermediates,
        end_tags,
    );
    for clause in &if_stmt.elif_else_clauses {
        classify_in_body(
            &clause.body,
            parser_var,
            all_tokens,
            intermediates,
            end_tags,
        );
    }
}

/// Check if a condition expression checks a token string against known stop-tokens.
///
/// Matches patterns like:
/// - `token.contents == "else"`
/// - `token.contents.split()[0] == "elif"`
fn extract_token_check(expr: &Expr, known_tokens: &[String]) -> Option<String> {
    if let Expr::Compare(compare) = expr {
        if compare.ops.len() == 1 && compare.comparators.len() == 1 {
            let left = &compare.left;
            let right = &compare.comparators[0];

            // Check both sides for string constant matching known tokens
            if is_token_contents_expr(left) {
                if let Some(s) = get_string_constant(right) {
                    let cmd = s.split_whitespace().next().unwrap_or("").to_string();
                    if known_tokens.contains(&cmd) {
                        return Some(cmd);
                    }
                }
            }
            if is_token_contents_expr(right) {
                if let Some(s) = get_string_constant(left) {
                    let cmd = s.split_whitespace().next().unwrap_or("").to_string();
                    if known_tokens.contains(&cmd) {
                        return Some(cmd);
                    }
                }
            }
        }
    }
    None
}

/// Check if a condition is a `startswith` check against known tokens.
///
/// Matches: `token.contents.startswith("elif")`
fn extract_startswith_check(expr: &Expr, known_tokens: &[String]) -> Option<String> {
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
    if !is_token_contents_expr(obj) {
        return None;
    }
    if arguments.args.is_empty() {
        return None;
    }
    let s = get_string_constant(&arguments.args[0])?;
    let cmd = s.split_whitespace().next().unwrap_or("").to_string();
    if known_tokens.contains(&cmd) {
        Some(cmd)
    } else {
        None
    }
}

/// Check if an expression accesses token contents.
///
/// Matches: `token.contents`, `token.contents.split()[0]`, `token.contents.strip()`
fn is_token_contents_expr(expr: &Expr) -> bool {
    match expr {
        // token.contents
        Expr::Attribute(ExprAttribute { attr, value, .. }) => {
            if attr.as_str() == "contents" {
                return matches!(value.as_ref(), Expr::Name(_));
            }
            false
        }
        // token.contents.strip() or token.contents.split()[0]
        Expr::Call(ExprCall { func, .. }) => {
            if let Expr::Attribute(ExprAttribute { value, .. }) = func.as_ref() {
                return is_token_contents_expr(value);
            }
            false
        }
        // token.contents.split()[0]
        Expr::Subscript(sub) => is_token_contents_expr(&sub.value),
        _ => false,
    }
}

/// Extract a string constant from an expression.
fn get_string_constant(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = expr {
        return Some(value.to_str().to_string());
    }
    None
}

/// Check if a statement body contains a `parser.parse(...)` call.
fn body_has_parse_call(body: &[Stmt], parser_var: &str) -> bool {
    for stmt in body {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if extract_parse_call_info(&expr_stmt.value, parser_var).is_some() {
                    return true;
                }
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                if extract_parse_call_info(value, parser_var).is_some() {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if body_has_parse_call(&if_stmt.body, parser_var) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if body_has_parse_call(&clause.body, parser_var) {
                        return true;
                    }
                }
            }
            Stmt::For(for_stmt) => {
                if body_has_parse_call(&for_stmt.body, parser_var) {
                    return true;
                }
            }
            Stmt::While(while_stmt) => {
                if body_has_parse_call(&while_stmt.body, parser_var) {
                    return true;
                }
            }
            Stmt::Try(try_stmt) => {
                if body_has_parse_call(&try_stmt.body, parser_var) {
                    return true;
                }
            }
            Stmt::With(with_stmt) => {
                if body_has_parse_call(&with_stmt.body, parser_var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Collect all `parser.skip_past("token")` calls in a statement body (recursively).
fn collect_skip_past_tokens(body: &[Stmt], parser_var: &str, tokens: &mut Vec<String>) {
    for stmt in body {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if let Some(t) = extract_skip_past_token(&expr_stmt.value, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                if let Some(t) = extract_skip_past_token(value, parser_var) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            Stmt::If(if_stmt) => {
                collect_skip_past_tokens(&if_stmt.body, parser_var, tokens);
                for clause in &if_stmt.elif_else_clauses {
                    collect_skip_past_tokens(&clause.body, parser_var, tokens);
                }
            }
            Stmt::For(for_stmt) => {
                collect_skip_past_tokens(&for_stmt.body, parser_var, tokens);
            }
            Stmt::While(while_stmt) => {
                collect_skip_past_tokens(&while_stmt.body, parser_var, tokens);
            }
            Stmt::Try(try_stmt) => {
                collect_skip_past_tokens(&try_stmt.body, parser_var, tokens);
            }
            _ => {}
        }
    }
}

/// Check if an expression is `parser.skip_past("token")` and extract the token.
fn extract_skip_past_token(expr: &Expr, parser_var: &str) -> Option<String> {
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
    if attr.as_str() != "skip_past" {
        return None;
    }
    if !is_parser_receiver(obj, parser_var) {
        return None;
    }
    if arguments.args.is_empty() {
        return None;
    }
    get_string_constant(&arguments.args[0])
}

fn has_dynamic_end_in_body(body: &[Stmt], parser_var: &str) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::Expr(expr_stmt) => is_dynamic_end_parse_call(&expr_stmt.value, parser_var),
        Stmt::Assign(StmtAssign { value, .. }) => is_dynamic_end_parse_call(value, parser_var),
        Stmt::If(if_stmt) => {
            has_dynamic_end_in_body(&if_stmt.body, parser_var)
                || if_stmt
                    .elif_else_clauses
                    .iter()
                    .any(|c| has_dynamic_end_in_body(&c.body, parser_var))
        }
        Stmt::For(StmtFor { body, .. }) | Stmt::While(ruff_python_ast::StmtWhile { body, .. }) => {
            has_dynamic_end_in_body(body, parser_var)
        }
        Stmt::Return(StmtReturn {
            value: Some(val), ..
        }) => is_dynamic_end_parse_call(val, parser_var),
        _ => false,
    })
}

/// Check if an expression is `parser.parse((f"end{...}",))`.
fn is_dynamic_end_parse_call(expr: &Expr, parser_var: &str) -> bool {
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
    if attr.as_str() != "parse" {
        return false;
    }
    if !is_parser_receiver(obj, parser_var) {
        return false;
    }
    if arguments.args.is_empty() {
        return false;
    }

    // Check if the argument is a tuple/list containing an f-string with "end" prefix
    let seq = &arguments.args[0];
    let elements = match seq {
        Expr::Tuple(t) => &t.elts,
        Expr::List(l) => &l.elts,
        _ => return false,
    };

    for elt in elements {
        if is_end_fstring(elt) {
            return true;
        }
    }
    false
}

/// Check if an expression is an f-string starting with "end".
fn is_end_fstring(expr: &Expr) -> bool {
    let Expr::FString(ExprFString { value, .. }) = expr else {
        return false;
    };

    for part in value {
        match part {
            FStringPart::FString(fstr) => {
                let mut has_end_prefix = false;
                let mut has_interpolation = false;

                for element in &fstr.elements {
                    match element {
                        FStringElement::Literal(lit) => {
                            if lit.value.starts_with("end") {
                                has_end_prefix = true;
                            }
                        }
                        FStringElement::Expression(_) => {
                            has_interpolation = true;
                        }
                    }
                }

                if has_end_prefix && has_interpolation {
                    return true;
                }
            }
            FStringPart::Literal(_) => {}
        }
    }

    false
}

/// Extract a block spec from `parser.next_token()` loop patterns.
///
/// Handles tags like `blocktrans`/`blocktranslate` that manually iterate
/// tokens instead of using `parser.parse((...))`. The pattern is:
///
/// ```python
/// while parser.tokens:
///     token = parser.next_token()
///     if token.token_type in (...):
///         singular.append(token)
///     else:
///         break
/// # check for intermediate: token.contents.strip() != "plural"
/// # ...
/// end_tag_name = "end%s" % bits[0]
/// if token.contents.strip() != end_tag_name:
///     raise TemplateSyntaxError(...)
/// ```
fn extract_next_token_loop_spec(body: &[Stmt], parser_var: &str) -> Option<BlockTagSpec> {
    // Check if the body contains a `while parser.tokens:` loop with `parser.next_token()`
    if !has_next_token_loop(body, parser_var) {
        return None;
    }

    // Collect string literals compared against token.contents in the body
    // These are intermediates (like "plural") and end-tags
    let mut token_comparisons: Vec<String> = Vec::new();
    collect_token_content_comparisons(body, &mut token_comparisons);

    // Check for dynamic end-tag patterns: `end_tag_name = "end%s" % bits[0]`
    // or `"end%s" % bits[0]` used in comparisons
    let has_dynamic_end = has_dynamic_end_tag_format(body);

    if token_comparisons.is_empty() && !has_dynamic_end {
        // Has a token loop but no string comparisons — can't determine structure
        return None;
    }

    // Separate end-tags from intermediates
    let mut intermediates = Vec::new();
    let mut end_tag = None;

    for token in &token_comparisons {
        if token.starts_with("end") {
            // Static end-tag found
            end_tag = Some(token.clone());
        } else {
            intermediates.push(token.clone());
        }
    }

    // If no static end-tag but has dynamic pattern, end_tag stays None
    // If we found only intermediates (like "plural") and a dynamic end,
    // that's a valid blocktrans-like pattern
    if end_tag.is_none() && !has_dynamic_end && intermediates.is_empty() {
        return None;
    }

    intermediates.sort();

    Some(BlockTagSpec {
        end_tag,
        intermediates,
        opaque: false,
    })
}

/// Check if a body contains `while parser.tokens:` with `parser.next_token()`.
fn has_next_token_loop(body: &[Stmt], parser_var: &str) -> bool {
    for stmt in body {
        match stmt {
            Stmt::While(while_stmt) => {
                if is_parser_tokens_check(&while_stmt.test, parser_var)
                    && body_has_next_token_call(&while_stmt.body, parser_var)
                {
                    return true;
                }
                // Recurse into the while body
                if has_next_token_loop(&while_stmt.body, parser_var) {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if has_next_token_loop(&if_stmt.body, parser_var) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if has_next_token_loop(&clause.body, parser_var) {
                        return true;
                    }
                }
            }
            Stmt::For(for_stmt) => {
                if has_next_token_loop(&for_stmt.body, parser_var) {
                    return true;
                }
            }
            Stmt::Try(try_stmt) => {
                if has_next_token_loop(&try_stmt.body, parser_var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Check if an expression is `parser.tokens` (the token list attribute).
fn is_parser_tokens_check(expr: &Expr, parser_var: &str) -> bool {
    if let Expr::Attribute(ExprAttribute { attr, value, .. }) = expr {
        if attr.as_str() == "tokens" {
            if let Expr::Name(ExprName { id, .. }) = value.as_ref() {
                return id.as_str() == parser_var;
            }
        }
    }
    false
}

/// Check if a body contains a `parser.next_token()` call.
fn body_has_next_token_call(body: &[Stmt], parser_var: &str) -> bool {
    for stmt in body {
        match stmt {
            Stmt::Assign(StmtAssign { value, .. }) => {
                if is_next_token_call(value, parser_var) {
                    return true;
                }
            }
            Stmt::Expr(expr_stmt) => {
                if is_next_token_call(&expr_stmt.value, parser_var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Check if an expression is `parser.next_token()`.
fn is_next_token_call(expr: &Expr, parser_var: &str) -> bool {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return false;
    };
    if !arguments.args.is_empty() {
        return false;
    }
    let Expr::Attribute(ExprAttribute { attr, value, .. }) = func.as_ref() else {
        return false;
    };
    if attr.as_str() != "next_token" {
        return false;
    }
    if let Expr::Name(ExprName { id, .. }) = value.as_ref() {
        return id.as_str() == parser_var;
    }
    false
}

/// Collect string literals compared against `token.contents` in a body.
///
/// Looks for patterns like:
/// - `token.contents.strip() != "plural"`
/// - `token.contents == "endblocktrans"`
/// - `token.contents.strip() != end_tag_name` (skipped — dynamic)
fn collect_token_content_comparisons(body: &[Stmt], comparisons: &mut Vec<String>) {
    for stmt in body {
        match stmt {
            Stmt::If(if_stmt) => {
                extract_comparisons_from_expr(&if_stmt.test, comparisons);
                collect_token_content_comparisons(&if_stmt.body, comparisons);
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        extract_comparisons_from_expr(test, comparisons);
                    }
                    collect_token_content_comparisons(&clause.body, comparisons);
                }
            }
            Stmt::While(while_stmt) => {
                collect_token_content_comparisons(&while_stmt.body, comparisons);
            }
            Stmt::For(for_stmt) => {
                collect_token_content_comparisons(&for_stmt.body, comparisons);
            }
            Stmt::Try(try_stmt) => {
                collect_token_content_comparisons(&try_stmt.body, comparisons);
                for handler in &try_stmt.handlers {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                    collect_token_content_comparisons(&h.body, comparisons);
                }
            }
            _ => {}
        }
    }
}

/// Extract string comparisons against token.contents from a comparison expression.
fn extract_comparisons_from_expr(expr: &Expr, comparisons: &mut Vec<String>) {
    if let Expr::Compare(compare) = expr {
        for (left, right) in std::iter::once(compare.left.as_ref())
            .chain(compare.comparators.iter())
            .zip(
                compare
                    .comparators
                    .iter()
                    .chain(std::iter::once(compare.left.as_ref())),
            )
        {
            if is_token_contents_expr(left) || is_token_contents_expr(right) {
                if let Some(s) = get_string_constant(left) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
                if let Some(s) = get_string_constant(right) {
                    if !comparisons.contains(&s) {
                        comparisons.push(s);
                    }
                }
            }
        }
    }
}

/// Check for dynamic end-tag format strings: `"end%s" % bits[0]` or `f"end{bits[0]}"`.
fn has_dynamic_end_tag_format(body: &[Stmt]) -> bool {
    for stmt in body {
        match stmt {
            Stmt::Assign(StmtAssign { value, .. }) => {
                if is_end_format_expr(value) {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                // Check comparisons: `token.contents.strip() != end_tag_name`
                // where end_tag_name was assigned from a format expression
                if has_dynamic_end_tag_format(&if_stmt.body) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if has_dynamic_end_tag_format(&clause.body) {
                        return true;
                    }
                }
            }
            Stmt::While(while_stmt) => {
                if has_dynamic_end_tag_format(&while_stmt.body) {
                    return true;
                }
            }
            _ => {}
        }
    }

    // Also check top-level assignments for `end_tag_name = "end%s" % bits[0]`
    for stmt in body {
        if let Stmt::Assign(StmtAssign { value, .. }) = stmt {
            if is_end_format_expr(value) {
                return true;
            }
        }
    }

    false
}

/// Check if an expression is `"end%s" % something` or similar end-tag format.
fn is_end_format_expr(expr: &Expr) -> bool {
    // `"end%s" % bits[0]`
    if let Expr::BinOp(ExprBinOp {
        left,
        op: Operator::Mod,
        ..
    }) = expr
    {
        if let Some(s) = get_string_constant(left) {
            if s.starts_with("end") && s.contains('%') {
                return true;
            }
        }
    }
    // f"end{...}" patterns
    if is_end_fstring(expr) {
        return true;
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

    // =========================================================================
    // Simple end-tag
    // =========================================================================

    #[test]
    fn simple_end_tag_single_parse() {
        let source = r#"
def do_for(parser, token):
    nodelist = parser.parse(("endfor",))
    parser.delete_first_token()
    return ForNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endfor"));
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    // =========================================================================
    // Intermediates (if/elif/else pattern)
    // =========================================================================

    #[test]
    fn if_else_intermediates() {
        let source = r#"
def do_if(parser, token):
    nodelist_true = parser.parse(("elif", "else", "endif"))
    token = parser.next_token()
    if token.contents == "elif":
        nodelist_elif = parser.parse(("elif", "else", "endif"))
    elif token.contents == "else":
        nodelist_false = parser.parse(("endif",))
    return IfNode(nodelist_true, nodelist_false)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endif"));
        assert!(spec.intermediates.contains(&"elif".to_string()));
        assert!(spec.intermediates.contains(&"else".to_string()));
        assert!(!spec.opaque);
    }

    // =========================================================================
    // Opaque block (skip_past)
    // =========================================================================

    #[test]
    fn opaque_block_skip_past() {
        let source = r#"
def do_verbatim(parser, token):
    parser.skip_past("endverbatim")
    return VerbatimNode()
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endverbatim"));
        assert!(spec.intermediates.is_empty());
        assert!(spec.opaque);
    }

    // =========================================================================
    // Non-conventional closer names (found via control flow)
    // =========================================================================

    #[test]
    fn non_conventional_closer_found_via_control_flow() {
        // A tag that uses "done" as end-tag instead of "end*"
        let source = r#"
def do_repeat(parser, token):
    nodelist = parser.parse(("done",))
    parser.delete_first_token()
    return RepeatNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("done"));
        assert!(spec.intermediates.is_empty());
    }

    // =========================================================================
    // Ambiguous → None
    // =========================================================================

    #[test]
    fn ambiguous_returns_none_for_end_tag() {
        // Multiple non-"end*" tokens in a single parse call with no control flow clues
        let source = r#"
def do_custom(parser, token):
    nodelist = parser.parse(("stop", "halt"))
    return CustomNode(nodelist)
"#;
        let func = parse_function(source);
        // Can't determine which is the end-tag without control flow or convention
        assert!(extract_block_spec(&func).is_none());
    }

    // =========================================================================
    // Dynamic f-string end-tags
    // =========================================================================

    #[test]
    fn dynamic_fstring_end_tag() {
        let source = r#"
def do_block(parser, token):
    tag_name, *rest = token.split_contents()
    nodelist = parser.parse((f"end{tag_name}",))
    parser.delete_first_token()
    return BlockNode(tag_name, nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        // Dynamic end-tag → end_tag is None (depends on runtime)
        assert!(spec.end_tag.is_none());
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    // =========================================================================
    // Multiple parser.parse() chains
    // =========================================================================

    #[test]
    fn multiple_parse_calls_classify_correctly() {
        let source = r#"
def do_for(parser, token):
    nodelist_loop = parser.parse(("empty", "endfor"))
    token = parser.next_token()
    if token.contents == "empty":
        nodelist_empty = parser.parse(("endfor",))
        parser.delete_first_token()
    return ForNode(nodelist_loop, nodelist_empty)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endfor"));
        assert_eq!(spec.intermediates, vec!["empty".to_string()]);
        assert!(!spec.opaque);
    }

    // =========================================================================
    // No block structure
    // =========================================================================

    #[test]
    fn no_parse_calls_returns_none() {
        let source = r"
def do_now(parser, token):
    bits = token.split_contents()
    return NowNode(bits[1])
";
        let func = parse_function(source);
        assert!(extract_block_spec(&func).is_none());
    }

    // =========================================================================
    // self.parser pattern (classytags-like)
    // =========================================================================

    #[test]
    fn self_parser_pattern() {
        let source = r#"
def do_block(self, token):
    nodelist = self.parser.parse(("endblock",))
    self.parser.delete_first_token()
    return BlockNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endblock"));
    }

    // =========================================================================
    // Convention tie-breaker for single-call multi-token
    // =========================================================================

    #[test]
    fn convention_tiebreaker_single_call_multi_token() {
        // Single parse call with both "end*" and non-"end*" tokens.
        // Convention used as tie-breaker: "endif" → end-tag, "else" → intermediate
        let source = r#"
def do_if(parser, token):
    nodelist = parser.parse(("else", "endif"))
    return IfNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endif"));
        assert_eq!(spec.intermediates, vec!["else".to_string()]);
    }

    // =========================================================================
    // Django-style with nested elif
    // =========================================================================

    #[test]
    fn django_if_tag_style() {
        // Full Django if-tag pattern with while loop for multiple elif branches
        let source = r#"
def do_if(parser, token):
    nodelist = parser.parse(("elif", "else", "endif"))
    token = parser.next_token()
    while token.contents.startswith("elif"):
        nodelist_elif = parser.parse(("elif", "else", "endif"))
        token = parser.next_token()
    if token.contents == "else":
        nodelist_else = parser.parse(("endif",))
        token = parser.next_token()
    return IfNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endif"));
        // Both "elif" and "else" should be intermediates
        assert!(spec.intermediates.contains(&"elif".to_string()));
        assert!(spec.intermediates.contains(&"else".to_string()));
    }

    // =========================================================================
    // Skip past with variable reference
    // =========================================================================

    #[test]
    fn skip_past_string_constant() {
        let source = r#"
def do_comment(parser, token):
    parser.skip_past("endcomment")
    return CommentNode()
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endcomment"));
        assert!(spec.opaque);
    }

    // =========================================================================
    // Function without parser parameter
    // =========================================================================

    #[test]
    fn no_parameters_returns_none() {
        let source = r"
def helper():
    pass
";
        let func = parse_function(source);
        assert!(extract_block_spec(&func).is_none());
    }

    // =========================================================================
    // Multiple parse chains via sequential control flow
    // =========================================================================

    #[test]
    fn sequential_parse_then_check() {
        // Pattern: parse → check token → conditional parse
        let source = r#"
def do_spaceless(parser, token):
    nodelist = parser.parse(("endspaceless",))
    parser.delete_first_token()
    return SpacelessNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endspaceless"));
        assert!(spec.intermediates.is_empty());
    }

    #[test]
    fn next_token_loop_blocktrans_pattern() {
        // Django's blocktrans/blocktranslate pattern: manual token iteration
        let source = r#"
def do_block_translate(parser, token):
    bits = token.split_contents()
    singular = []
    plural = []
    while parser.tokens:
        token = parser.next_token()
        if token.token_type in (TokenType.VAR, TokenType.TEXT):
            singular.append(token)
        else:
            break
    if countervar and counter:
        if token.contents.strip() != "plural":
            raise TemplateSyntaxError("error")
        while parser.tokens:
            token = parser.next_token()
            if token.token_type in (TokenType.VAR, TokenType.TEXT):
                plural.append(token)
            else:
                break
    end_tag_name = "end%s" % bits[0]
    if token.contents.strip() != end_tag_name:
        raise TemplateSyntaxError("error")
    return BlockTranslateNode(singular, plural)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        // Dynamic end-tag ("end%s" % bits[0]) → None
        assert!(spec.end_tag.is_none());
        assert_eq!(spec.intermediates, vec!["plural".to_string()]);
        assert!(!spec.opaque);
    }

    #[test]
    fn next_token_loop_static_end_tag() {
        // A next_token loop with a static end-tag comparison
        let source = r#"
def do_custom_block(parser, token):
    content = []
    while parser.tokens:
        token = parser.next_token()
        if token.token_type == TokenType.TEXT:
            content.append(token)
        else:
            break
    if token.contents.strip() != "endcustom":
        raise TemplateSyntaxError("error")
    return CustomBlockNode(content)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endcustom"));
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    #[test]
    fn next_token_loop_with_intermediate_and_static_end() {
        // next_token loop with both an intermediate and a static end-tag
        let source = r#"
def do_custom(parser, token):
    nodes = []
    while parser.tokens:
        token = parser.next_token()
        if token.token_type in (TokenType.VAR, TokenType.TEXT):
            nodes.append(token)
        else:
            break
    if token.contents.strip() == "middle":
        more_nodes = []
        while parser.tokens:
            token = parser.next_token()
            if token.token_type in (TokenType.VAR, TokenType.TEXT):
                more_nodes.append(token)
            else:
                break
    if token.contents.strip() != "endcustom":
        raise TemplateSyntaxError("error")
    return CustomNode(nodes)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endcustom"));
        assert_eq!(spec.intermediates, vec!["middle".to_string()]);
    }

    #[test]
    fn no_next_token_loop_no_parse_returns_none() {
        // Function with no parser.parse() and no next_token loop
        let source = r"
def do_simple(parser, token):
    bits = token.split_contents()
    return SimpleNode(bits[1])
";
        let func = parse_function(source);
        assert!(extract_block_spec(&func).is_none());
    }
}
