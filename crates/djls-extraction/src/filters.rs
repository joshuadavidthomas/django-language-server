use ruff_python_ast::ModModule;
use ruff_python_ast::Stmt;

use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::FilterArity;
use crate::ExtractionError;

/// Extract filter arity from the function signature.
pub fn extract_filter_arity(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
) -> Result<FilterArity, ExtractionError> {
    let ModModule { body, .. } = parsed.ast();

    let func_def = body.iter().find_map(|stmt| {
        if let Stmt::FunctionDef(fd) = stmt {
            if fd.name.as_str() == registration.function_name {
                return Some(fd);
            }
        }
        None
    });

    let Some(func_def) = func_def else {
        return Ok(FilterArity::Unknown);
    };

    let params = &func_def.parameters;
    let positional_count = params.args.len() + params.posonlyargs.len();

    if params.vararg.is_some() {
        return Ok(FilterArity::Unknown);
    }

    match positional_count {
        0 | 1 => Ok(FilterArity::None),
        2 => {
            // Check if the second positional arg has a default value
            // Order: posonlyargs first, then args
            let has_default = if params.posonlyargs.len() == 2 {
                params.posonlyargs[1].default.is_some()
            } else if params.posonlyargs.len() == 1 && params.args.len() == 1 {
                params.args[0].default.is_some()
            } else if params.args.len() == 2 {
                params.args[1].default.is_some()
            } else {
                false
            };

            if has_default {
                Ok(FilterArity::Optional)
            } else {
                Ok(FilterArity::Required)
            }
        }
        _ => Ok(FilterArity::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract_rules;

    #[test]
    fn test_filter_no_arg() {
        let source = r#"
@register.filter
def title(value):
    return value.title()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters[0].arity, FilterArity::None);
    }

    #[test]
    fn test_filter_required_arg() {
        let source = r#"
@register.filter
def truncatewords(value, arg):
    return value[:int(arg)]
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters[0].arity, FilterArity::Required);
    }

    #[test]
    fn test_filter_optional_arg() {
        let source = r#"
@register.filter
def default(value, arg=""):
    return value or arg
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters[0].arity, FilterArity::Optional);
    }
}
