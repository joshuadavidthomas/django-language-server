mod dynamic_end;
mod next_token;
mod opaque;
mod parse_calls;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::StmtFunctionDef;

use crate::ext::ExprExt;
use crate::types::BlockTagSpec;

/// Extract a block spec from a tag's compile function.
///
/// Finds calls to `parser.parse((...))` with tuple arguments containing
/// stop-token strings. Determines end-tag vs intermediate from control flow:
/// - If a stop-token leads to another `parser.parse()` call → intermediate
/// - If a stop-token leads to return/node construction → terminal (end-tag)
///
/// Also detects opaque blocks via `parser.skip_past(...)` patterns.
///
/// Returns `None` when no block structure is detected or inference is ambiguous.
#[must_use]
pub fn extract_block_spec(func: &StmtFunctionDef) -> Option<BlockTagSpec> {
    let parser_var = func
        .parameters
        .args
        .first()
        .map(|p| p.parameter.name.to_string())?;

    // Check for opaque block patterns first: parser.skip_past("endtag")
    if let Some(spec) = opaque::detect(&func.body, &parser_var) {
        return Some(spec);
    }

    // Try parser.parse((...)) calls with control flow classification
    if let Some(spec) = parse_calls::detect(&func.body, &parser_var) {
        return Some(spec);
    }

    // Try dynamic end-tag patterns: parser.parse((f"end{tag_name}",))
    if let Some(spec) = dynamic_end::detect(&func.body, &parser_var) {
        return Some(spec);
    }

    // Try parser.next_token() loop patterns (e.g., blocktrans/blocktranslate)
    next_token::detect(&func.body, &parser_var)
}

/// Check if an expression is the parser variable (or `self.parser`).
pub(crate) fn is_parser_receiver(expr: &Expr, parser_var: &str) -> bool {
    if let Expr::Name(ExprName { id, .. }) = expr {
        if id.as_str() == parser_var {
            return true;
        }
    }
    if let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = expr
    {
        if attr.as_str() == "parser" {
            if let Expr::Name(ExprName { id, .. }) = obj.as_ref() {
                if id.as_str() == parser_var || id.as_str() == "self" {
                    return true;
                }
            }
        }
    }
    false
}

/// Extract literal string constants from a tuple/list/set expression.
///
/// Handles:
/// - `("endif", "else", "elif")`
/// - `("endif",)`
///
/// Does not resolve variable references.
pub(super) fn extract_string_sequence(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::Tuple(t) => t
            .elts
            .iter()
            .filter_map(ExprExt::string_literal_first_word)
            .collect(),
        Expr::List(l) => l
            .elts
            .iter()
            .filter_map(ExprExt::string_literal_first_word)
            .collect(),
        Expr::Set(s) => s
            .elts
            .iter()
            .filter_map(ExprExt::string_literal_first_word)
            .collect(),
        _ => Vec::new(),
    }
}

/// Check if an expression accesses token contents.
///
/// Matches: `token.contents`, `token.contents.split()[0]`, `token.contents.strip()`
pub(crate) fn is_token_contents_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Attribute(ExprAttribute { attr, value, .. }) => {
            if attr.as_str() == "contents" {
                return matches!(value.as_ref(), Expr::Name(_));
            }
            false
        }
        Expr::Call(ExprCall { func, .. }) => {
            if let Expr::Attribute(ExprAttribute { value, .. }) = func.as_ref() {
                return is_token_contents_expr(value);
            }
            false
        }
        Expr::Subscript(sub) => is_token_contents_expr(&sub.value),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::test_helpers::django_function;

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

    // Corpus: verbatim in defaulttags.py — parse(("endverbatim",)) + delete_first_token
    #[test]
    fn simple_end_tag_single_parse() {
        let func = django_function("django/template/defaulttags.py", "verbatim").unwrap();
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endverbatim"));
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    // Corpus: do_if in defaulttags.py — parse(("elif", "else", "endif")) with while/if branches
    #[test]
    fn if_else_intermediates() {
        let func = django_function("django/template/defaulttags.py", "do_if").unwrap();
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endif"));
        assert!(spec.intermediates.contains(&"elif".to_string()));
        assert!(spec.intermediates.contains(&"else".to_string()));
        assert!(!spec.opaque);
    }

    // Corpus: comment in defaulttags.py — skip_past("endcomment")
    #[test]
    fn opaque_block_skip_past() {
        let func = django_function("django/template/defaulttags.py", "comment").unwrap();
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endcomment"));
        assert!(spec.intermediates.is_empty());
        assert!(spec.opaque);
    }

    // Fabricated: tests non-conventional closer ("done" instead of "end*").
    // No corpus function uses a non-"end*" closer with a single-token parse call.
    #[test]
    fn non_conventional_closer_found_via_control_flow() {
        let source = r#"
def do_repeat(parser, token):
    nodelist = parser.parse(("done",))
    parser.delete_first_token()
    return RepeatNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("done"));
        assert!(spec.intermediates.is_empty());
    }

    // Fabricated: tests ambiguous multi-token parse with no control flow clues.
    // No corpus function has this pattern — real code always has control flow
    // that disambiguates end-tag vs intermediate.
    #[test]
    fn ambiguous_returns_none_for_end_tag() {
        let source = r#"
def do_custom(parser, token):
    nodelist = parser.parse(("stop", "halt"))
    return CustomNode(nodelist)
"#;
        let func = parse_function(source);
        assert!(extract_block_spec(&func).is_none());
    }

    // Fabricated: tests f-string in parser.parse() producing dynamic (None) end-tag.
    // No corpus function puts an f-string directly in parser.parse() — real Django
    // uses "end%s" % bits[0] (percent formatting) in do_block_translate, or builds
    // the f-string into a variable first (partialdef_func). This tests the f-string
    // detection path specifically.
    #[test]
    fn dynamic_fstring_end_tag() {
        let source = r#"
def do_block(parser, token):
    tag_name, *rest = token.split_contents()
    nodelist = parser.parse((f"end{tag_name}",))
    parser.delete_first_token()
    return BlockNode(tag_name, nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert!(spec.end_tag.is_none());
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    // Corpus: do_for in defaulttags.py — parse(("empty", "endfor")) then
    // conditional parse(("endfor",))
    #[test]
    fn multiple_parse_calls_classify_correctly() {
        let func = django_function("django/template/defaulttags.py", "do_for").unwrap();
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endfor"));
        assert_eq!(spec.intermediates, vec!["empty".to_string()]);
        assert!(!spec.opaque);
    }

    // Corpus: now in defaulttags.py — no parser.parse() or skip_past calls
    #[test]
    fn no_parse_calls_returns_none() {
        let func = django_function("django/template/defaulttags.py", "now").unwrap();
        assert!(extract_block_spec(&func).is_none());
    }

    // Fabricated: tests classytags-style self.parser.parse() pattern.
    // No corpus function uses self.parser — this is a third-party pattern
    // (classytags, wagtail) not in standard Django.
    #[test]
    fn self_parser_pattern() {
        let source = r#"
def do_block(self, token):
    nodelist = self.parser.parse(("endblock",))
    self.parser.delete_first_token()
    return BlockNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endblock"));
    }

    // Fabricated: tests convention tie-breaker when a single parse() call has
    // both "end*" and non-"end*" tokens with no control flow. Real Django
    // functions always have multiple parse calls or control flow that the
    // classifier uses — this tests the fallback convention path.
    #[test]
    fn convention_tiebreaker_single_call_multi_token() {
        let source = r#"
def do_if(parser, token):
    nodelist = parser.parse(("else", "endif"))
    return IfNode(nodelist)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endif"));
        assert_eq!(spec.intermediates, vec!["else".to_string()]);
    }

    // Corpus: do_block in loader_tags.py — parse(("endblock",)) with next_token
    // for endblock validation
    #[test]
    fn simple_block_with_endblock_validation() {
        let func = django_function("django/template/loader_tags.py", "do_block").unwrap();
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endblock"));
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    // Corpus: spaceless in defaulttags.py — parse(("endspaceless",)) +
    // delete_first_token
    #[test]
    fn sequential_parse_then_check() {
        let func = django_function("django/template/defaulttags.py", "spaceless").unwrap();
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endspaceless"));
        assert!(spec.intermediates.is_empty());
    }

    // Corpus: do_block_translate in i18n.py — next_token loop with dynamic
    // end-tag ("end%s" % bits[0]) and "plural" intermediate
    #[test]
    fn next_token_loop_blocktrans_pattern() {
        let func = django_function("django/templatetags/i18n.py", "do_block_translate").unwrap();
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert!(spec.end_tag.is_none());
        assert_eq!(spec.intermediates, vec!["plural".to_string()]);
        assert!(!spec.opaque);
    }

    // Fabricated: next_token loop with a static end-tag comparison.
    // Real Django's do_block_translate uses a dynamic end-tag. This tests
    // the static end-tag detection path in next_token loops.
    #[test]
    fn next_token_loop_static_end_tag() {
        let source = r#"
def do_custom_block(parser, token):
    content = []
    while parser.tokens:
        token = parser.next_token()
        if token.token_type == TokenType.TEXT:
            content.append(token)
        else:
            break
    if token.contents.strip() != "endcustom":
        raise TemplateSyntaxError("error")
    return CustomBlockNode(content)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endcustom"));
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    // Fabricated: next_token loop with both an intermediate and a static end-tag.
    // Real Django's do_block_translate has a dynamic end-tag. This tests the
    // intermediate + static end-tag combination in next_token loops.
    #[test]
    fn next_token_loop_with_intermediate_and_static_end() {
        let source = r#"
def do_custom(parser, token):
    nodes = []
    while parser.tokens:
        token = parser.next_token()
        if token.token_type in (TokenType.VAR, TokenType.TEXT):
            nodes.append(token)
        else:
            break
    if token.contents.strip() == "middle":
        more_nodes = []
        while parser.tokens:
            token = parser.next_token()
            if token.token_type in (TokenType.VAR, TokenType.TEXT):
                more_nodes.append(token)
            else:
                break
    if token.contents.strip() != "endcustom":
        raise TemplateSyntaxError("error")
    return CustomNode(nodes)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_deref(), Some("endcustom"));
        assert_eq!(spec.intermediates, vec!["middle".to_string()]);
    }

    // Fabricated: function with parser param but no parse/skip_past/next_token calls.
    // Edge case — tests that a function with no block structure returns None.
    #[test]
    fn no_next_token_loop_no_parse_returns_none() {
        let source = r"
def do_simple(parser, token):
    bits = token.split_contents()
    return SimpleNode(bits[1])
";
        let func = parse_function(source);
        assert!(extract_block_spec(&func).is_none());
    }

    // Fabricated: function with no parameters at all returns None.
    // Edge case — tests the parameter check guard.
    #[test]
    fn no_parameters_returns_none() {
        let source = r"
def helper():
    pass
";
        let func = parse_function(source);
        assert!(extract_block_spec(&func).is_none());
    }
}
