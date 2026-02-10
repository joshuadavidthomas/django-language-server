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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::django_function;
    use crate::test_helpers::find_function_in_source;

    // No-arg filters (value only)

    // Corpus: `title` in defaultfilters.py — `def title(value):`
    #[test]
    fn no_arg_filter() {
        let func = django_function("django/template/defaultfilters.py", "title")
            .unwrap();
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Corpus: `upper` in defaultfilters.py — `def upper(value):`
    #[test]
    fn no_arg_filter_upper() {
        let func = django_function("django/template/defaultfilters.py", "upper")
            .unwrap();
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Required-arg filters

    // Corpus: `cut` in defaultfilters.py — `def cut(value, arg):`
    #[test]
    fn required_arg_filter() {
        let func =
            django_function("django/template/defaultfilters.py", "cut").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Corpus: `add` in defaultfilters.py — `def add(value, arg):`
    #[test]
    fn required_arg_filter_add() {
        let func =
            django_function("django/template/defaultfilters.py", "add").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Optional-arg filters

    // Corpus: `floatformat` in defaultfilters.py — `def floatformat(text, arg=-1):`
    #[test]
    fn optional_arg_filter() {
        let func = django_function("django/template/defaultfilters.py", "floatformat")
            .unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // Corpus: `date` in defaultfilters.py — `def date(value, arg=None):`
    #[test]
    fn optional_arg_filter_none_default() {
        let func = django_function("django/template/defaultfilters.py", "date")
            .unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // Method-style filters (with self)
    // No standard Django filter uses `self` — these test the self-skipping logic
    // for class-based filter implementations in third-party packages.

    #[test]
    fn method_style_no_arg() {
        let source = "def my_filter(self, value):\n    return value.upper()\n";
        let func = find_function_in_source(source, "my_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn method_style_with_arg() {
        let source = "def my_filter(self, value, arg):\n    return value + arg\n";
        let func = find_function_in_source(source, "my_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn method_style_with_optional_arg() {
        let source = "def my_filter(self, value, arg=\"default\"):\n    return value + arg\n";
        let func = find_function_in_source(source, "my_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // Edge cases — no real filter has zero params or self-only.
    // These test robustness of the extraction logic.

    #[test]
    fn no_params_at_all() {
        let source = "def weird_filter():\n    return 'nothing'\n";
        let func = find_function_in_source(source, "weird_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn self_only() {
        let source = "def weird_method(self):\n    return 'nothing'\n";
        let func = find_function_in_source(source, "weird_method").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Python 3.8+ positional-only parameters — no Django filter uses these
    // currently, but they are valid Python and should be handled correctly.

    #[test]
    fn posonly_params() {
        let source = "def my_filter(value, /, arg):\n    return value + arg\n";
        let func = find_function_in_source(source, "my_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn posonly_with_default() {
        let source = "def my_filter(value, /, arg=\"x\"):\n    return value + arg\n";
        let func = find_function_in_source(source, "my_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // Unusual multi-param signatures — no real Django filter has 3+ positional
    // params, but these test the "all have defaults" logic edge cases.

    #[test]
    fn multiple_extra_args_all_with_defaults() {
        let source = "def my_filter(value, arg1=\"a\", arg2=\"b\"):\n    return value\n";
        let func = find_function_in_source(source, "my_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    #[test]
    fn multiple_extra_args_mixed_defaults() {
        let source = "def my_filter(value, arg1, arg2=\"b\"):\n    return value\n";
        let func = find_function_in_source(source, "my_filter").unwrap();
        let arity = extract_filter_arity(&func);
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Arity doesn't change with decorator kwargs

    // Corpus: `addslashes` in defaultfilters.py — `@register.filter(is_safe=True)`
    // with `def addslashes(value):` (no extra arg, but proves is_safe doesn't add one)
    // We use `cut` which has `@register.filter` and a required arg, to prove
    // `is_safe` on the decorator doesn't change the arity.
    // Corpus: `floatformat` — `@register.filter(is_safe=True)` with `def floatformat(text, arg=-1):`
    #[test]
    fn is_safe_does_not_affect_arity() {
        let func = django_function("django/template/defaultfilters.py", "floatformat")
            .unwrap();
        let arity = extract_filter_arity(&func);
        // floatformat has is_safe=True on decorator; arity should reflect signature only
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // Corpus: `title` in defaultfilters.py — decorated with both
    // `@register.filter(is_safe=True)` and `@stringfilter`
    #[test]
    fn stringfilter_does_not_affect_arity() {
        let func = django_function("django/template/defaultfilters.py", "title")
            .unwrap();
        let arity = extract_filter_arity(&func);
        // title has @stringfilter decorator; arity should reflect signature only (value-only)
        assert!(!arity.expects_arg);
        assert!(!arity.arg_optional);
    }
}
