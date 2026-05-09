mod next_token;
mod parse_calls;

use ruff_python_ast::statement_visitor::walk_stmt;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprFString;
use ruff_python_ast::ExprName;
use ruff_python_ast::FStringPart;
use ruff_python_ast::InterpolatedStringElement;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::StmtReturn;

use crate::ext::ExprExt;
use crate::types::BlockSpec;

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
pub(crate) fn extract_block_spec(func: &StmtFunctionDef) -> Option<BlockSpec> {
    let parser_var = func
        .parameters
        .args
        .first()
        .map(|p| p.parameter.name.to_string())?;

    let token_var = func
        .parameters
        .args
        .get(1)
        .map(|p| p.parameter.name.to_string())?;

    // Check for opaque block patterns first: parser.skip_past("endtag")
    if let Some(spec) = extract_opaque_block_spec(&func.body, &parser_var) {
        return Some(spec);
    }

    // Try parser.parse((...)) calls with control flow classification
    if let Some(spec) = parse_calls::detect(&func.body, &parser_var, &token_var) {
        return Some(spec);
    }

    // Try dynamic end-tag patterns: parser.parse((f"end{tag_name}",))
    if let Some(spec) = extract_dynamic_end_block_spec(&func.body, &parser_var) {
        return Some(spec);
    }

    // Try parser.next_token() loop patterns (e.g., blocktrans/blocktranslate)
    next_token::detect(&func.body, &parser_var, &token_var)
}

/// Detect dynamic end-tag patterns: `parser.parse((f"end{tag_name}",))`.
fn extract_dynamic_end_block_spec(body: &[Stmt], parser_var: &str) -> Option<BlockSpec> {
    let mut visitor = DynamicEndFinder::new(parser_var);
    visitor.visit_body(body);

    if !visitor.found {
        return None;
    }
    Some(BlockSpec {
        end_tag: None,
        intermediates: Vec::new(),
        opaque: false,
    })
}

struct DynamicEndFinder<'a> {
    parser_var: &'a str,
    found: bool,
}

impl<'a> DynamicEndFinder<'a> {
    fn new(parser_var: &'a str) -> Self {
        Self {
            parser_var,
            found: false,
        }
    }
}

impl StatementVisitor<'_> for DynamicEndFinder<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if self.found {
            return;
        }

        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.found = is_dynamic_end_parse_call(&expr_stmt.value, self.parser_var);
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                self.found = is_dynamic_end_parse_call(value, self.parser_var);
            }
            Stmt::Return(StmtReturn {
                value: Some(val), ..
            }) => {
                self.found = is_dynamic_end_parse_call(val, self.parser_var);
            }
            Stmt::If(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::Try(_)
            | Stmt::With(_)
            | Stmt::Match(_) => {
                walk_stmt(self, stmt);
            }
            _ => {}
        }
    }
}

/// Check if an expression is `parser.parse((f"end{...}",))`.
fn is_dynamic_end_parse_call(expr: &Expr, parser_var: &str) -> bool {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return false;
    };
    let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = func.as_ref()
    else {
        return false;
    };
    if attr.as_str() != "parse" {
        return false;
    }
    if !is_parser_receiver(obj, parser_var) {
        return false;
    }
    if arguments.args.is_empty() {
        return false;
    }

    let seq = &arguments.args[0];
    let elements = match seq {
        Expr::Tuple(t) => &t.elts,
        Expr::List(l) => &l.elts,
        _ => return false,
    };

    elements.iter().any(is_end_fstring)
}

/// Check if an expression is an f-string starting with "end".
fn is_end_fstring(expr: &Expr) -> bool {
    let Expr::FString(ExprFString { value, .. }) = expr else {
        return false;
    };

    for part in value {
        match part {
            FStringPart::FString(fstr) => {
                let Some(first) = fstr.elements.first() else {
                    continue;
                };

                let has_end_prefix = matches!(
                    first,
                    InterpolatedStringElement::Literal(lit) if lit.value.starts_with("end")
                );
                if !has_end_prefix {
                    continue;
                }

                let has_interpolation = fstr
                    .elements
                    .iter()
                    .any(|e| matches!(e, InterpolatedStringElement::Interpolation(_)));

                if has_interpolation {
                    return true;
                }
            }
            FStringPart::Literal(_) => {}
        }
    }

    false
}

/// Check for dynamic end-tag format strings: `"end%s" % bits[0]` or `f"end{bits[0]}"`.
pub(super) fn has_dynamic_end_tag_format(body: &[Stmt]) -> bool {
    let mut visitor = DynamicEndFormatFinder::default();
    visitor.visit_body(body);
    visitor.found
}

#[derive(Default)]
struct DynamicEndFormatFinder {
    found: bool,
}

impl StatementVisitor<'_> for DynamicEndFormatFinder {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if self.found {
            return;
        }

        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.found = is_end_format_expr(&expr_stmt.value);
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                self.found = is_end_format_expr(value);
            }
            Stmt::Return(StmtReturn {
                value: Some(val), ..
            }) => {
                self.found = is_end_format_expr(val);
            }
            Stmt::If(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::Try(_)
            | Stmt::With(_)
            | Stmt::Match(_) => {
                walk_stmt(self, stmt);
            }
            _ => {}
        }
    }
}

/// Check if an expression is `"end%s" % something` or similar end-tag format.
fn is_end_format_expr(expr: &Expr) -> bool {
    if let Expr::BinOp(ExprBinOp {
        left,
        op: Operator::Mod,
        ..
    }) = expr
    {
        if let Some(s) = left.string_literal() {
            if s.starts_with("end") && s.contains('%') {
                return true;
            }
        }
    }
    is_end_fstring(expr)
}

/// Detect opaque block patterns: `parser.skip_past("endtag")`.
fn extract_opaque_block_spec(body: &[Stmt], parser_var: &str) -> Option<BlockSpec> {
    let mut visitor = SkipPastVisitor::new(parser_var);
    visitor.visit_body(body);
    let skip_past_tokens = visitor.tokens;

    if skip_past_tokens.is_empty() {
        return None;
    }
    let end_tag = if skip_past_tokens.len() == 1 {
        Some(skip_past_tokens[0].clone())
    } else {
        None
    };
    Some(BlockSpec {
        end_tag,
        intermediates: Vec::new(),
        opaque: true,
    })
}

struct SkipPastVisitor<'a> {
    parser_var: &'a str,
    tokens: Vec<String>,
}

impl<'a> SkipPastVisitor<'a> {
    fn new(parser_var: &'a str) -> Self {
        Self {
            parser_var,
            tokens: Vec::new(),
        }
    }

    fn insert_token(&mut self, token: String) {
        if !self.tokens.contains(&token) {
            self.tokens.push(token);
        }
    }
}

impl StatementVisitor<'_> for SkipPastVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                if let Some(token) = extract_skip_past_token(&expr_stmt.value, self.parser_var) {
                    self.insert_token(token);
                }
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                if let Some(token) = extract_skip_past_token(value, self.parser_var) {
                    self.insert_token(token);
                }
            }
            // Stay within the current function scope.
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }
}

/// Check if an expression is `parser.skip_past("token")` and extract the token.
fn extract_skip_past_token(expr: &Expr, parser_var: &str) -> Option<String> {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return None;
    };
    let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = func.as_ref()
    else {
        return None;
    };
    if attr.as_str() != "skip_past" {
        return None;
    }
    if !is_parser_receiver(obj, parser_var) {
        return None;
    }
    if arguments.args.is_empty() {
        return None;
    }
    arguments.args[0].string_literal()
}

/// Check if an expression is the parser variable (or `self.parser`).
pub(super) fn is_parser_receiver(expr: &Expr, parser_var: &str) -> bool {
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
    let elements = match expr {
        Expr::Tuple(t) => &t.elts,
        Expr::List(l) => &l.elts,
        Expr::Set(s) => &s.elts,
        _ => return Vec::new(),
    };

    elements
        .iter()
        .filter_map(ExprExt::string_literal_first_word)
        .collect()
}

/// Check if an expression accesses token contents.
///
/// Matches: `token.contents`, `token.contents.split()[0]`, `token.contents.strip()`
pub(super) fn is_token_contents_expr(expr: &Expr, token_var: Option<&str>) -> bool {
    match expr {
        Expr::Attribute(ExprAttribute { attr, value, .. }) => {
            if attr.as_str() == "contents" {
                if let Expr::Name(ExprName { id, .. }) = value.as_ref() {
                    if let Some(tv) = token_var {
                        return id.as_str() == tv;
                    }
                    return true;
                }
            }
            false
        }
        Expr::Call(ExprCall { func, .. }) => {
            if let Expr::Attribute(ExprAttribute { value, .. }) = func.as_ref() {
                return is_token_contents_expr(value, token_var);
            }
            false
        }
        Expr::Subscript(sub) => is_token_contents_expr(&sub.value, token_var),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::testing::django_function;

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
