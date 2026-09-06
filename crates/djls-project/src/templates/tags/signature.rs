use ruff_python_ast::StmtFunctionDef;

use crate::templates::RegistrationKind;
use crate::templates::registrations::ContextProvision;
use crate::templates::tags::types::AsVar;
use crate::templates::tags::types::ParameterRequirement;
use crate::templates::tags::types::TagArgument;
use crate::templates::tags::types::TagArgumentKind;
use crate::templates::tags::types::TagArgumentSyntax;
use crate::templates::tags::types::TagRule;

/// Extract a trusted callable contract for a Django tag helper.
///
/// Django validates these helpers with `parse_bits()`. The signature variant
/// retains that boundary instead of turning it into coarse count constraints.
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

    let positional_only = params
        .posonlyargs
        .len()
        .saturating_sub(required_framework_names.len());
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

    if let Some(vararg) = &params.vararg {
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
        argument_syntax: TagArgumentSyntax::Signature {
            parameters: extracted_args,
            positional_only,
            variadic_keyword: params.kwarg.as_ref().map(|kwarg| kwarg.name.to_string()),
        },
        as_var,
        ..TagRule::default()
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
        assert_eq!(
            rule.argument_syntax,
            TagArgumentSyntax::Signature {
                parameters: Vec::new(),
                positional_only: 0,
                variadic_keyword: None,
            }
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
        let parameters = rule
            .argument_syntax
            .parameters()
            .expect("simple tag should expose signature parameters");
        assert_eq!(parameters.len(), 2);
        assert!(
            parameters
                .iter()
                .all(|parameter| parameter.requirement == ParameterRequirement::Required)
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
        assert!(matches!(
            rule.argument_syntax.parameters(),
            Some([TagArgument {
                kind: TagArgumentKind::VarArgs,
                ..
            }])
        ));
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
        let parameters = rule
            .argument_syntax
            .parameters()
            .expect("context tag should expose signature parameters");
        assert_eq!(
            parameters
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>(),
            ["url", "popup", "to_field"]
        );
    }

    #[test]
    fn preserves_positional_only_keyword_only_and_variadic_keyword_shape() {
        let source = "def shaped(first, /, second='x', *, required, optional=None, **extra): pass";
        let func = find_function_in_source(source, "shaped").expect("function should exist");
        let rule = extract_parse_bits_rule(
            &func,
            RegistrationKind::SimpleTag,
            ContextProvision::None,
            AsVar::Strip,
        )
        .expect("signature should be trusted");

        let TagArgumentSyntax::Signature {
            parameters,
            positional_only,
            variadic_keyword,
        } = rule.argument_syntax
        else {
            panic!("expected trusted signature syntax");
        };
        assert_eq!(positional_only, 1);
        assert_eq!(variadic_keyword.as_deref(), Some("extra"));
        assert_eq!(
            parameters
                .iter()
                .map(|parameter| (
                    parameter.name.as_str(),
                    parameter.requirement,
                    &parameter.kind
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "first",
                    ParameterRequirement::Required,
                    &TagArgumentKind::Variable
                ),
                (
                    "second",
                    ParameterRequirement::Optional,
                    &TagArgumentKind::Variable
                ),
                (
                    "required",
                    ParameterRequirement::Required,
                    &TagArgumentKind::Keyword
                ),
                (
                    "optional",
                    ParameterRequirement::Optional,
                    &TagArgumentKind::Keyword
                ),
            ]
        );
    }
}
