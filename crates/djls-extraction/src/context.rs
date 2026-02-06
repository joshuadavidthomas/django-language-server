use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;

/// Context information extracted from a tag registration function.
///
/// Detects key variable bindings:
/// - `split`: The variable bound to `token.split_contents()` (e.g., `bits`, `args`)
/// - `parser`: The parser parameter name (usually `parser`)
/// - `token`: The token parameter name (usually `token`)
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct FunctionContext {
    /// Variable name bound to `token.split_contents()` result
    pub split: Option<String>,
    /// Variable name for the parser parameter
    pub parser: Option<String>,
    /// Variable name for the token parameter
    pub token: Option<String>,
}

impl FunctionContext {
    /// Build context by analyzing the function definition identified by `reg`.
    pub fn from_registration(parsed: &ParsedModule, reg: &RegistrationInfo) -> Self {
        let module = parsed.ast();

        let func_def = module.body.iter().find_map(|stmt| {
            if let Stmt::FunctionDef(fd) = stmt {
                if fd.name.as_str() == reg.function_name {
                    return Some(fd);
                }
            }
            None
        });

        let Some(func_def) = func_def else {
            return Self::default();
        };

        let params = &func_def.parameters;
        let parser_var = params.args.first().map(|p| p.parameter.name.to_string());
        let token_var = params.args.get(1).map(|p| p.parameter.name.to_string());

        let split_var = find_split_contents_var(&func_def.body, token_var.as_deref());

        Self {
            split: split_var,
            parser: parser_var,
            token: token_var,
        }
    }

    /// Returns the split variable name, or `None` if not detected.
    #[allow(dead_code)]
    #[must_use]
    pub fn split_var(&self) -> Option<&str> {
        self.split.as_deref()
    }
}

/// Find the variable assigned from `<token>.split_contents()`.
///
/// Recurses into `if`/`try` blocks to find patterns like:
/// - `bits = token.split_contents()`
/// - `args = t.split_contents()`
fn find_split_contents_var(stmts: &[Stmt], token_var: Option<&str>) -> Option<String> {
    for stmt in stmts {
        match stmt {
            Stmt::Assign(assign) => {
                if is_split_contents_call(&assign.value, token_var) {
                    if let Some(Expr::Name(name)) = assign.targets.first() {
                        return Some(name.id.to_string());
                    }
                }
            }

            Stmt::If(if_stmt) => {
                if let Some(var) = find_split_contents_var(&if_stmt.body, token_var) {
                    return Some(var);
                }
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(var) = find_split_contents_var(&clause.body, token_var) {
                        return Some(var);
                    }
                }
            }

            Stmt::Try(try_stmt) => {
                if let Some(var) = find_split_contents_var(&try_stmt.body, token_var) {
                    return Some(var);
                }
            }

            _ => {}
        }
    }

    None
}

/// Check if an expression is `<token>.split_contents()`.
fn is_split_contents_call(expr: &Expr, token_var: Option<&str>) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Expr::Attribute(attr) = call.func.as_ref() else {
        return false;
    };

    if attr.attr.as_str() != "split_contents" {
        return false;
    }

    if let Some(expected_token) = token_var {
        if let Expr::Name(name) = attr.value.as_ref() {
            return name.id.as_str() == expected_token;
        }
    }

    // If we don't know the token var, accept any `.split_contents()` call
    true
}

#[cfg(test)]
#[allow(clippy::needless_raw_string_hashes)]
mod tests {
    use super::*;
    use crate::parser::parse_module;
    use crate::registry::find_registrations;

    #[test]
    fn detect_bits() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("bits"));
        assert_eq!(ctx.parser.as_deref(), Some("parser"));
        assert_eq!(ctx.token.as_deref(), Some("token"));
    }

    #[test]
    fn detect_args() {
        let source = r#"
@register.tag
def my_tag(p, t):
    args = t.split_contents()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("args"));
        assert_eq!(ctx.parser.as_deref(), Some("p"));
        assert_eq!(ctx.token.as_deref(), Some("t"));
    }

    #[test]
    fn detect_parts() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    parts = token.split_contents()
    if len(parts) < 2:
        raise Error()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("parts"));
    }

    #[test]
    fn no_split_contents_for_simple_tag() {
        let source = r#"
@register.simple_tag
def my_tag(value):
    return value
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), None);
    }

    #[test]
    fn split_contents_inside_if() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    if some_condition:
        bits = token.split_contents()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("bits"));
    }

    #[test]
    fn split_contents_inside_try() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    try:
        bits = token.split_contents()
    except Exception:
        raise TemplateSyntaxError("error")
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("bits"));
    }

    #[test]
    fn split_contents_inside_elif() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    if False:
        pass
    elif True:
        bits = token.split_contents()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("bits"));
    }

    #[test]
    fn no_params_function() {
        let source = r#"
@register.simple_tag
def no_args():
    return "hello"
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), None);
        assert_eq!(ctx.parser.as_deref(), None);
        assert_eq!(ctx.token.as_deref(), None);
    }

    #[test]
    fn wrong_method_not_detected() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.contents.split()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), None);
    }
}
