use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::StmtFunctionDef;

use crate::types::FilterArity;

/// Extract filter argument arity from a filter function's signature.
///
/// Django filters receive the value being filtered as their first positional
/// argument. Some filters accept an additional argument after the colon
/// (e.g., `{{ value|default:"nothing" }}`).
///
/// This function inspects the function signature to determine:
/// - Whether the filter expects an argument beyond the value parameter
/// - Whether that argument is optional (has a default value)
///
/// The first positional parameter is always the value being filtered.
/// If the function is a method (first param is `self`), `self` is skipped
/// before identifying the value parameter.
#[must_use]
pub fn extract_filter_arity(func: &StmtFunctionDef) -> FilterArity {
    let params = &func.parameters;

    // Collect all positional parameters (posonly + regular)
    let all_positional: Vec<&ruff_python_ast::ParameterWithDefault> = params
        .posonlyargs
        .iter()
        .chain(params.args.iter())
        .collect();

    if all_positional.is_empty() {
        return FilterArity {
            expects_arg: false,
            arg_optional: false,
        };
    }

    // Skip `self` if present (method-style filter)
    let skip_self = usize::from(
        all_positional
            .first()
            .is_some_and(|p| p.parameter.name.as_str() == "self"),
    );

    // After skipping self, the first param is the value being filtered.
    // Any additional positional params are the filter's argument(s).
    let after_self = &all_positional[skip_self..];

    if after_self.len() <= 1 {
        // Only the value parameter (or no params beyond self)
        return FilterArity {
            expects_arg: false,
            arg_optional: false,
        };
    }

    // There's at least one additional parameter beyond the value.
    // Check if the extra parameter(s) have defaults.
    let extra_params = &after_self[1..];
    let all_have_defaults = extra_params.iter().all(|p| p.default.is_some());

    FilterArity {
        expects_arg: true,
        arg_optional: all_have_defaults,
    }
}

/// Check if a filter function has the `@stringfilter` decorator.
///
/// This is informational — `@stringfilter` doesn't change arity, it just
/// coerces the input value to a string before passing it to the filter.
#[must_use]
pub fn has_stringfilter_decorator(func: &StmtFunctionDef) -> bool {
    for decorator in &func.decorator_list {
        match &decorator.expression {
            Expr::Name(name) if name.id.as_str() == "stringfilter" => return true,
            Expr::Attribute(ExprAttribute { attr, .. })
                if attr.as_str() == "stringfilter" =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Check if a filter's decorator includes `is_safe=True`.
///
/// This is informational — `is_safe` doesn't change arity, it tells Django's
/// auto-escaping system that the filter's output is safe HTML.
#[must_use]
pub fn has_is_safe(func: &StmtFunctionDef) -> bool {
    for decorator in &func.decorator_list {
        if let Expr::Call(ExprCall { arguments, .. }) = &decorator.expression {
            for kw in &arguments.keywords {
                if let Some(arg) = &kw.arg {
                    if arg.as_str() == "is_safe" {
                        if let Expr::BooleanLiteral(lit) = &kw.value {
                            return lit.value;
                        }
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;
    use ruff_python_ast::Stmt;

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
    // No-arg filters (value only)
    // =========================================================================

    #[test]
    fn no_arg_filter() {
        let source = r"
@register.filter
def title(value):
    return value.title()
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn no_arg_filter_upper() {
        let source = r"
@register.filter
def upper(value):
    return value.upper()
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // =========================================================================
    // Required-arg filters
    // =========================================================================

    #[test]
    fn required_arg_filter() {
        let source = r#"
@register.filter
def cut(value, arg):
    return value.replace(arg, "")
"#;
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn required_arg_filter_add() {
        let source = r"
@register.filter
def add(value, arg):
    return value + arg
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // =========================================================================
    // Optional-arg filters
    // =========================================================================

    #[test]
    fn optional_arg_filter() {
        let source = r#"
@register.filter
def default(value, arg=""):
    if not value:
        return arg
    return value
"#;
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    #[test]
    fn optional_arg_filter_none_default() {
        let source = r"
@register.filter
def truncatewords(value, arg=None):
    return value
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // =========================================================================
    // Method-style filters (with self)
    // =========================================================================

    #[test]
    fn method_style_no_arg() {
        let source = r"
def my_filter(self, value):
    return value.upper()
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        // self is skipped, only value param → no arg expected
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn method_style_with_arg() {
        let source = r"
def my_filter(self, value, arg):
    return value + arg
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        // self is skipped, value is first, arg is extra → expects_arg
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn method_style_with_optional_arg() {
        let source = r#"
def my_filter(self, value, arg="default"):
    return value + arg
"#;
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // =========================================================================
    // Edge cases
    // =========================================================================

    #[test]
    fn no_params_at_all() {
        let source = r"
def weird_filter():
    return 'nothing'
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn self_only() {
        let source = r"
def weird_method(self):
    return 'nothing'
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn posonly_params() {
        // Python 3.8+ positional-only parameters
        let source = r"
def my_filter(value, /, arg):
    return value + arg
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn posonly_with_default() {
        let source = r#"
def my_filter(value, /, arg="x"):
    return value + arg
"#;
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    #[test]
    fn multiple_extra_args_all_with_defaults() {
        // Unusual but handle gracefully
        let source = r#"
def my_filter(value, arg1="a", arg2="b"):
    return value
"#;
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    #[test]
    fn multiple_extra_args_mixed_defaults() {
        let source = r#"
def my_filter(value, arg1, arg2="b"):
    return value
"#;
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        // Not all extra params have defaults → not optional
        assert!(!arity.arg_optional);
    }

    // =========================================================================
    // Decorator metadata (informational)
    // =========================================================================

    #[test]
    fn stringfilter_decorator() {
        let source = r"
@stringfilter
def title(value):
    return value.title()
";
        let func = parse_function(source);
        assert!(has_stringfilter_decorator(&func));
    }

    #[test]
    fn no_stringfilter_decorator() {
        let source = r"
@register.filter
def add(value, arg):
    return value + arg
";
        let func = parse_function(source);
        assert!(!has_stringfilter_decorator(&func));
    }

    #[test]
    fn is_safe_true() {
        let source = r"
@register.filter(is_safe=True)
def my_filter(value):
    return value
";
        let func = parse_function(source);
        assert!(has_is_safe(&func));
    }

    #[test]
    fn is_safe_false() {
        let source = r"
@register.filter(is_safe=False)
def my_filter(value):
    return value
";
        let func = parse_function(source);
        assert!(!has_is_safe(&func));
    }

    #[test]
    fn no_is_safe() {
        let source = r"
@register.filter
def my_filter(value):
    return value
";
        let func = parse_function(source);
        assert!(!has_is_safe(&func));
    }

    // =========================================================================
    // Arity doesn't change with decorator kwargs
    // =========================================================================

    #[test]
    fn is_safe_does_not_affect_arity() {
        let source = r"
@register.filter(is_safe=True)
def my_filter(value, arg):
    return value
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn stringfilter_does_not_affect_arity() {
        let source = r"
@stringfilter
def my_filter(value, arg):
    return value + arg
";
        let func = parse_function(source);
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }
}
