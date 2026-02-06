use ruff_python_ast::Stmt;

use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::FilterArity;
use crate::ExtractionError;

#[allow(clippy::unnecessary_wraps)]
pub fn extract_filter_arity(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
) -> Result<FilterArity, ExtractionError> {
    let module = parsed.ast();

    let func_def = module.body.iter().find_map(|stmt| {
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
            let last_arg = params.args.last();
            let has_default = last_arg.is_some_and(|arg| arg.default.is_some());
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
    use crate::extract_rules;
    use crate::types::FilterArity;

    #[test]
    fn filter_no_arg() {
        let source = r"
@register.filter
def title(value):
    return value.title()
";
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters.len(), 1);
        assert_eq!(result.filters[0].arity, FilterArity::None);
    }

    #[test]
    fn filter_required_arg() {
        let source = r"
@register.filter
def truncatewords(value, arg):
    return value[:int(arg)]
";
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters.len(), 1);
        assert_eq!(result.filters[0].arity, FilterArity::Required);
    }

    #[test]
    fn filter_optional_arg() {
        let source = r#"
@register.filter
def default(value, arg=""):
    return value or arg
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters.len(), 1);
        assert_eq!(result.filters[0].arity, FilterArity::Optional);
    }

    #[test]
    fn filter_no_params() {
        let source = r"
@register.filter
def noop():
    return ''
";
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters.len(), 1);
        assert_eq!(result.filters[0].arity, FilterArity::None);
    }

    #[test]
    fn filter_vararg() {
        let source = r"
@register.filter
def weird(value, *args):
    return value
";
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters.len(), 1);
        assert_eq!(result.filters[0].arity, FilterArity::Unknown);
    }

    #[test]
    fn filter_too_many_params() {
        let source = r"
@register.filter
def odd(a, b, c):
    return a
";
        let result = extract_rules(source).unwrap();
        assert_eq!(result.filters.len(), 1);
        assert_eq!(result.filters[0].arity, FilterArity::Unknown);
    }
}
