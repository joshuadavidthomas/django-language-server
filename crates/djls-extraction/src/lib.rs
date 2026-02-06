mod types;

#[cfg(feature = "parser")]
mod blocks;
#[cfg(feature = "parser")]
mod context;
#[cfg(feature = "parser")]
mod filters;
#[cfg(feature = "parser")]
mod registry;
#[cfg(feature = "parser")]
mod rules;

#[cfg(feature = "parser")]
pub use blocks::extract_block_spec;
#[cfg(feature = "parser")]
pub use context::detect_split_var;
#[cfg(feature = "parser")]
pub use filters::extract_filter_arity;
#[cfg(feature = "parser")]
pub use filters::has_is_safe;
#[cfg(feature = "parser")]
pub use filters::has_stringfilter_decorator;
#[cfg(feature = "parser")]
pub use registry::collect_registrations;
#[cfg(feature = "parser")]
pub use registry::collect_registrations_from_body;
#[cfg(feature = "parser")]
pub use registry::RegistrationInfo;
#[cfg(feature = "parser")]
pub use registry::RegistrationKind;
#[cfg(feature = "parser")]
pub use rules::extract_tag_rule;
pub use types::ArgumentCountConstraint;
pub use types::BlockTagSpec;
pub use types::ExtractionResult;
pub use types::FilterArity;
pub use types::KnownOptions;
pub use types::RequiredKeyword;
pub use types::SymbolKey;
pub use types::SymbolKind;
pub use types::TagRule;

/// Extract validation rules from a Python registration module source.
///
/// Parses the source with Ruff's Python parser, walks the AST to find
/// `@register.tag` / `@register.filter` decorators, and extracts validation
/// semantics (argument counts, block structure, option constraints) from the
/// associated compile functions.
///
/// The `module_path` parameter is the dotted Python module path (e.g.,
/// `"django.template.defaulttags"`) used as the `registration_module` field
/// in `SymbolKey`s. Pass an empty string if unknown.
///
/// Returns an `ExtractionResult` mapping each discovered `SymbolKey` to its
/// extracted rules.
#[cfg(feature = "parser")]
#[must_use]
pub fn extract_rules(source: &str, module_path: &str) -> ExtractionResult {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return ExtractionResult::default();
    };
    let module = parsed.into_syntax();

    let registrations = registry::collect_registrations_from_body(&module.body);

    let func_defs: Vec<&ruff_python_ast::StmtFunctionDef> = collect_func_defs(&module.body);

    let mut result = ExtractionResult::default();

    for reg in &registrations {
        let func_def = reg
            .func_name
            .as_deref()
            .and_then(|name| func_defs.iter().find(|f| f.name.as_str() == name).copied());

        let key = SymbolKey {
            registration_module: module_path.to_string(),
            name: reg.name.clone(),
            kind: reg.kind.symbol_kind(),
        };

        match reg.kind {
            RegistrationKind::Filter => {
                if let Some(func) = func_def {
                    result
                        .filter_arities
                        .insert(key, filters::extract_filter_arity(func));
                }
            }
            RegistrationKind::Tag
            | RegistrationKind::SimpleTag
            | RegistrationKind::InclusionTag
            | RegistrationKind::SimpleBlockTag => {
                if let Some(func) = func_def {
                    let tag_rule = rules::extract_tag_rule(func, reg.kind);
                    if !tag_rule.arg_constraints.is_empty()
                        || !tag_rule.required_keywords.is_empty()
                        || tag_rule.known_options.is_some()
                    {
                        result.tag_rules.insert(key.clone(), tag_rule);
                    }

                    if let Some(block_spec) = blocks::extract_block_spec(func) {
                        result.block_specs.insert(key, block_spec);
                    }
                }
            }
        }
    }

    result
}

/// Recursively collect all function definitions from a module body.
#[cfg(feature = "parser")]
fn collect_func_defs(body: &[ruff_python_ast::Stmt]) -> Vec<&ruff_python_ast::StmtFunctionDef> {
    let mut defs = Vec::new();
    for stmt in body {
        match stmt {
            ruff_python_ast::Stmt::FunctionDef(func) => {
                defs.push(func);
            }
            ruff_python_ast::Stmt::ClassDef(class) => {
                defs.extend(collect_func_defs(&class.body));
            }
            _ => {}
        }
    }
    defs
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    #[test]
    fn smoke_test_ruff_parser() {
        let source = r#"
from django import template

register = template.Library()

@register.simple_tag
def hello():
    return "Hello, world!"
"#;

        let result = parse_module(source);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        let module = parsed.into_syntax();
        assert!(!module.body.is_empty());
    }

    #[test]
    fn extract_rules_simple_tag() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_tag
def hello(name):
    return f"Hello, {name}!"
"#;
        let result = extract_rules(source, "myapp.templatetags.custom");
        // simple_tag with one param (minus takes_context) → should have an arg constraint
        assert!(!result.is_empty());
    }

    #[test]
    fn extract_rules_filter() {
        let source = r"
from django import template
register = template.Library()

@register.filter
def lower(value):
    return value.lower()
";
        let result = extract_rules(source, "myapp.templatetags.custom");
        let key = SymbolKey::filter("myapp.templatetags.custom", "lower");
        assert!(result.filter_arities.contains_key(&key));
        let arity = &result.filter_arities[&key];
        assert!(!arity.expects_arg);
    }

    #[test]
    fn extract_rules_filter_with_arg() {
        let source = r"
from django import template
register = template.Library()

@register.filter
def default(value, arg):
    return value or arg
";
        let result = extract_rules(source, "test.module");
        let key = SymbolKey::filter("test.module", "default");
        assert!(result.filter_arities.contains_key(&key));
        let arity = &result.filter_arities[&key];
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn extract_rules_block_tag() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("myblock")
def do_myblock(parser, token):
    nodelist = parser.parse(("endmyblock",))
    parser.delete_first_token()
    return MyBlockNode(nodelist)
"#;
        let result = extract_rules(source, "test.module");
        let key = SymbolKey::tag("test.module", "myblock");
        assert!(
            result.block_specs.contains_key(&key),
            "should extract block spec for myblock"
        );
        let spec = &result.block_specs[&key];
        assert_eq!(spec.end_tag.as_deref(), Some("endmyblock"));
    }

    #[test]
    fn extract_rules_empty_source() {
        let result = extract_rules("", "test.module");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_rules_invalid_python() {
        let result = extract_rules("def {invalid python", "test.module");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_rules_no_registrations() {
        let source = r"
def regular_function():
    pass

class MyClass:
    pass
";
        let result = extract_rules(source, "test.module");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_rules_multiple_registrations() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("...")
    return MyNode()

@register.filter
def my_filter(value, arg):
    return value + arg
"#;
        let result = extract_rules(source, "test.module");
        let tag_key = SymbolKey::tag("test.module", "my_tag");
        let filter_key = SymbolKey::filter("test.module", "my_filter");

        assert!(
            result.tag_rules.contains_key(&tag_key),
            "should extract tag rule"
        );
        assert!(
            result.filter_arities.contains_key(&filter_key),
            "should extract filter arity"
        );
    }

    #[test]
    fn extract_rules_call_style_registration_no_func_def() {
        // Call-style registration where the function def isn't in the same file
        let source = r#"
from django import template
from somewhere import do_for
register = template.Library()

register.tag("for", do_for)
"#;
        let result = extract_rules(source, "test.module");
        // Registration found but no matching func def → no rules extracted
        assert!(result.tag_rules.is_empty());
        assert!(result.block_specs.is_empty());
    }
}
