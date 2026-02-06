//! Block structure extraction from `parser.parse()` calls.
//!
//! ## End-Tag Inference Strategy
//!
//! **NEVER GUESS.** We do NOT invent end tags from thin air. We only return
//! `end_tag: Some(...)` when the source code provides clear evidence.
//!
//! We apply three strategies in order, stopping at the first match:
//!
//! 1. **Singleton pattern** (HIGH CONFIDENCE): If `parser.parse((<single>,))`
//!    appears with exactly one unique stop tag, that tag is the closer.
//!
//! 2. **Unique stop tag** (HIGH CONFIDENCE): If only one stop tag is ever
//!    referenced across ALL `parser.parse()` calls, it's the closer.
//!
//! 3. **Django convention fallback** (CONSERVATIVE TIE-BREAKER FOR ALL BLOCK TAGS):
//!    If strategies 1 and 2 fail, AND there are no conflicting signals, check if
//!    `end{tag_name}` appears in the extracted stop-tag literals. This reflects
//!    Django's widespread convention (if→endif, for→endfor, block→endblock).
//!    **CRITICAL**: This is a tie-breaker, NOT a primary signal. We do NOT invent
//!    `end{tag_name}` — it must actually appear in the source's stop-tag literals.
//!
//! **Note**: `@register.simple_block_tag` has ADDITIONAL decorator-defined semantics:
//! Django hardcodes `end_name = f"end{function_name}"` as the default (`library.py:190`).
//! This is applied even without stop-tag analysis for that specific decorator.
//!
//! **ALL OTHER CASES → `end_tag: None`**:
//! - Multiple different singleton patterns (ambiguous)
//! - `end{tag_name}` not present in stop-tag literals
//! - Dynamic/computed stop tags (f-strings, variables)

use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::context::FunctionContext;
use crate::parser::ParsedModule;
use crate::patterns;
use crate::registry::RegistrationInfo;
use crate::types::BlockTagSpec;
use crate::types::DecoratorKind;
use crate::types::IntermediateTagSpec;
use crate::ExtractionError;

/// Extract block specification from a tag function.
///
/// ## Priority Order
///
/// 1. Explicit `end_name` from decorator (highest confidence)
/// 2. Singleton parse pattern (`parser.parse(("end...",))`)
/// 3. Unique stop tag (only one ever mentioned)
/// 4. Django convention fallback (`end{tag}` in stop-tags, tie-breaker)
/// 5. `simple_block_tag` decorator default (Django-defined semantic)
/// 6. None (ambiguous or no signals)
#[allow(clippy::unnecessary_wraps)]
pub fn extract_block_spec(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
    _ctx: &FunctionContext,
) -> Result<Option<BlockTagSpec>, ExtractionError> {
    if let Some(ref explicit_end) = registration.explicit_end_name {
        return Ok(Some(BlockTagSpec {
            end_tag: Some(explicit_end.clone()),
            intermediate_tags: vec![],
            opaque: false,
        }));
    }

    let django_default_end =
        if matches!(registration.decorator_kind, DecoratorKind::SimpleBlockTag) {
            Some(format!("end{}", registration.function_name))
        } else {
            None
        };

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
        if let Some(default_end) = django_default_end {
            return Ok(Some(BlockTagSpec {
                end_tag: Some(default_end),
                intermediate_tags: vec![],
                opaque: false,
            }));
        }
        return Ok(None);
    };

    let mut parse_calls: Vec<Vec<String>> = Vec::new();
    collect_parse_calls(&func_def.body, &mut parse_calls);

    if parse_calls.is_empty() {
        if let Some(default_end) = django_default_end {
            return Ok(Some(BlockTagSpec {
                end_tag: Some(default_end),
                intermediate_tags: vec![],
                opaque: false,
            }));
        }
        return Ok(None);
    }

    let end_tag =
        infer_end_tag_from_control_flow(&parse_calls, &registration.name);

    let mut all_stop_tags: Vec<String> = Vec::new();
    for stop_tags in &parse_calls {
        for tag in stop_tags {
            if !all_stop_tags.contains(tag) {
                all_stop_tags.push(tag.clone());
            }
        }
    }

    let intermediate_tags: Vec<IntermediateTagSpec> = all_stop_tags
        .into_iter()
        .filter(|t| end_tag.as_ref() != Some(t))
        .map(|name| IntermediateTagSpec {
            name,
            repeatable: false,
        })
        .collect();

    let opaque = intermediate_tags.is_empty()
        && end_tag.is_some()
        && !has_compile_filter_call(&func_def.body);

    if end_tag.is_none() && intermediate_tags.is_empty() {
        return Ok(None);
    }

    Ok(Some(BlockTagSpec {
        end_tag,
        intermediate_tags,
        opaque,
    }))
}

/// Infer the closer (end tag) from control flow patterns.
///
/// Returns `Some` ONLY when evidence exists in the source:
/// 1. Singleton pattern: exactly one unique `parser.parse((<single>,))` call
/// 2. Unique stop tag: only one stop tag mentioned across all parse calls
/// 3. Django convention: `end{tag_name}` appears in stop-tag literals
fn infer_end_tag_from_control_flow(
    parse_calls: &[Vec<String>],
    tag_name: &str,
) -> Option<String> {
    let mut all_tags: Vec<&str> = Vec::new();
    for stop_tags in parse_calls {
        for tag in stop_tags {
            if !all_tags.contains(&tag.as_str()) {
                all_tags.push(tag.as_str());
            }
        }
    }

    let mut singletons: Vec<&str> = Vec::new();
    for stop_tags in parse_calls {
        if stop_tags.len() == 1 {
            let tag = stop_tags[0].as_str();
            if !singletons.contains(&tag) {
                singletons.push(tag);
            }
        }
    }

    if singletons.len() == 1 {
        return Some(singletons[0].to_string());
    }

    if singletons.len() > 1 {
        return None;
    }

    if all_tags.len() == 1 {
        return Some(all_tags[0].to_string());
    }

    if singletons.is_empty() {
        let candidate = format!("end{tag_name}");
        if all_tags.contains(&candidate.as_str()) {
            return Some(candidate);
        }
    }

    None
}

fn collect_parse_calls(stmts: &[Stmt], results: &mut Vec<Vec<String>>) {
    for stmt in stmts {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if let Some(tags) = extract_parse_call_tags(&expr_stmt.value) {
                    results.push(tags);
                }
            }

            Stmt::Assign(assign) => {
                if let Some(tags) = extract_parse_call_tags(&assign.value) {
                    results.push(tags);
                }
            }

            Stmt::If(if_stmt) => {
                collect_parse_calls(&if_stmt.body, results);
                for clause in &if_stmt.elif_else_clauses {
                    collect_parse_calls(&clause.body, results);
                }
            }

            Stmt::While(while_stmt) => {
                collect_parse_calls(&while_stmt.body, results);
            }

            Stmt::For(for_stmt) => {
                collect_parse_calls(&for_stmt.body, results);
            }

            Stmt::Try(try_stmt) => {
                collect_parse_calls(&try_stmt.body, results);
            }

            _ => {}
        }
    }
}

fn extract_parse_call_tags(expr: &Expr) -> Option<Vec<String>> {
    let Expr::Call(call) = expr else { return None };
    let Expr::Attribute(attr) = call.func.as_ref() else {
        return None;
    };

    if attr.attr.as_str() != "parse" {
        return None;
    }

    let first_arg = call.arguments.args.first()?;

    if let Expr::Tuple(tuple) = first_arg {
        let mut tags = Vec::new();
        for elt in &tuple.elts {
            if let Some(s) = patterns::extract_string_literal(elt) {
                tags.push(s);
            }
        }
        if !tags.is_empty() {
            return Some(tags);
        }
    }

    if let Some(s) = patterns::extract_string_literal(first_arg) {
        return Some(vec![s]);
    }

    None
}

fn has_compile_filter_call(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if is_compile_filter_call(&expr_stmt.value) {
                    return true;
                }
            }
            Stmt::Assign(assign) => {
                if is_compile_filter_call(&assign.value) {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if has_compile_filter_call(&if_stmt.body) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if has_compile_filter_call(&clause.body) {
                        return true;
                    }
                }
            }
            Stmt::While(while_stmt) => {
                if has_compile_filter_call(&while_stmt.body) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn is_compile_filter_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Expr::Attribute(attr) = call.func.as_ref() else {
        return false;
    };
    attr.attr.as_str() == "compile_filter"
}

#[cfg(test)]
#[allow(clippy::needless_raw_string_hashes)]
mod tests {
    use crate::extract_rules;

    #[test]
    fn singleton_closer_pattern() {
        let source = r#"
@register.tag("if")
def do_if(parser, token):
    nodelist = parser.parse(("elif", "else", "endif"))
    token = parser.next_token()
    if token.contents == "else":
        nodelist = parser.parse(("endif",))
    return IfNode(nodelist)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("endif".to_string()));

        let names: Vec<_> = block_spec
            .intermediate_tags
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"elif"));
        assert!(names.contains(&"else"));
    }

    #[test]
    fn single_stop_tag() {
        let source = r#"
@register.tag
def my_block(parser, token):
    nodelist = parser.parse(("endmy_block",))
    return MyBlockNode(nodelist)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("endmy_block".to_string()));
        assert!(block_spec.intermediate_tags.is_empty());
    }

    #[test]
    fn non_conventional_closer() {
        let source = r#"
@register.tag
def mywidget(parser, token):
    nodelist = parser.parse(("else_widget", "finish_widget"))
    if token.contents == "else_widget":
        nodelist2 = parser.parse(("finish_widget",))
    return WidgetNode(nodelist, nodelist2)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("finish_widget".to_string()));
        assert_eq!(block_spec.intermediate_tags.len(), 1);
        assert_eq!(block_spec.intermediate_tags[0].name, "else_widget");
    }

    #[test]
    fn for_with_empty() {
        let source = r#"
@register.tag("for")
def do_for(parser, token):
    nodelist_loop = parser.parse(("empty", "endfor"))
    if token.contents == "empty":
        nodelist_empty = parser.parse(("endfor",))
    return ForNode(nodelist_loop, nodelist_empty)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("endfor".to_string()));
        assert_eq!(block_spec.intermediate_tags.len(), 1);
        assert_eq!(block_spec.intermediate_tags[0].name, "empty");
    }

    #[test]
    fn no_block_spec_for_simple_tag() {
        let source = r#"
@register.simple_tag
def now(format_string):
    return datetime.now().strftime(format_string)
"#;
        let result = extract_rules(source).unwrap();
        assert!(result.tags[0].block_spec.is_none());
    }

    #[test]
    fn simple_block_tag_with_explicit_end_name() {
        let source = r#"
@register.simple_block_tag(end_name="endmycustom")
def mycustom(context, nodelist):
    return nodelist.render(context)
"#;
        let result = extract_rules(source).unwrap();
        let tag = &result.tags[0];

        assert!(matches!(
            tag.decorator_kind,
            crate::DecoratorKind::SimpleBlockTag
        ));

        let block_spec = tag.block_spec.as_ref().unwrap();
        assert_eq!(block_spec.end_tag, Some("endmycustom".to_string()));
        assert!(block_spec.intermediate_tags.is_empty());
    }

    #[test]
    fn simple_block_tag_without_end_name_uses_django_default() {
        let source = r#"
@register.simple_block_tag
def myblock(context, nodelist):
    return nodelist.render(context)
"#;
        let result = extract_rules(source).unwrap();
        let tag = &result.tags[0];

        assert!(matches!(
            tag.decorator_kind,
            crate::DecoratorKind::SimpleBlockTag
        ));

        let block_spec = tag.block_spec.as_ref().unwrap();
        assert_eq!(block_spec.end_tag, Some("endmyblock".to_string()));
    }

    #[test]
    fn simple_block_tag_with_custom_name() {
        let source = r#"
@register.simple_block_tag(name="customname", end_name="endcustom")
def my_internal_func(context, nodelist):
    return nodelist.render(context)
"#;
        let result = extract_rules(source).unwrap();
        let tag = &result.tags[0];

        assert_eq!(tag.name, "customname");
        assert_eq!(
            tag.decorator_kind,
            crate::DecoratorKind::SimpleBlockTag
        );

        let block_spec = tag.block_spec.as_ref().unwrap();
        assert_eq!(block_spec.end_tag, Some("endcustom".to_string()));
    }

    #[test]
    fn helper_wrapper_decorator() {
        let source = r#"
@register_simple_block_tag(end_name="endhelper")
def helper_tag(context, nodelist):
    return nodelist.render(context)
"#;
        let result = extract_rules(source).unwrap();
        let tag = &result.tags[0];

        assert!(matches!(
            tag.decorator_kind,
            crate::DecoratorKind::HelperWrapper(ref name)
                if name == "register_simple_block_tag"
        ));

        let block_spec = tag.block_spec.as_ref().unwrap();
        assert_eq!(block_spec.end_tag, Some("endhelper".to_string()));
    }

    #[test]
    fn non_end_prefix_closer() {
        let source = r#"
@register.tag
def custom_block(parser, token):
    nodelist = parser.parse(("middle_custom", "finish_custom"))
    if token.contents == "middle_custom":
        nodelist2 = parser.parse(("finish_custom",))
    return CustomNode(nodelist, nodelist2)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("finish_custom".to_string()));
        assert_eq!(block_spec.intermediate_tags.len(), 1);
        assert_eq!(block_spec.intermediate_tags[0].name, "middle_custom");
    }

    #[test]
    fn ambiguous_returns_none() {
        let source = r#"
@register.tag
def confusing_block(parser, token):
    nodelist = parser.parse(("tag_a",))
    nodelist2 = parser.parse(("tag_b",))
    return Node(nodelist, nodelist2)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, None);
    }

    #[test]
    fn django_convention_fallback() {
        let source = r#"
@register.tag
def foo(parser, token):
    nodelist = parser.parse(("middle", "endfoo"))
    if some_condition:
        handle_middle()
    return FooNode(nodelist)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("endfoo".to_string()));
        assert_eq!(block_spec.intermediate_tags.len(), 1);
        assert_eq!(block_spec.intermediate_tags[0].name, "middle");
    }

    #[test]
    fn django_convention_not_invented() {
        let source = r#"
@register.tag
def bar(parser, token):
    nodelist = parser.parse(("stop_a", "stop_b"))
    return BarNode(nodelist)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, None);
    }

    #[test]
    fn django_convention_blocked_by_singleton_ambiguity() {
        let source = r#"
@register.tag
def foo(parser, token):
    nodelist = parser.parse(("other",))
    nodelist2 = parser.parse(("another",))
    nodelist3 = parser.parse(("middle", "endfoo"))
    return FooNode(nodelist, nodelist2, nodelist3)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, None);
    }

    #[test]
    fn opaque_block_detected() {
        let source = r#"
@register.tag
def verbatim(parser, token):
    nodelist = parser.parse(("endverbatim",))
    parser.delete_first_token()
    return VerbatimNode(nodelist)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("endverbatim".to_string()));
        assert!(block_spec.opaque);
    }

    #[test]
    fn non_opaque_block_with_compile_filter() {
        let source = r#"
@register.tag
def widthratio(parser, token):
    nodelist = parser.parse(("endwidthratio",))
    expr = parser.compile_filter(bits[1])
    return WidthRatioNode(nodelist, expr)
"#;
        let result = extract_rules(source).unwrap();
        let block_spec = result.tags[0].block_spec.as_ref().unwrap();

        assert_eq!(block_spec.end_tag, Some("endwidthratio".to_string()));
        assert!(!block_spec.opaque);
    }
}
