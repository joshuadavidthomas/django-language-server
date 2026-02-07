use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtExpr;
use ruff_python_ast::StmtFunctionDef;

/// Detect the variable bound to `token.split_contents()` within a function body.
///
/// Scans the function body for assignments like:
/// - `bits = token.split_contents()`
/// - `args = token.split_contents()`
/// - `parts = token.split_contents()`
/// - `bits = parser.token.split_contents()` (indirect via `parser` parameter)
///
/// For tuple unpacking (`tag_name, *rest = token.split_contents()`), returns
/// `None` since the result isn't stored in a single variable for later `len()` checks.
///
/// Returns the variable name if found, or `None` if no `split_contents()` call
/// is detected in the function body.
#[must_use]
pub fn detect_split_var(func: &StmtFunctionDef) -> Option<String> {
    detect_split_var_in_body(&func.body)
}

fn detect_split_var_in_body(body: &[Stmt]) -> Option<String> {
    for stmt in body {
        match stmt {
            Stmt::Assign(StmtAssign { targets, value, .. }) => {
                if is_split_contents_call(value) {
                    // Single name target: `bits = token.split_contents()`
                    if targets.len() == 1 {
                        if let Expr::Name(ExprName { id, .. }) = &targets[0] {
                            return Some(id.to_string());
                        }
                    }
                    // Tuple target (e.g., `tag_name, *rest = token.split_contents()`)
                    // — the result isn't stored in a single variable, skip
                }
            }
            // Recurse into nested blocks (if/for/try/with) to find split_contents
            // in conditional branches
            Stmt::If(stmt_if) => {
                if let Some(var) = detect_split_var_in_body(&stmt_if.body) {
                    return Some(var);
                }
                for elif in &stmt_if.elif_else_clauses {
                    if let Some(var) = detect_split_var_in_body(&elif.body) {
                        return Some(var);
                    }
                }
            }
            Stmt::For(stmt_for) => {
                if let Some(var) = detect_split_var_in_body(&stmt_for.body) {
                    return Some(var);
                }
            }
            Stmt::Try(stmt_try) => {
                if let Some(var) = detect_split_var_in_body(&stmt_try.body) {
                    return Some(var);
                }
            }
            Stmt::With(stmt_with) => {
                if let Some(var) = detect_split_var_in_body(&stmt_with.body) {
                    return Some(var);
                }
            }
            _ => {}
        }
    }
    None
}

/// Check if an expression is a call to `split_contents()`.
///
/// Recognizes:
/// - `token.split_contents()` — direct call on `token`
/// - `parser.token.split_contents()` — indirect via `parser` parameter
fn is_split_contents_call(expr: &Expr) -> bool {
    let Expr::Call(ExprCall { func, .. }) = expr else {
        return false;
    };
    let Expr::Attribute(ExprAttribute { attr, value, .. }) = func.as_ref() else {
        return false;
    };
    if attr.as_str() != "split_contents" {
        return false;
    }

    // Direct: `token.split_contents()`
    if let Expr::Name(ExprName { id, .. }) = value.as_ref() {
        if id.as_str() == "token" {
            return true;
        }
    }

    // Indirect: `parser.token.split_contents()`
    if let Expr::Attribute(ExprAttribute {
        attr: inner_attr,
        value: inner_value,
        ..
    }) = value.as_ref()
    {
        if inner_attr.as_str() == "token" {
            if let Expr::Name(ExprName { id, .. }) = inner_value.as_ref() {
                if id.as_str() == "parser" {
                    return true;
                }
            }
        }
    }

    false
}

/// Check if the compile function's token parameter is passed to a helper
/// function that internally calls `split_contents()`.
///
/// Detects patterns like:
/// ```python
/// def parse_tag(token, parser):
///     bits = token.split_contents()
///     tag_name = bits.pop(0)
///     # ... returns processed data
///
/// @register.tag(name="element")
/// def do_element(parser, token):
///     tag_name, args, kwargs = parse_tag(token, parser)
///     if len(args) > 1:
///         raise TemplateSyntaxError(...)
/// ```
///
/// When this returns `true`, the variables in the compile function body
/// (e.g., `args`) have transformed semantics — they are NOT equivalent to
/// the raw `split_contents()` result. Constraint extraction should be
/// skipped to avoid false positives.
#[must_use]
pub fn token_delegated_to_helper(
    func: &StmtFunctionDef,
    module_funcs: &[&StmtFunctionDef],
) -> bool {
    // The token parameter is the second parameter of a compile function:
    //   def do_element(parser, token):
    let Some(token_param) = func.parameters.args.get(1) else {
        return false;
    };
    let token_name = token_param.parameter.name.as_str();

    // Find function calls in the body that pass the token parameter
    let mut callees = Vec::new();
    find_calls_passing_arg(&func.body, token_name, &mut callees);

    // For each callee, look it up in the module and check if it calls
    // split_contents() on the parameter that received token
    for (callee_name, token_position) in &callees {
        if let Some(callee) = module_funcs
            .iter()
            .find(|f| f.name.as_str() == callee_name.as_str())
        {
            if helper_calls_split_contents(callee, *token_position) {
                return true;
            }
        }
    }

    false
}

/// Find function calls in a statement list that pass `arg_name` as a
/// positional argument. Returns `(function_name, argument_position)` pairs.
fn find_calls_passing_arg(body: &[Stmt], arg_name: &str, results: &mut Vec<(String, usize)>) {
    for stmt in body {
        match stmt {
            Stmt::Assign(StmtAssign { value, .. }) | Stmt::Expr(StmtExpr { value, .. }) => {
                collect_calls_with_arg(value, arg_name, results);
            }
            Stmt::If(if_stmt) => {
                find_calls_passing_arg(&if_stmt.body, arg_name, results);
                for clause in &if_stmt.elif_else_clauses {
                    find_calls_passing_arg(&clause.body, arg_name, results);
                }
            }
            Stmt::For(for_stmt) => {
                find_calls_passing_arg(&for_stmt.body, arg_name, results);
            }
            Stmt::Try(try_stmt) => {
                find_calls_passing_arg(&try_stmt.body, arg_name, results);
            }
            Stmt::With(with_stmt) => {
                find_calls_passing_arg(&with_stmt.body, arg_name, results);
            }
            _ => {}
        }
    }
}

/// If `expr` is a function call that includes `arg_name` as a positional
/// argument, push `(function_name, arg_position)` into `results`.
fn collect_calls_with_arg(expr: &Expr, arg_name: &str, results: &mut Vec<(String, usize)>) {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return;
    };
    for (pos, arg) in arguments.args.iter().enumerate() {
        if let Expr::Name(ExprName { id, .. }) = arg {
            if id.as_str() == arg_name {
                if let Expr::Name(ExprName { id: func_name, .. }) = func.as_ref() {
                    results.push((func_name.to_string(), pos));
                }
            }
        }
    }
}

/// Check if a helper function calls `.split_contents()` on the parameter
/// at the given position.
///
/// For `parse_tag(token, parser)` where `token` was passed at position 0,
/// this checks that `parse_tag`'s first parameter is used in a
/// `param.split_contents()` call.
fn helper_calls_split_contents(func: &StmtFunctionDef, token_param_position: usize) -> bool {
    let Some(param) = func.parameters.args.get(token_param_position) else {
        return false;
    };
    let param_name = param.parameter.name.as_str();
    body_has_split_contents_on(&func.body, param_name)
}

/// Recursively check if a body contains `var_name.split_contents()`.
fn body_has_split_contents_on(body: &[Stmt], var_name: &str) -> bool {
    for stmt in body {
        match stmt {
            Stmt::Assign(StmtAssign { value, .. }) | Stmt::Expr(StmtExpr { value, .. }) => {
                if is_split_contents_call_on(value, var_name) {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if body_has_split_contents_on(&if_stmt.body, var_name) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if body_has_split_contents_on(&clause.body, var_name) {
                        return true;
                    }
                }
            }
            Stmt::For(for_stmt) => {
                if body_has_split_contents_on(&for_stmt.body, var_name) {
                    return true;
                }
            }
            Stmt::Try(try_stmt) => {
                if body_has_split_contents_on(&try_stmt.body, var_name) {
                    return true;
                }
            }
            Stmt::With(with_stmt) => {
                if body_has_split_contents_on(&with_stmt.body, var_name) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Check if an expression is `var_name.split_contents()`.
fn is_split_contents_call_on(expr: &Expr, var_name: &str) -> bool {
    let Expr::Call(ExprCall { func, .. }) = expr else {
        return false;
    };
    let Expr::Attribute(ExprAttribute { attr, value, .. }) = func.as_ref() else {
        return false;
    };
    if attr.as_str() != "split_contents" {
        return false;
    }
    if let Expr::Name(ExprName { id, .. }) = value.as_ref() {
        return id.as_str() == var_name;
    }
    false
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

    #[test]
    fn bits_equals_token_split_contents() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError('too few args')
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func).as_deref(), Some("bits"));
    }

    #[test]
    fn args_equals_token_split_contents() {
        let source = r"
def do_tag(parser, token):
    args = token.split_contents()
    return args
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func).as_deref(), Some("args"));
    }

    #[test]
    fn parts_equals_token_split_contents() {
        let source = r"
def do_tag(parser, token):
    parts = token.split_contents()
    return parts
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func).as_deref(), Some("parts"));
    }

    #[test]
    fn tuple_unpacking_returns_none() {
        let source = r"
def do_tag(parser, token):
    tag_name, *rest = token.split_contents()
    return rest
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func), None);
    }

    #[test]
    fn no_split_contents_returns_none() {
        let source = r"
def do_tag(parser, token):
    name = token.contents
    return name
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func), None);
    }

    #[test]
    fn split_contents_via_parser_token() {
        let source = r"
def do_tag(parser, token):
    bits = parser.token.split_contents()
    return bits
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func).as_deref(), Some("bits"));
    }

    #[test]
    fn split_contents_in_nested_if() {
        let source = r"
def do_tag(parser, token):
    if True:
        bits = token.split_contents()
    return bits
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func).as_deref(), Some("bits"));
    }

    #[test]
    fn split_contents_in_try_block() {
        let source = r"
def do_tag(parser, token):
    try:
        tokens = token.split_contents()
    except Exception:
        pass
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func).as_deref(), Some("tokens"));
    }

    #[test]
    fn first_assignment_wins() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    args = token.split_contents()
    return bits
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func).as_deref(), Some("bits"));
    }

    #[test]
    fn unrelated_split_ignored() {
        let source = r"
def do_tag(parser, token):
    bits = 'hello'.split(',')
    return bits
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func), None);
    }

    #[test]
    fn empty_function_body() {
        let source = r"
def do_tag(parser, token):
    pass
";
        let func = parse_function(source);
        assert_eq!(detect_split_var(&func), None);
    }

    // =========================================================================
    // token_delegated_to_helper tests
    // =========================================================================

    fn parse_all_functions(source: &str) -> Vec<StmtFunctionDef> {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        module
            .body
            .into_iter()
            .filter_map(|stmt| {
                if let Stmt::FunctionDef(func_def) = stmt {
                    Some(func_def)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn delegation_allauth_parse_tag_pattern() {
        // Modeled on allauth's parse_tag + do_element pattern
        let source = r#"
def parse_tag(token, parser):
    bits = token.split_contents()
    tag_name = bits.pop(0)
    args = []
    kwargs = {}
    for bit in bits:
        match = kwarg_re.match(bit)
        if match and match.group(1):
            key, value = match.groups()
            kwargs[key] = value
        else:
            args.append(bit)
    return (tag_name, args, kwargs)

def do_element(parser, token):
    tag_name, args, kwargs = parse_tag(token, parser)
    if len(args) > 1:
        raise TemplateSyntaxError("too many args")
    return ElementNode(args[0], kwargs)
"#;
        let funcs = parse_all_functions(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();
        let compile_func = funcs
            .iter()
            .find(|f| f.name.as_str() == "do_element")
            .unwrap();

        assert!(token_delegated_to_helper(compile_func, &func_refs));
    }

    #[test]
    fn delegation_not_triggered_for_direct_split_contents() {
        let source = r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError('too few')
";
        let funcs = parse_all_functions(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();
        let compile_func = &funcs[0];

        // No delegation — split_contents is called directly
        assert!(!token_delegated_to_helper(compile_func, &func_refs));
    }

    #[test]
    fn delegation_not_triggered_when_helper_has_no_split_contents() {
        let source = r"
def some_helper(tok, parser):
    return tok.contents

def do_tag(parser, token):
    result = some_helper(token, parser)
";
        let funcs = parse_all_functions(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();
        let compile_func = funcs.iter().find(|f| f.name.as_str() == "do_tag").unwrap();

        // Helper doesn't call split_contents
        assert!(!token_delegated_to_helper(compile_func, &func_refs));
    }

    #[test]
    fn delegation_not_triggered_when_helper_not_in_module() {
        let source = r"
def do_tag(parser, token):
    tag_name, args, kwargs = parse_tag(token, parser)
    return MyNode(args)
";
        let funcs = parse_all_functions(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();
        let compile_func = &funcs[0];

        // parse_tag is not defined in this module
        assert!(!token_delegated_to_helper(compile_func, &func_refs));
    }

    #[test]
    fn delegation_tracks_correct_parameter_position() {
        // token is passed at position 1 (second arg), so the helper's
        // second parameter should be checked for split_contents
        let source = r"
def my_parser(parser, tok):
    bits = tok.split_contents()
    return bits

def do_tag(parser, token):
    result = my_parser(parser, token)
";
        let funcs = parse_all_functions(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();
        let compile_func = funcs.iter().find(|f| f.name.as_str() == "do_tag").unwrap();

        assert!(token_delegated_to_helper(compile_func, &func_refs));
    }

    #[test]
    fn delegation_wrong_parameter_position_no_match() {
        // token is passed at position 0, but the helper calls split_contents
        // on its second parameter — should NOT match
        let source = r"
def my_parser(something, parser_obj):
    bits = parser_obj.split_contents()
    return bits

def do_tag(parser, token):
    result = my_parser(token, parser)
";
        let funcs = parse_all_functions(source);
        let func_refs: Vec<&StmtFunctionDef> = funcs.iter().collect();
        let compile_func = funcs.iter().find(|f| f.name.as_str() == "do_tag").unwrap();

        // split_contents is called on the wrong parameter
        assert!(!token_delegated_to_helper(compile_func, &func_refs));
    }
}
