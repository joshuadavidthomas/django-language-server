use ruff_python_ast::Decorator;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_text_size::Ranged;

use crate::parser::ParsedModule;
use crate::types::DecoratorKind;
use crate::ExtractionError;

/// Official Django `template.Library` decorator methods for tags.
const TAG_DECORATORS: &[&str] = &["tag", "simple_tag", "inclusion_tag", "simple_block_tag"];

/// Official Django `template.Library` decorator methods for filters.
const FILTER_DECORATORS: &[&str] = &["filter"];

/// Helper/wrapper decorator functions (NOT `register.<method>` but recognized
/// as registration signals). These are typically library-specific wrappers that
/// delegate to official decorators.
///
/// Example: `@register_simple_block_tag(...)` (pretix pattern)
const TAG_HELPER_DECORATORS: &[&str] = &["register_simple_block_tag"];

/// Information about a discovered registration decorator.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RegistrationInfo {
    /// The registered name (tag/filter name in templates)
    pub name: String,
    /// Kind of decorator
    pub decorator_kind: DecoratorKind,
    /// Name of the decorated Python function
    pub function_name: String,
    /// Byte offset of the decorator in the source
    pub offset: usize,
    /// For `simple_block_tag`: explicitly provided `end_name` argument (if any).
    /// When present, this is the authoritative closer — no inference needed.
    /// When absent, Django defaults to `f"end{function_name}"` at runtime.
    pub explicit_end_name: Option<String>,
}

/// All registrations found in a module.
#[derive(Debug, Clone, Default)]
pub struct FoundRegistrations {
    pub tags: Vec<RegistrationInfo>,
    pub filters: Vec<RegistrationInfo>,
}

/// Find all tag and filter registrations in a parsed Python module.
#[allow(clippy::unnecessary_wraps)]
pub fn find_registrations(
    parsed: &ParsedModule,
) -> Result<FoundRegistrations, ExtractionError> {
    let mut result = FoundRegistrations::default();
    let module = parsed.ast();

    for stmt in &module.body {
        if let Stmt::FunctionDef(func_def) = stmt {
            let func_name = func_def.name.as_str();
            for decorator in &func_def.decorator_list {
                if let Some(info) = analyze_tag_decorator(decorator, func_name) {
                    result.tags.push(info);
                }
                if let Some(info) = analyze_filter_decorator(decorator, func_name) {
                    result.filters.push(info);
                }
            }
        }
    }

    Ok(result)
}

fn analyze_tag_decorator(
    decorator: &Decorator,
    func_name: &str,
) -> Option<RegistrationInfo> {
    let expr = &decorator.expression;

    match expr {
        // Bare decorator: `@register.tag`
        Expr::Attribute(attr) => {
            let method_name = attr.attr.as_str();
            let kind = decorator_kind_from_name(method_name)?;

            if !is_register_attribute(attr) {
                return None;
            }

            Some(RegistrationInfo {
                name: func_name.to_string(),
                decorator_kind: kind,
                function_name: func_name.to_string(),
                offset: decorator.range().start().to_usize(),
                explicit_end_name: None,
            })
        }

        // Call decorator: `@register.tag("name")` or
        // `@register.simple_block_tag(end_name="...")`
        Expr::Call(call) => {
            // Check for helper/wrapper decorators first:
            // `@register_simple_block_tag(...)`
            if let Expr::Name(name) = call.func.as_ref() {
                if TAG_HELPER_DECORATORS.contains(&name.id.as_str()) {
                    let helper_kind = DecoratorKind::HelperWrapper(
                        name.id.to_string(),
                    );
                    let tag_name =
                        extract_name_from_call(call, &helper_kind)
                            .unwrap_or_else(|| func_name.to_string());
                    let explicit_end_name = extract_end_name_from_call(call);

                    return Some(RegistrationInfo {
                        name: tag_name,
                        decorator_kind: DecoratorKind::HelperWrapper(
                            name.id.to_string(),
                        ),
                        function_name: func_name.to_string(),
                        offset: decorator.range().start().to_usize(),
                        explicit_end_name,
                    });
                }
            }

            // Standard `register.method(...)` pattern
            let Expr::Attribute(attr) = call.func.as_ref() else {
                return None;
            };

            let method_name = attr.attr.as_str();
            let kind = decorator_kind_from_name(method_name)?;

            if !is_register_attribute(attr) {
                return None;
            }

            let name = extract_name_from_call(call, &kind)
                .unwrap_or_else(|| func_name.to_string());

            let explicit_end_name =
                if matches!(kind, DecoratorKind::SimpleBlockTag) {
                    extract_end_name_from_call(call)
                } else {
                    None
                };

            Some(RegistrationInfo {
                name,
                decorator_kind: kind,
                function_name: func_name.to_string(),
                offset: decorator.range().start().to_usize(),
                explicit_end_name,
            })
        }

        // Bare helper decorator: `@register_simple_block_tag`
        // (unlikely but handle it)
        Expr::Name(name) => {
            if TAG_HELPER_DECORATORS.contains(&name.id.as_str()) {
                return Some(RegistrationInfo {
                    name: func_name.to_string(),
                    decorator_kind: DecoratorKind::HelperWrapper(
                        name.id.to_string(),
                    ),
                    function_name: func_name.to_string(),
                    offset: decorator.range().start().to_usize(),
                    explicit_end_name: None,
                });
            }
            None
        }

        _ => None,
    }
}

fn analyze_filter_decorator(
    decorator: &Decorator,
    func_name: &str,
) -> Option<RegistrationInfo> {
    let is_filter = match &decorator.expression {
        Expr::Attribute(attr) => {
            FILTER_DECORATORS.contains(&attr.attr.as_str())
                && is_register_attribute(attr)
        }
        Expr::Call(call) => {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                FILTER_DECORATORS.contains(&attr.attr.as_str())
                    && is_register_attribute(attr)
            } else {
                false
            }
        }
        _ => false,
    };

    if !is_filter {
        return None;
    }

    let filter_kind = DecoratorKind::Tag;
    let name = if let Expr::Call(call) = &decorator.expression {
        extract_name_from_call(call, &filter_kind)
            .unwrap_or_else(|| func_name.to_string())
    } else {
        func_name.to_string()
    };

    Some(RegistrationInfo {
        name,
        decorator_kind: filter_kind,
        function_name: func_name.to_string(),
        offset: decorator.range().start().to_usize(),
        explicit_end_name: None,
    })
}

fn decorator_kind_from_name(name: &str) -> Option<DecoratorKind> {
    match name {
        "tag" => Some(DecoratorKind::Tag),
        "simple_tag" => Some(DecoratorKind::SimpleTag),
        "inclusion_tag" => Some(DecoratorKind::InclusionTag),
        "simple_block_tag" => Some(DecoratorKind::SimpleBlockTag),
        _ if TAG_DECORATORS.contains(&name) => {
            Some(DecoratorKind::Custom(name.to_string()))
        }
        _ => None,
    }
}

/// Check whether an attribute expression is accessing a recognized register
/// object.
///
/// Accepts `register`, `lib`, `library`, and any name ending in `register`
/// (e.g., `*register`).
fn is_register_attribute(attr: &ruff_python_ast::ExprAttribute) -> bool {
    match attr.value.as_ref() {
        Expr::Name(name) => {
            let n = name.id.as_str();
            n == "register"
                || n == "lib"
                || n == "library"
                || n.ends_with("register")
        }
        // If the value is not a simple name (e.g., `foo.bar.tag`), allow it
        // rather than silently skipping — the decorator method name is the
        // authoritative signal.
        _ => true,
    }
}

/// Extract the registered name from a decorator call's arguments.
///
/// For `@register.tag("name")` and `@register.filter("name")`, the first
/// positional string arg is the registered name. For `inclusion_tag`,
/// `simple_tag`, `simple_block_tag`, the first positional arg is NOT a
/// name (it's a template path or unused). All decorator types support
/// `name="custom"` as a keyword argument.
fn extract_name_from_call(
    call: &ruff_python_ast::ExprCall,
    kind: &DecoratorKind,
) -> Option<String> {
    // Only `@register.tag` and `@register.filter` (and helper wrappers)
    // use the first positional string as the name.
    let first_positional_is_name = matches!(
        kind,
        DecoratorKind::Tag | DecoratorKind::HelperWrapper(_)
    );

    if first_positional_is_name {
        if let Some(Expr::StringLiteral(s)) = call.arguments.args.first()
        {
            return Some(s.value.to_string());
        }
    }

    for kw in &call.arguments.keywords {
        if let Some(arg) = &kw.arg {
            if arg.as_str() == "name" {
                if let Expr::StringLiteral(s) = &kw.value {
                    return Some(s.value.to_string());
                }
            }
        }
    }

    None
}

/// Extract `end_name` keyword argument from a decorator call.
///
/// Handles: `@register.simple_block_tag(end_name="endmytag")`
fn extract_end_name_from_call(
    call: &ruff_python_ast::ExprCall,
) -> Option<String> {
    for kw in &call.arguments.keywords {
        if let Some(arg) = &kw.arg {
            if arg.as_str() == "end_name" {
                if let Expr::StringLiteral(s) = &kw.value {
                    return Some(s.value.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::needless_raw_string_hashes)]
mod tests {
    use super::*;
    use crate::parser::parse_module;

    #[test]
    fn bare_decorator() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "my_tag");
        assert_eq!(regs.tags[0].function_name, "my_tag");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::Tag
        ));
    }

    #[test]
    fn decorator_with_name() {
        let source = r#"
@register.tag("custom_name")
def my_func(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "custom_name");
        assert_eq!(regs.tags[0].function_name, "my_func");
    }

    #[test]
    fn decorator_with_keyword_name() {
        let source = r#"
@register.tag(name="keyword_name")
def my_func(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "keyword_name");
        assert_eq!(regs.tags[0].function_name, "my_func");
    }

    #[test]
    fn simple_block_tag_kind() {
        let source = r#"
@register.simple_block_tag
def myblock(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "myblock");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::SimpleBlockTag
        ));
        assert!(regs.tags[0].explicit_end_name.is_none());
    }

    #[test]
    fn simple_block_tag_with_end_name() {
        let source = r#"
@register.simple_block_tag(end_name="endcustom")
def custom(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "custom");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::SimpleBlockTag
        ));
        assert_eq!(
            regs.tags[0].explicit_end_name,
            Some("endcustom".to_string())
        );
    }

    #[test]
    fn helper_wrapper_decorator() {
        let source = r#"
@register_simple_block_tag(end_name="endhelper")
def helper(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "helper");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::HelperWrapper(ref name) if name == "register_simple_block_tag"
        ));
        assert_eq!(
            regs.tags[0].explicit_end_name,
            Some("endhelper".to_string())
        );
    }

    #[test]
    fn filter_bare_decorator() {
        let source = r#"
@register.filter
def my_filter(value):
    return value
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.filters.len(), 1);
        assert_eq!(regs.filters[0].name, "my_filter");
        assert_eq!(regs.filters[0].function_name, "my_filter");
    }

    #[test]
    fn filter_with_name() {
        let source = r#"
@register.filter("custom_filter")
def my_func(value):
    return value
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.filters.len(), 1);
        assert_eq!(regs.filters[0].name, "custom_filter");
        assert_eq!(regs.filters[0].function_name, "my_func");
    }

    #[test]
    fn simple_tag_decorator() {
        let source = r#"
@register.simple_tag
def current_time(format_string):
    return datetime.now().strftime(format_string)
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "current_time");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::SimpleTag
        ));
    }

    #[test]
    fn inclusion_tag_decorator() {
        let source = r#"
@register.inclusion_tag("results.html")
def show_results(poll):
    return {"choices": poll.choice_set.all()}
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "show_results");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::InclusionTag
        ));
    }

    #[test]
    fn alternative_register_names() {
        let source = r#"
@lib.tag
def tag_a(parser, token):
    pass

@library.simple_tag
def tag_b():
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 2);
        assert_eq!(regs.tags[0].name, "tag_a");
        assert_eq!(regs.tags[1].name, "tag_b");
    }

    #[test]
    fn no_registrations_in_plain_functions() {
        let source = r#"
def helper(x):
    return x + 1

def another():
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert!(regs.tags.is_empty());
        assert!(regs.filters.is_empty());
    }

    #[test]
    fn mixed_tags_and_filters() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    pass

@register.filter
def my_filter(value):
    return value

@register.simple_tag
def simple(arg):
    return arg
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 2);
        assert_eq!(regs.filters.len(), 1);
        assert_eq!(regs.tags[0].name, "my_tag");
        assert_eq!(regs.tags[1].name, "simple");
        assert_eq!(regs.filters[0].name, "my_filter");
    }

    #[test]
    fn unrecognized_decorator_method_ignored() {
        let source = r#"
@register.unknown_method
def something(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert!(regs.tags.is_empty());
        assert!(regs.filters.is_empty());
    }

    #[test]
    fn bare_helper_decorator() {
        let source = r#"
@register_simple_block_tag
def helper(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "helper");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::HelperWrapper(ref name)
                if name == "register_simple_block_tag"
        ));
        assert!(regs.tags[0].explicit_end_name.is_none());
    }

    #[test]
    fn helper_wrapper_with_name() {
        let source = r#"
@register_simple_block_tag("custom_name", end_name="endcustom")
def my_func(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "custom_name");
        assert_eq!(regs.tags[0].function_name, "my_func");
        assert_eq!(
            regs.tags[0].explicit_end_name,
            Some("endcustom".to_string())
        );
    }
}
