use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;

/// Function context containing detected variable names.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct FunctionContext {
    /// Name of the variable bound to `token.split_contents()`
    pub split_var: Option<String>,
    /// Name of the parser parameter
    pub parser_var: Option<String>,
    /// Name of the token parameter
    pub token_var: Option<String>,
}

impl FunctionContext {
    /// Build function context from a registration, detecting split-contents variable.
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

        // Extract parameter names (first two positional params)
        let params = &func_def.parameters;
        let parser_var = params.args.first().map(|p| p.parameter.name.to_string());
        let token_var = params.args.get(1).map(|p| p.parameter.name.to_string());

        // Find split_var by looking for: <var> = <token>.split_contents()
        let split_var = find_split_contents_var(&func_def.body, token_var.as_deref());

        Self {
            split_var,
            parser_var,
            token_var,
        }
    }

    /// Returns the split variable name, or None if not detected.
    #[allow(dead_code)]
    pub fn split_var(&self) -> Option<&str> {
        self.split_var.as_deref()
    }
}

/// Find the variable assigned from `<token>.split_contents()`.
///
/// Looks for patterns like:
/// - `bits = token.split_contents()`
/// - `args = t.split_contents()`
fn find_split_contents_var(stmts: &[Stmt], token_var: Option<&str>) -> Option<String> {
    for stmt in stmts {
        match stmt {
            Stmt::Assign(assign) => {
                if is_split_contents_call(&assign.value, token_var) {
                    // Get the first target (simple name assignment)
                    if let Some(Expr::Name(name)) = assign.targets.first() {
                        return Some(name.id.to_string());
                    }
                }
            }

            // Recurse into control flow
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
    let Expr::Call(call) = expr else { return false };
    let Expr::Attribute(attr) = call.func.as_ref() else { return false };

    if attr.attr.as_str() != "split_contents" {
        return false;
    }

    // If we know the token var, verify it matches
    if let Some(expected_token) = token_var {
        if let Expr::Name(name) = attr.value.as_ref() {
            return name.id.as_str() == expected_token;
        }
        // If the value is not a simple name, we can't match
        return false;
    }

    // If we don't know the token var, accept any .split_contents() call
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_module;
    use crate::registry::find_registrations;

    #[test]
    fn test_detect_bits() {
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
        assert_eq!(ctx.parser_var.as_deref(), Some("parser"));
        assert_eq!(ctx.token_var.as_deref(), Some("token"));
    }

    #[test]
    fn test_detect_args() {
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
        assert_eq!(ctx.parser_var.as_deref(), Some("p"));
        assert_eq!(ctx.token_var.as_deref(), Some("t"));
    }

    #[test]
    fn test_detect_parts() {
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
    fn test_no_split_contents() {
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
    fn test_detect_tokens() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    tokens = token.split_contents()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("tokens"));
    }

    #[test]
    fn test_detect_in_try_block() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    try:
        bits = token.split_contents()
    except:
        pass
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("bits"));
    }

    #[test]
    fn test_detect_in_if_block() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    if True:
        args = token.split_contents()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        assert_eq!(ctx.split_var(), Some("args"));
    }

    #[test]
    fn test_wrong_variable_not_detected() {
        // If token_var is 'token', we should NOT detect 'other.split_contents()'
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = other.split_contents()
    return Node()
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();
        let ctx = FunctionContext::from_registration(&parsed, &regs.tags[0]);

        // Should be None because we're calling 'other.split_contents()', not 'token.split_contents()'
        assert_eq!(ctx.split_var(), None);
    }
}
