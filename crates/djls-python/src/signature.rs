use ruff_python_ast::Expr;
use ruff_python_ast::ExprCall;
use ruff_python_ast::StmtFunctionDef;

use crate::ext::ExprExt;
use crate::types::ArgumentCountConstraint;
use crate::types::AsVar;
use crate::types::ExtractedArg;
use crate::types::ExtractedArgKind;
use crate::types::TagRule;

/// Extract rules from a `simple_tag` or `inclusion_tag` function signature.
///
/// These tags use Django's `parse_bits` for argument validation, so we derive
/// constraints from the function signature (required params, optional params,
/// `*args`, `**kwargs`).
///
/// `as_var` controls whether Django's framework strips trailing
/// `as <varname>` before argument validation.
#[must_use]
pub(crate) fn extract_parse_bits_rule(func: &StmtFunctionDef, as_var: AsVar) -> TagRule {
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

    TagRule {
        arg_constraints,
        required_keywords: Vec::new(),
        choice_at_constraints: Vec::new(),
        known_options: None,
        extracted_args,
        as_var,
    }
}

/// Check if a function's decorator includes `takes_context=True`.
fn has_takes_context(func: &StmtFunctionDef) -> bool {
    for decorator in &func.decorator_list {
        if let Expr::Call(ExprCall { arguments, .. }) = &decorator.expression {
            for kw in &arguments.keywords {
                if let Some(arg) = &kw.arg {
                    if arg.as_str() == "takes_context" && kw.value.is_true_literal() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::django_function;
    use crate::testing::find_function_in_source;

    // Corpus: `no_params` in custom.py — `def no_params():`
    // No params → only Max constraint
    #[test]
    fn simple_tag_no_params() {
        let func =
            django_function("tests/template_tests/templatetags/custom.py", "no_params").unwrap();
        let rule = extract_parse_bits_rule(&func, AsVar::Strip);
        assert!(rule
            .arg_constraints
            .iter()
            .all(|c| matches!(c, ArgumentCountConstraint::Max(_))));
    }

    // Corpus: `simple_two_params` in custom.py — `def simple_two_params(one, two):`
    // Two required params → Min(3) (tag name + 2 args)
    #[test]
    fn simple_tag_required_params() {
        let func = django_function(
            "tests/template_tests/templatetags/custom.py",
            "simple_two_params",
        )
        .unwrap();
        let rule = extract_parse_bits_rule(&func, AsVar::Strip);
        assert!(rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Min(3)));
    }

    // Corpus: `simple_one_default` in custom.py — `def simple_one_default(one, two="hi"):`
    // One required, one optional → Min(2)
    #[test]
    fn simple_tag_with_defaults() {
        let func = django_function(
            "tests/template_tests/templatetags/custom.py",
            "simple_one_default",
        )
        .unwrap();
        let rule = extract_parse_bits_rule(&func, AsVar::Strip);
        assert!(rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Min(2)));
    }

    // Fabricated: `*args` on simple_tag is uncommon in real Django code.
    // No corpus equivalent found. Tests that varargs removes Max constraint.
    #[test]
    fn simple_tag_with_varargs() {
        let source = r"
@register.simple_tag
def concat(*args):
    return ''.join(str(a) for a in args)
";
        let func = find_function_in_source(source, "concat").unwrap();
        let rule = extract_parse_bits_rule(&func, AsVar::Strip);
        assert!(!rule
            .arg_constraints
            .iter()
            .any(|c| matches!(c, ArgumentCountConstraint::Max(_))));
    }

    // Corpus: `add_preserved_filters` in admin_urls.py —
    // `def add_preserved_filters(context, url, popup=False, to_field=None):`
    // takes_context=True skips `context` param → 1 required (url), 2 optional → Min(2)
    #[test]
    fn simple_tag_takes_context() {
        let func = django_function(
            "django/contrib/admin/templatetags/admin_urls.py",
            "add_preserved_filters",
        )
        .unwrap();
        let rule = extract_parse_bits_rule(&func, AsVar::Strip);
        assert!(rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Min(2)));
    }
}
