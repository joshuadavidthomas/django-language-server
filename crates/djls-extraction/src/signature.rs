use ruff_python_ast::Expr;
use ruff_python_ast::ExprCall;
use ruff_python_ast::StmtFunctionDef;

use crate::types::ArgumentCountConstraint;
use crate::types::ExtractedArg;
use crate::types::ExtractedArgKind;
use crate::types::TagRule;

/// Extract rules from a `simple_tag` or `inclusion_tag` function signature.
///
/// These tags use Django's `parse_bits` for argument validation, so we derive
/// constraints from the function signature (required params, optional params,
/// `*args`, `**kwargs`).
#[must_use]
pub fn extract_parse_bits_rule(func: &StmtFunctionDef) -> TagRule {
    let params = &func.parameters;

    let takes_context = has_takes_context(func);

    let skip = usize::from(takes_context);

    let effective_params: Vec<&ruff_python_ast::ParameterWithDefault> =
        params.args.iter().skip(skip).collect();

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

    #[test]
    fn simple_tag_no_params() {
        let source = r"
@register.simple_tag
def now():
    return datetime.now()
";
        let func = parse_function(source);
        let rule = extract_parse_bits_rule(&func);
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
        let rule = extract_parse_bits_rule(&func);
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
        let rule = extract_parse_bits_rule(&func);
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
        let rule = extract_parse_bits_rule(&func);
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
        let rule = extract_parse_bits_rule(&func);
        assert!(rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Min(2)));
    }
}
