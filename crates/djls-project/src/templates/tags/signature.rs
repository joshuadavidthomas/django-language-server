use ruff_python_ast::StmtFunctionDef;

use crate::templates::RegistrationKind;
use crate::templates::registrations::ContextProvision;
use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::AsVar;
use crate::templates::tags::types::ParameterRequirement;
use crate::templates::tags::types::TagArgument;
use crate::templates::tags::types::TagArgumentKind;
use crate::templates::tags::types::TagArgumentSyntax;
use crate::templates::tags::types::TagRule;

/// Extract rules from a `simple_tag` or `inclusion_tag` function signature.
///
/// These tags use Django's `parse_bits` for argument validation, so we derive
/// constraints from the function signature (required params, optional params,
/// `*args`, `**kwargs`).
///
/// `as_var` controls whether Django's framework strips trailing
/// `as <varname>` before argument validation.
#[must_use]
pub(crate) fn extract_parse_bits_rule(
    func: &StmtFunctionDef,
    kind: RegistrationKind,
    context: ContextProvision,
    as_var: AsVar,
) -> Option<TagRule> {
    let params = &func.parameters;
    let required_framework_names: &[&str] = match (kind, context) {
        (_, ContextProvision::Unknown) | (RegistrationKind::Tag | RegistrationKind::Filter, _) => {
            return None;
        }
        (RegistrationKind::SimpleTag | RegistrationKind::InclusionTag, ContextProvision::None) => {
            &[]
        }
        (
            RegistrationKind::SimpleTag | RegistrationKind::InclusionTag,
            ContextProvision::Context,
        ) => &["context"],
        (RegistrationKind::SimpleBlockTag, ContextProvision::None) => &["content"],
        (RegistrationKind::SimpleBlockTag, ContextProvision::Context) => &["context", "content"],
    };
    let combined: Vec<&ruff_python_ast::ParameterWithDefault> =
        params.posonlyargs.iter().chain(&params.args).collect();
    if !combined
        .iter()
        .zip(required_framework_names)
        .all(|(parameter, required)| parameter.parameter.name.as_str() == *required)
        || combined.len() < required_framework_names.len()
    {
        return None;
    }
    let effective_params: Vec<_> = combined
        .into_iter()
        .skip(required_framework_names.len())
        .collect();

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
    for param in effective_params {
        let name = param.parameter.name.to_string();
        let requirement = if param.default.is_none() {
            ParameterRequirement::Required
        } else {
            ParameterRequirement::Optional
        };
        extracted_args.push(TagArgument {
            name,
            requirement,
            kind: TagArgumentKind::Variable,
        });
    }

    if has_varargs && let Some(vararg) = &params.vararg {
        extracted_args.push(TagArgument {
            name: vararg.name.to_string(),
            requirement: ParameterRequirement::Optional,
            kind: TagArgumentKind::VarArgs,
        });
    }

    for kwonly in &params.kwonlyargs {
        let name = kwonly.parameter.name.to_string();
        let requirement = if kwonly.default.is_none() {
            ParameterRequirement::Required
        } else {
            ParameterRequirement::Optional
        };
        extracted_args.push(TagArgument {
            name,
            requirement,
            kind: TagArgumentKind::Keyword,
        });
    }

    Some(TagRule {
        arg_constraints,
        required_keywords: Vec::new(),
        choice_at_constraints: Vec::new(),
        known_options: None,
        diagnostic_messages: None,
        argument_syntax: TagArgumentSyntax::Parameters(extracted_args),
        as_var,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::tags::testing::django_function;
    use crate::templates::tags::testing::find_function_in_source;

    // Corpus: `no_params` in custom.py — `def no_params():`
    // No params → only Max constraint
    #[test]
    fn simple_tag_no_params() {
        let func = django_function("tests/template_tests/templatetags/custom.py", "no_params")
            .expect("expected Django fixture function should exist");
        let rule = extract_parse_bits_rule(
            &func,
            RegistrationKind::SimpleTag,
            ContextProvision::None,
            AsVar::Strip,
        )
        .expect("simple tag signature should be trusted");
        assert!(
            rule.arg_constraints
                .iter()
                .all(|c| matches!(c, ArgumentCountConstraint::Max(_)))
        );
    }

    // Corpus: `simple_two_params` in custom.py — `def simple_two_params(one, two):`
    // Two required params → Min(3) (tag name + 2 args)
    #[test]
    fn simple_tag_required_params() {
        let func = django_function(
            "tests/template_tests/templatetags/custom.py",
            "simple_two_params",
        )
        .expect("expected Django fixture function should exist");
        let rule = extract_parse_bits_rule(
            &func,
            RegistrationKind::SimpleTag,
            ContextProvision::None,
            AsVar::Strip,
        )
        .expect("simple tag signature should be trusted");
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::Min(3))
        );
    }

    // Corpus: `simple_one_default` in custom.py — `def simple_one_default(one, two="hi"):`
    // One required, one optional → Min(2)
    #[test]
    fn simple_tag_with_defaults() {
        let func = django_function(
            "tests/template_tests/templatetags/custom.py",
            "simple_one_default",
        )
        .expect("expected Django fixture function should exist");
        let rule = extract_parse_bits_rule(
            &func,
            RegistrationKind::SimpleTag,
            ContextProvision::None,
            AsVar::Strip,
        )
        .expect("simple tag signature should be trusted");
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::Min(2))
        );
        let parameters = rule
            .argument_syntax
            .parameters()
            .expect("simple tag should expose signature parameters");
        assert_eq!(parameters[0].requirement, ParameterRequirement::Required);
        assert_eq!(parameters[1].requirement, ParameterRequirement::Optional);
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
        let func = find_function_in_source(source, "concat")
            .expect("expected function should exist in test source");
        let rule = extract_parse_bits_rule(
            &func,
            RegistrationKind::SimpleTag,
            ContextProvision::None,
            AsVar::Strip,
        )
        .expect("simple tag signature should be trusted");
        assert!(
            !rule
                .arg_constraints
                .iter()
                .any(|c| matches!(c, ArgumentCountConstraint::Max(_)))
        );
    }

    #[test]
    fn block_tag_removes_framework_parameters() {
        let source = r"
def panel(context, content, title='Title'):
    pass
";
        let func = find_function_in_source(source, "panel").expect("function should exist");
        let rule = extract_parse_bits_rule(
            &func,
            RegistrationKind::SimpleBlockTag,
            ContextProvision::Context,
            AsVar::Strip,
        )
        .expect("framework parameters should match");
        let parameters = rule
            .argument_syntax
            .parameters()
            .expect("signature parameters");
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0].name, "title");
    }

    #[test]
    fn malformed_or_unknown_framework_parameters_have_no_rule() {
        let source = "def panel(body, title): pass";
        let func = find_function_in_source(source, "panel").expect("function should exist");
        assert!(
            extract_parse_bits_rule(
                &func,
                RegistrationKind::SimpleBlockTag,
                ContextProvision::None,
                AsVar::Strip,
            )
            .is_none()
        );
        assert!(
            extract_parse_bits_rule(
                &func,
                RegistrationKind::SimpleBlockTag,
                ContextProvision::Unknown,
                AsVar::Strip,
            )
            .is_none()
        );
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
        .expect("expected Django fixture function should exist");
        let rule = extract_parse_bits_rule(
            &func,
            RegistrationKind::SimpleTag,
            ContextProvision::Context,
            AsVar::Strip,
        )
        .expect("context simple tag signature should be trusted");
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::Min(2))
        );
    }
}
