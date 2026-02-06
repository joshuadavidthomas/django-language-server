use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
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
}
