use ruff_python_ast::Decorator;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::parser::ParsedModule;
use crate::types::DecoratorKind;
use crate::ExtractionError;

/// Official Django template.Library decorator methods for tags.
const TAG_DECORATORS: &[&str] = &["tag", "simple_tag", "inclusion_tag", "simple_block_tag"];

/// Official Django template.Library decorator methods for filters.
const FILTER_DECORATORS: &[&str] = &["filter"];

/// Helper/wrapper decorator functions (NOT `register.<method>` but recognized as registration signals).
/// These are typically library-specific wrappers that delegate to official decorators.
/// Example: `@register_simple_block_tag(...)` (pretix pattern)
const TAG_HELPER_DECORATORS: &[&str] = &["register_simple_block_tag"];

/// Information about a found registration decorator.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RegistrationInfo {
    /// Tag/filter name as registered (may differ from function name)
    pub name: String,
    /// Kind of registration decorator
    pub decorator_kind: DecoratorKind,
    /// Original function name in Python source
    pub function_name: String,
    /// Byte offset in source where decorator starts
    pub offset: usize,
    /// For `simple_block_tag`: explicitly provided `end_name` argument (if any).
    /// When present, this is the authoritative closer — no inference needed.
    /// When absent, Django defaults to `f"end{function_name}"` at runtime.
    pub explicit_end_name: Option<String>,
}

/// Registry of found tag and filter registrations.
#[derive(Debug, Clone, Default)]
pub struct FoundRegistrations {
    /// Found tag registrations
    pub tags: Vec<RegistrationInfo>,
    /// Found filter registrations
    pub filters: Vec<RegistrationInfo>,
}

/// Find all `@register.tag`, `@register.filter`, and related decorators in the AST.
pub fn find_registrations(parsed: &ParsedModule) -> Result<FoundRegistrations, ExtractionError> {
    let mut result = FoundRegistrations::default();

    let ast = parsed.ast();

    for stmt in &ast.body {
        if let Stmt::FunctionDef(func_def) = stmt {
            for decorator in &func_def.decorator_list {
                if let Some(info) = analyze_tag_decorator(decorator, &func_def.name) {
                    result.tags.push(info);
                }

                if let Some(info) = analyze_filter_decorator(decorator, &func_def.name) {
                    result.filters.push(info);
                }
            }
        }
    }

    Ok(result)
}

fn analyze_tag_decorator(decorator: &Decorator, func_name: &str) -> Option<RegistrationInfo> {
    let expr = &decorator.expression;

    match expr {
        // Bare decorator: @register.tag
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
                offset: decorator.range.start().to_usize(),
                explicit_end_name: None,
            })
        }

        // Call decorator: @register.tag("name") or @register.simple_block_tag(end_name="...")
        Expr::Call(call) => {
            // Check for helper/wrapper decorators first: @register_simple_block_tag(...)
            if let Expr::Name(name) = call.func.as_ref() {
                if TAG_HELPER_DECORATORS.contains(&name.id.as_str()) {
                    let tag_name = extract_name_from_call(call, false)
                        .unwrap_or_else(|| func_name.to_string());
                    let explicit_end_name = extract_end_name_from_call(call);

                    return Some(RegistrationInfo {
                        name: tag_name,
                        decorator_kind: DecoratorKind::HelperWrapper(name.id.to_string()),
                        function_name: func_name.to_string(),
                        offset: decorator.range.start().to_usize(),
                        explicit_end_name,
                    });
                }
            }

            // Standard register.method(...) pattern
            let Expr::Attribute(attr) = call.func.as_ref() else {
                return None;
            };

            let method_name = attr.attr.as_str();
            let kind = decorator_kind_from_name(method_name)?;

            if !is_register_attribute(attr) {
                return None;
            }

            // inclusion_tag's first positional arg is template filename, not tag name
            let allow_positional_name = method_name != "inclusion_tag";
            let name = extract_name_from_call(call, allow_positional_name)
                .unwrap_or_else(|| func_name.to_string());

            // For simple_block_tag, extract end_name if provided
            let explicit_end_name = if matches!(kind, DecoratorKind::SimpleBlockTag) {
                extract_end_name_from_call(call)
            } else {
                None
            };

            Some(RegistrationInfo {
                name,
                decorator_kind: kind,
                function_name: func_name.to_string(),
                offset: decorator.range.start().to_usize(),
                explicit_end_name,
            })
        }

        // Bare helper decorator: @register_simple_block_tag (unlikely but handle it)
        Expr::Name(name) => {
            if TAG_HELPER_DECORATORS.contains(&name.id.as_str()) {
                return Some(RegistrationInfo {
                    name: func_name.to_string(),
                    decorator_kind: DecoratorKind::HelperWrapper(name.id.to_string()),
                    function_name: func_name.to_string(),
                    offset: decorator.range.start().to_usize(),
                    explicit_end_name: None,
                });
            }
            None
        }

        _ => None,
    }
}

fn analyze_filter_decorator(decorator: &Decorator, func_name: &str) -> Option<RegistrationInfo> {
    let is_filter = match &decorator.expression {
        Expr::Attribute(attr) => {
            FILTER_DECORATORS.contains(&attr.attr.as_str()) && is_register_attribute(attr)
        }
        Expr::Call(call) => {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                FILTER_DECORATORS.contains(&attr.attr.as_str()) && is_register_attribute(attr)
            } else {
                false
            }
        }
        _ => false,
    };

    if !is_filter {
        return None;
    }

    let name = if let Expr::Call(call) = &decorator.expression {
        extract_name_from_call(call, true).unwrap_or_else(|| func_name.to_string())
    } else {
        func_name.to_string()
    };

    Some(RegistrationInfo {
        name,
        decorator_kind: DecoratorKind::Tag, // Filters don't need distinct kinds
        function_name: func_name.to_string(),
        offset: decorator.range.start().to_usize(),
        explicit_end_name: None,
    })
}

fn decorator_kind_from_name(name: &str) -> Option<DecoratorKind> {
    match name {
        "tag" => Some(DecoratorKind::Tag),
        "simple_tag" => Some(DecoratorKind::SimpleTag),
        "inclusion_tag" => Some(DecoratorKind::InclusionTag),
        "simple_block_tag" => Some(DecoratorKind::SimpleBlockTag),
        _ if TAG_DECORATORS.contains(&name) => Some(DecoratorKind::Custom(name.to_string())),
        _ => None,
    }
}

/// Extract `end_name` keyword argument from a decorator call.
///
/// Handles: `@register.simple_block_tag(end_name="endmytag")`
fn extract_end_name_from_call(call: &ruff_python_ast::ExprCall) -> Option<String> {
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

fn is_register_attribute(attr: &ruff_python_ast::ExprAttribute) -> bool {
    match attr.value.as_ref() {
        Expr::Name(name) => {
            let n = name.id.as_str();
            n == "register" || n == "lib" || n == "library" || n.ends_with("register")
        }
        _ => true,
    }
}

fn extract_name_from_call(
    call: &ruff_python_ast::ExprCall,
    allow_positional: bool,
) -> Option<String> {
    // Only check positional args if allowed for this decorator type
    if allow_positional {
        if let Some(Expr::StringLiteral(s)) = call.arguments.args.first() {
            return Some(s.value.to_string());
        }
    }

    // Always check for explicit `name=` keyword argument
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_module;

    #[test]
    fn test_find_bare_decorator() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "my_tag");
        assert!(matches!(regs.tags[0].decorator_kind, DecoratorKind::Tag));
    }

    #[test]
    fn test_find_decorator_with_name() {
        let source = r#"
@register.tag("custom_name")
def my_func(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags[0].name, "custom_name");
        assert_eq!(regs.tags[0].function_name, "my_func");
    }

    #[test]
    fn test_simple_tag_decorator() {
        let source = r#"
@register.simple_tag
def my_simple_tag(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "my_simple_tag");
        assert!(matches!(regs.tags[0].decorator_kind, DecoratorKind::SimpleTag));
    }

    #[test]
    fn test_inclusion_tag_decorator() {
        let source = r#"
@register.inclusion_tag("template.html")
def my_inclusion_tag(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "my_inclusion_tag");
        assert!(matches!(
            regs.tags[0].decorator_kind,
            DecoratorKind::InclusionTag
        ));
    }

    #[test]
    fn test_simple_block_tag_decorator_kind() {
        let source = r#"
@register.simple_block_tag
def myblock(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "myblock");
        // Must be SimpleBlockTag, NOT Tag
        assert!(
            matches!(regs.tags[0].decorator_kind, DecoratorKind::SimpleBlockTag)
        );
        assert!(regs.tags[0].explicit_end_name.is_none());
    }

    #[test]
    fn test_simple_block_tag_with_end_name() {
        let source = r#"
@register.simple_block_tag(end_name="endcustom")
def custom(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags[0].name, "custom");
        assert!(
            matches!(regs.tags[0].decorator_kind, DecoratorKind::SimpleBlockTag)
        );
        assert_eq!(
            regs.tags[0].explicit_end_name,
            Some("endcustom".to_string())
        );
    }

    #[test]
    fn test_simple_block_tag_with_name_and_end_name() {
        let source = r#"
@register.simple_block_tag(name="myblock", end_name="endmyblock")
def custom(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags[0].name, "myblock");
        assert_eq!(
            regs.tags[0].explicit_end_name,
            Some("endmyblock".to_string())
        );
    }

    #[test]
    fn test_helper_wrapper_decorator_recognized() {
        let source = r#"
@register_simple_block_tag(end_name="endhelper")
def helper(context, nodelist):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "helper");
        assert!(
            matches!(
                &regs.tags[0].decorator_kind,
                DecoratorKind::HelperWrapper(name) if name == "register_simple_block_tag"
            ),
            "Expected HelperWrapper decorator kind, got {:?}",
            regs.tags[0].decorator_kind
        );
        assert_eq!(
            regs.tags[0].explicit_end_name,
            Some("endhelper".to_string())
        );
    }

    #[test]
    fn test_filter_decorator_bare() {
        let source = r#"
@register.filter
def my_filter(value):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.filters.len(), 1);
        assert_eq!(regs.filters[0].name, "my_filter");
    }

    #[test]
    fn test_filter_decorator_with_name() {
        let source = r#"
@register.filter("custom_filter")
def my_func(value):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.filters.len(), 1);
        assert_eq!(regs.filters[0].name, "custom_filter");
        assert_eq!(regs.filters[0].function_name, "my_func");
    }

    #[test]
    fn test_filter_decorator_with_name_keyword() {
        let source = r#"
@register.filter(name="another_filter")
def my_func(value):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.filters.len(), 1);
        assert_eq!(regs.filters[0].name, "another_filter");
    }

    #[test]
    fn test_lib_alias_recognized() {
        let source = r#"
@lib.tag
def my_tag(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "my_tag");
    }

    #[test]
    fn test_library_alias_recognized() {
        let source = r#"
@library.tag
def my_tag(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.tags[0].name, "my_tag");
    }

    #[test]
    fn test_multiple_decorators() {
        let source = r#"
@register.tag
@register.filter
def multi_use(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 1);
        assert_eq!(regs.filters.len(), 1);
        assert_eq!(regs.tags[0].name, "multi_use");
        assert_eq!(regs.filters[0].name, "multi_use");
    }

    #[test]
    fn test_offset_tracking() {
        let source = r#"
@register.tag
def first(parser, token):
    pass

@register.tag
def second(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert_eq!(regs.tags.len(), 2);
        // First decorator should have a smaller offset than the second
        assert!(regs.tags[0].offset < regs.tags[1].offset);
    }

    #[test]
    fn test_non_register_decorator_ignored() {
        let source = r#"
@some_other_decorator
def my_func(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert!(regs.tags.is_empty());
        assert!(regs.filters.is_empty());
    }

    #[test]
    fn test_unrelated_call_decorator_ignored() {
        let source = r#"
@something.tag
def my_func(parser, token):
    pass
"#;
        let parsed = parse_module(source).unwrap();
        let regs = find_registrations(&parsed).unwrap();

        assert!(regs.tags.is_empty());
        assert!(regs.filters.is_empty());
    }
}
