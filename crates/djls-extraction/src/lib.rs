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
pub use registry::collect_registrations_from_body;
#[cfg(feature = "parser")]
pub use registry::RegistrationInfo;
#[cfg(feature = "parser")]
pub use registry::RegistrationKind;
#[cfg(feature = "parser")]
pub use rules::extract_tag_rule;
pub use types::ArgumentCountConstraint;
pub use types::BlockTagSpec;
pub use types::ExtractedArg;
pub use types::ExtractedArgKind;
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
                    let tag_rule = rules::extract_tag_rule(func, reg.kind, &func_defs);
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
    use std::collections::BTreeMap;

    use ruff_python_parser::parse_module;
    use serde::Serialize;

    use super::*;

    /// A deterministically-ordered version of `ExtractionResult` for snapshot testing.
    ///
    /// `FxHashMap` iteration order is non-deterministic, so we convert to `BTreeMap`
    /// (sorted by `SymbolKey` string representation) before serializing.
    #[derive(Debug, Serialize)]
    struct SortedExtractionResult {
        tag_rules: BTreeMap<String, TagRule>,
        filter_arities: BTreeMap<String, FilterArity>,
        block_specs: BTreeMap<String, BlockTagSpec>,
    }

    impl From<ExtractionResult> for SortedExtractionResult {
        fn from(result: ExtractionResult) -> Self {
            let key_str = |k: &SymbolKey| format!("{}::{}", k.registration_module, k.name);
            Self {
                tag_rules: result
                    .tag_rules
                    .iter()
                    .map(|(k, v)| (key_str(k), v.clone()))
                    .collect(),
                filter_arities: result
                    .filter_arities
                    .iter()
                    .map(|(k, v)| (key_str(k), v.clone()))
                    .collect(),
                block_specs: result
                    .block_specs
                    .iter()
                    .map(|(k, v)| (key_str(k), v.clone()))
                    .collect(),
            }
        }
    }

    fn snapshot(result: ExtractionResult) -> SortedExtractionResult {
        result.into()
    }

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

    // =====================================================================
    // Golden fixture tests — end-to-end through extract_rules() with insta
    // =====================================================================

    // --- Registration discovery fixtures ---

    #[test]
    fn golden_decorator_bare_tag() {
        let source = r"
from django import template
register = template.Library()

@register.tag
def mytag(parser, token):
    return MyNode()
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_decorator_tag_with_explicit_name() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("custom_name")
def do_custom(parser, token):
    return CustomNode()
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_decorator_tag_with_name_kwarg() {
        let source = r#"
from django import template
register = template.Library()

@register.tag(name="named_tag")
def do_named(parser, token):
    return NamedNode()
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_simple_tag_no_args() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_tag
def current_time():
    return "now"
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_simple_tag_with_args() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_tag
def greet(name, greeting="Hello"):
    return f"{greeting}, {name}!"
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_simple_tag_takes_context() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_tag(takes_context=True)
def show_user(context):
    return context["user"].username
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_inclusion_tag() {
        let source = r#"
from django import template
register = template.Library()

@register.inclusion_tag("results.html")
def show_results(poll):
    return {"choices": poll.choice_set.all()}
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_inclusion_tag_takes_context() {
        let source = r#"
from django import template
register = template.Library()

@register.inclusion_tag("link.html", takes_context=True)
def jump_link(context):
    return {"link": context["home_link"]}
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_call_style_registration() {
        let source = r#"
from django import template
register = template.Library()

def do_uppercase(parser, token):
    return UpperNode()

register.tag("upper", do_uppercase)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_filter_bare_decorator() {
        let source = r"
from django import template
register = template.Library()

@register.filter
def lower(value):
    return value.lower()
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.filters")));
    }

    #[test]
    fn golden_filter_with_name_kwarg() {
        let source = r"
from django import template
register = template.Library()

@register.filter(name='cut')
def cut_filter(value, arg):
    return value.replace(arg, '')
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.filters")));
    }

    #[test]
    fn golden_filter_is_safe() {
        let source = r"
from django import template
register = template.Library()

@register.filter(is_safe=True)
def safe_lower(value):
    return value.lower()
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.filters")));
    }

    #[test]
    fn golden_multiple_registrations() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("my_tag takes one argument")
    return MyNode(bits[1])

@register.simple_tag
def simple_hello(name):
    return f"Hello, {name}!"

@register.filter
def my_filter(value, arg):
    return value + arg

@register.filter
def no_arg_filter(value):
    return value.upper()
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.mixed")));
    }

    // --- Rule extraction fixtures ---

    #[test]
    fn golden_len_exact_check() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def widthratio(parser, token):
    bits = token.split_contents()
    if len(bits) != 4:
        raise template.TemplateSyntaxError("widthratio takes three arguments")
    return WidthRatioNode(bits[1], bits[2], bits[3])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_len_min_check() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def cycle(parser, token):
    args = token.split_contents()
    if len(args) < 2:
        raise template.TemplateSyntaxError("'cycle' tag requires at least one argument")
    return CycleNode(args[1:])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_len_max_check() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def debug(parser, token):
    bits = token.split_contents()
    if len(bits) > 1:
        raise template.TemplateSyntaxError("'debug' tag takes no arguments")
    return DebugNode()
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_len_not_in_check() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def firstof(parser, token):
    bits = token.split_contents()
    if len(bits) not in (2, 3, 4):
        raise template.TemplateSyntaxError("'firstof' takes 1 to 3 arguments")
    return FirstOfNode(bits[1:])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_keyword_position_check() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def cycle(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise template.TemplateSyntaxError("'cycle' requires at least one argument")
    if bits[-1] != "as" and bits[-2] == "as":
        pass
    if len(bits) > 3 and bits[2] != "as":
        raise template.TemplateSyntaxError("Second argument to 'cycle' must be 'as'")
    return CycleNode()
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_option_loop() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("include")
def do_include(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise template.TemplateSyntaxError("'include' takes at least one argument")
    remaining_bits = bits[2:]
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option in options:
            raise template.TemplateSyntaxError("Duplicate option")
        elif option == "with":
            pass
        elif option == "only":
            pass
        else:
            raise template.TemplateSyntaxError("Unknown option")
    return IncludeNode(bits[1])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_non_bits_variable() {
        // Tests that the extraction uses the dynamically detected split variable,
        // NOT a hardcoded "bits" name
        let source = r#"
from django import template
register = template.Library()

@register.tag
def custom_tag(parser, token):
    parts = token.split_contents()
    if len(parts) != 3:
        raise template.TemplateSyntaxError("'custom_tag' requires exactly two arguments")
    return CustomNode(parts[1], parts[2])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    #[test]
    fn golden_multiple_raise_statements() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def url(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise template.TemplateSyntaxError("'url' takes at least one argument, a URL pattern name")
    if len(bits) > 4:
        raise template.TemplateSyntaxError("'url' takes at most three arguments")
    return URLNode(bits[1])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    // --- Block spec extraction fixtures ---

    #[test]
    fn golden_simple_block() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("block")
def do_block(parser, token):
    bits = token.split_contents()
    nodelist = parser.parse(("endblock",))
    parser.delete_first_token()
    return BlockNode(bits[1], nodelist)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.loader_tags"
        )));
    }

    #[test]
    fn golden_block_with_intermediates() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("if")
def do_if(parser, token):
    nodelist_true = parser.parse(("elif", "else", "endif"))
    token = parser.next_token()
    if token.contents == "elif":
        nodelist_false = parser.parse(("else", "endif"))
        parser.delete_first_token()
    elif token.contents == "else":
        nodelist_false = parser.parse(("endif",))
        parser.delete_first_token()
    return IfNode(nodelist_true, nodelist_false)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_opaque_block() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def verbatim(parser, token):
    bits = token.split_contents()
    if len(bits) != 1:
        raise template.TemplateSyntaxError("'verbatim' takes no arguments")
    parser.skip_past("endverbatim")
    return VerbatimNode()
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_for_tag_with_empty() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("for")
def do_for(parser, token):
    bits = token.split_contents()
    if len(bits) < 4:
        raise template.TemplateSyntaxError("'for' requires at least three arguments")
    nodelist_loop = parser.parse(("empty", "endfor"))
    token = parser.next_token()
    if token.contents == "empty":
        nodelist_empty = parser.parse(("endfor",))
        parser.delete_first_token()
    else:
        nodelist_empty = None
    return ForNode(bits, nodelist_loop, nodelist_empty)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    // --- Filter arity fixtures ---

    #[test]
    fn golden_filter_no_arg() {
        let source = r"
from django import template
register = template.Library()

@register.filter
def title(value):
    return value.title()
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaultfilters"
        )));
    }

    #[test]
    fn golden_filter_required_arg() {
        let source = r"
from django import template
register = template.Library()

@register.filter
def default(value, arg):
    return value or arg
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaultfilters"
        )));
    }

    #[test]
    fn golden_filter_optional_arg() {
        let source = r#"
from django import template
register = template.Library()

@register.filter
def truncatewords(value, arg=30):
    words = value.split()
    return " ".join(words[:arg])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaultfilters"
        )));
    }

    #[test]
    fn golden_filter_method_style() {
        let source = r"
from django import template
register = template.Library()

class StringFilter:
    def upper(self, value):
        return value.upper()

register.filter('upper', StringFilter().upper)
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.filters")));
    }

    // --- Edge case fixtures ---

    #[test]
    fn golden_no_split_contents() {
        // Tag compile function that doesn't call split_contents
        let source = r#"
from django import template
register = template.Library()

@register.tag
def comment(parser, token):
    parser.skip_past("endcomment")
    return CommentNode()
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_dynamic_end_tag() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("spaceless")
def do_spaceless(parser, token):
    tag_name = token.split_contents()[0]
    nodelist = parser.parse((f"end{tag_name}",))
    parser.delete_first_token()
    return SpacelessNode(nodelist)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            source,
            "django.template.defaulttags"
        )));
    }

    #[test]
    fn golden_empty_source() {
        insta::assert_yaml_snapshot!(snapshot(extract_rules("", "test.module")));
    }

    #[test]
    fn golden_invalid_python() {
        insta::assert_yaml_snapshot!(snapshot(extract_rules("def {invalid", "test.module")));
    }

    #[test]
    fn golden_no_registrations() {
        let source = r"
def helper():
    pass

class Config:
    DEBUG = True
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "test.module")));
    }

    #[test]
    fn golden_call_style_no_func_def() {
        let source = r#"
from django import template
from somewhere import do_for
register = template.Library()

register.tag("for", do_for)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "test.module")));
    }

    #[test]
    fn golden_mixed_library() {
        // A realistic library module with multiple registration styles
        let source = r#"
from django import template
register = template.Library()

@register.tag("with")
def do_with(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise template.TemplateSyntaxError("'with' requires at least one argument")
    nodelist = parser.parse(("endwith",))
    parser.delete_first_token()
    return WithNode(bits[1:], nodelist)

@register.simple_tag(takes_context=True)
def csrf_token(context):
    return context.get("csrf_token", "")

@register.filter
def length(value):
    return len(value)

@register.filter
def add(value, arg):
    return value + arg

@register.filter(name="default_if_none")
def default_if_none_filter(value, arg=""):
    if value is None:
        return arg
    return value
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.helpers")));
    }

    #[test]
    fn golden_simple_tag_with_name_kwarg() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_tag(name="get_value")
def my_get_value(key, fallback=None):
    return lookup(key, fallback)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.utils")));
    }

    #[test]
    fn golden_inclusion_tag_with_args() {
        let source = r#"
from django import template
register = template.Library()

@register.inclusion_tag("breadcrumbs.html")
def breadcrumbs(items, separator="/"):
    return {"items": items, "sep": separator}
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.nav")));
    }
}
