mod dynamic_end;
mod next_token;
mod opaque;
mod parse_calls;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::StmtFunctionDef;

use crate::ast::ExprExt;
use crate::templates::tags::analysis::AbstractValue;
use crate::templates::tags::types::BodyAnalysisEvidence;
use crate::templates::tags::types::SplitPosition;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EndTagEvidence {
    Literal(String),
    SelfNamed,
    Unknown,
}

impl EndTagEvidence {
    #[cfg(test)]
    fn as_literal(&self) -> Option<&str> {
        match self {
            Self::Literal(end_tag) => Some(end_tag.as_str()),
            Self::SelfNamed | Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtractedBlockSpec {
    pub(crate) end_tag: EndTagEvidence,
    pub(crate) intermediates: Vec<String>,
    pub(crate) body_analysis_evidence: BodyAnalysisEvidence,
}

pub(super) fn is_tag_name_value(value: &AbstractValue) -> bool {
    matches!(
        value,
        AbstractValue::SplitElement {
            index: SplitPosition::Forward(0)
        }
    )
}

/// Extract a block spec from a tag's compile function.
///
/// Finds calls to `parser.parse((...))` with tuple arguments containing
/// stop-token strings. Determines end-tag vs intermediate from control flow:
/// - If a stop-token leads to another `parser.parse()` call → intermediate
/// - If a stop-token leads to return/node construction → terminal (end-tag)
///
/// Also records body-consumption evidence from `parser.skip_past(...)` patterns.
///
/// Returns `None` when no block structure or body-consumption evidence is detected.
#[must_use]
pub(crate) fn extract_block_spec(func: &StmtFunctionDef) -> Option<ExtractedBlockSpec> {
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

    let skip_past = opaque::detect(&func.body, &parser_var);
    let parse_detected = parse_calls::is_detected(&func.body, &parser_var);
    let parse_spec = parse_calls::detect(&func.body, &parser_var, &token_var)
        .or_else(|| dynamic_end::detect(&func.body, &parser_var, &token_var));
    let body_analysis_evidence = match (skip_past.is_some(), parse_detected) {
        (false, _) => BodyAnalysisEvidence::NotDetected,
        (true, false) => BodyAnalysisEvidence::SkipPast,
        (true, true) => BodyAnalysisEvidence::Mixed,
    };

    let mut spec = match (skip_past, parse_detected) {
        (Some(skip_spec), true) => combine_mixed_structure(&skip_spec, parse_spec),
        (Some(skip_spec), false) => skip_spec,
        (None, _) => {
            parse_spec.or_else(|| next_token::detect(&func.body, &parser_var, &token_var))?
        }
    };
    spec.body_analysis_evidence = body_analysis_evidence;
    Some(spec)
}

fn combine_mixed_structure(
    skip_spec: &ExtractedBlockSpec,
    parse_spec: Option<ExtractedBlockSpec>,
) -> ExtractedBlockSpec {
    let Some(parse_spec) = parse_spec else {
        return ExtractedBlockSpec {
            end_tag: EndTagEvidence::Unknown,
            intermediates: Vec::new(),
            body_analysis_evidence: BodyAnalysisEvidence::Mixed,
        };
    };

    let end_tag = match (&skip_spec.end_tag, &parse_spec.end_tag) {
        (EndTagEvidence::Literal(skip), EndTagEvidence::Literal(parse)) if skip == parse => {
            EndTagEvidence::Literal(skip.clone())
        }
        _ => EndTagEvidence::Unknown,
    };
    ExtractedBlockSpec {
        end_tag,
        intermediates: parse_spec.intermediates,
        body_analysis_evidence: BodyAnalysisEvidence::Mixed,
    }
}

/// Check if an expression is the parser variable (or `self.parser`).
pub(super) fn is_parser_receiver(expr: &Expr, parser_var: &str) -> bool {
    if expr.name_target() == Some(parser_var) {
        return true;
    }
    if let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = expr
        && attr.as_str() == "parser"
        && obj
            .name_target()
            .is_some_and(|id| id == parser_var || id == "self")
    {
        return true;
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
            .filter_map(|expr| expr.string_literal_first_word().map(str::to_string))
            .collect(),
        Expr::List(l) => l
            .elts
            .iter()
            .filter_map(|expr| expr.string_literal_first_word().map(str::to_string))
            .collect(),
        Expr::Set(s) => s
            .elts
            .iter()
            .filter_map(|expr| expr.string_literal_first_word().map(str::to_string))
            .collect(),
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Compare(_)
        | Expr::Call(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Attribute(_)
        | Expr::Subscript(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => Vec::new(),
    }
}

/// Check if an expression accesses token contents.
///
/// Matches: `token.contents`, `token.contents.split()[0]`, `token.contents.strip()`
pub(super) fn is_token_contents_expr(expr: &Expr, token_var: Option<&str>) -> bool {
    match expr {
        Expr::Attribute(ExprAttribute { attr, value, .. }) => {
            if attr.as_str() == "contents" {
                if let Some(tv) = token_var {
                    return value.name_target() == Some(tv);
                }
                return value.name_target().is_some();
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
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Compare(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::templates::tags::testing::django_function;

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
        let func = django_function("django/template/defaulttags.py", "verbatim")
            .expect("expected Django fixture function should exist");
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_literal(), Some("endverbatim"));
        assert!(spec.intermediates.is_empty());
        assert_eq!(
            spec.body_analysis_evidence,
            BodyAnalysisEvidence::NotDetected
        );
    }

    // Corpus: do_if in defaulttags.py — parse(("elif", "else", "endif")) with while/if branches
    #[test]
    fn if_else_intermediates() {
        let func = django_function("django/template/defaulttags.py", "do_if")
            .expect("expected Django fixture function should exist");
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_literal(), Some("endif"));
        assert!(spec.intermediates.contains(&"elif".to_string()));
        assert!(spec.intermediates.contains(&"else".to_string()));
        assert_eq!(
            spec.body_analysis_evidence,
            BodyAnalysisEvidence::NotDetected
        );
    }

    // Corpus: comment in defaulttags.py — skip_past("endcomment")
    #[test]
    fn opaque_block_skip_past() {
        let func = django_function("django/template/defaulttags.py", "comment")
            .expect("expected Django fixture function should exist");
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_literal(), Some("endcomment"));
        assert!(spec.intermediates.is_empty());
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::SkipPast);
    }

    #[test]
    fn mixed_parse_and_skip_past_keeps_mixed_body_evidence() {
        let source = r#"
def mixed(parser, token):
    if token.contents:
        parser.skip_past("endmixed")
    else:
        parser.parse(("endmixed",))
    return MixedNode()
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should retain mixed parser evidence");

        assert_eq!(spec.end_tag.as_literal(), Some("endmixed"));
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::Mixed);
    }

    #[test]
    fn mixed_parse_and_skip_past_with_conflicting_closers_is_unknown() {
        let source = r#"
def mixed(parser, token):
    if token.contents:
        parser.skip_past("endskipped")
    else:
        parser.parse(("endparsed",))
    return MixedNode()
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should retain mixed parser evidence");

        assert_eq!(spec.end_tag, EndTagEvidence::Unknown);
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::Mixed);
    }

    #[test]
    fn mixed_parse_and_skip_past_with_unknown_closer_is_unknown() {
        let source = r#"
def mixed(parser, token):
    if token.contents:
        parser.skip_past(token.contents)
    else:
        parser.parse(("endmixed",))
    return MixedNode()
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should retain mixed parser evidence");

        assert_eq!(spec.end_tag, EndTagEvidence::Unknown);
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::Mixed);
    }

    #[test]
    fn mixed_parse_and_skip_past_preserves_parse_intermediates() {
        let source = r#"
def mixed(parser, token):
    if token.contents:
        parser.skip_past("endmixed")
    else:
        body = parser.parse(("otherwise", "endmixed"))
        if parser.next_token().contents == "otherwise":
            alternate = parser.parse(("endmixed",))
    return MixedNode(body, alternate)
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should retain mixed parser evidence");

        assert_eq!(spec.end_tag.as_literal(), Some("endmixed"));
        assert_eq!(spec.intermediates, vec!["otherwise".to_string()]);
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::Mixed);
    }

    #[test]
    fn nested_and_annotated_parse_calls_are_mixed_evidence() {
        for parse_statement in [
            "body: NodeList = parser.parse((\"endmixed\",))",
            "return MixedNode(parser.parse((\"endmixed\",)))",
        ] {
            let source = format!(
                "def mixed(parser, token):\n    parser.skip_past(\"endmixed\")\n    {parse_statement}\n"
            );
            let func = parse_function(&source);
            let spec = extract_block_spec(&func).expect("should retain mixed parser evidence");

            assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::Mixed);
        }
    }

    #[test]
    fn parser_parse_default_arguments_are_mixed_evidence() {
        let source = r#"
def mixed(parser, token):
    parser.skip_past("endmixed")
    parser.parse()
    return MixedNode()
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should retain mixed parser evidence");

        assert_eq!(spec.end_tag, EndTagEvidence::Unknown);
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::Mixed);
    }

    #[test]
    fn parse_call_in_nested_function_is_not_mixed_evidence() {
        let source = r#"
def skipped(parser, token):
    parser.skip_past("endskipped")
    def helper():
        parser.parse(("endskipped",))
    return SkippedNode()
"#;
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should retain skip evidence");

        assert_eq!(spec.end_tag.as_literal(), Some("endskipped"));
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::SkipPast);
    }

    #[test]
    fn skip_past_without_arguments_is_not_evidence() {
        let source = r"
def invalid(parser, token):
    parser.skip_past()
    return InvalidNode()
";
        let func = parse_function(source);

        assert!(extract_block_spec(&func).is_none());
    }

    #[test]
    fn skip_past_with_unknown_closer_keeps_positive_body_evidence() {
        let source = r"
def raw(parser, token):
    parser.skip_past(token.contents)
    return RawNode()
";
        let func = parse_function(source);
        let spec = extract_block_spec(&func).expect("should retain skip evidence");

        assert_eq!(spec.end_tag, EndTagEvidence::Unknown);
        assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::SkipPast);
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
        assert_eq!(spec.end_tag.as_literal(), Some("done"));
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
        assert_eq!(spec.end_tag, EndTagEvidence::SelfNamed);
        assert!(spec.intermediates.is_empty());
        assert_eq!(
            spec.body_analysis_evidence,
            BodyAnalysisEvidence::NotDetected
        );
    }

    // Corpus: do_for in defaulttags.py — parse(("empty", "endfor")) then
    // conditional parse(("endfor",))
    #[test]
    fn multiple_parse_calls_classify_correctly() {
        let func = django_function("django/template/defaulttags.py", "do_for")
            .expect("expected Django fixture function should exist");
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_literal(), Some("endfor"));
        assert_eq!(spec.intermediates, vec!["empty".to_string()]);
        assert_eq!(
            spec.body_analysis_evidence,
            BodyAnalysisEvidence::NotDetected
        );
    }

    // Corpus: now in defaulttags.py — no parser.parse() or skip_past calls
    #[test]
    fn no_parse_calls_returns_none() {
        let func = django_function("django/template/defaulttags.py", "now")
            .expect("expected Django fixture function should exist");
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
        assert_eq!(spec.end_tag.as_literal(), Some("endblock"));
    }

    // Fabricated: a single parse() call with both "end*" and non-"end*"
    // tokens has no control-flow evidence. Convention alone is not evidence,
    // so ambiguous single-call multi-token shapes are conservatively skipped.
    #[test]
    fn convention_tiebreaker_single_call_multi_token() {
        let source = r#"
def do_if(parser, token):
    nodelist = parser.parse(("else", "endif"))
    return IfNode(nodelist)
"#;
        let func = parse_function(source);
        assert!(extract_block_spec(&func).is_none());
    }

    // Corpus: do_block in loader_tags.py — parse(("endblock",)) with next_token
    // for endblock validation
    #[test]
    fn simple_block_with_endblock_validation() {
        let func = django_function("django/template/loader_tags.py", "do_block")
            .expect("expected Django fixture function should exist");
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_literal(), Some("endblock"));
        assert!(spec.intermediates.is_empty());
        assert_eq!(
            spec.body_analysis_evidence,
            BodyAnalysisEvidence::NotDetected
        );
    }

    // Corpus: spaceless in defaulttags.py — parse(("endspaceless",)) +
    // delete_first_token
    #[test]
    fn sequential_parse_then_check() {
        let func = django_function("django/template/defaulttags.py", "spaceless")
            .expect("expected Django fixture function should exist");
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag.as_literal(), Some("endspaceless"));
        assert!(spec.intermediates.is_empty());
    }

    // Corpus: do_block_translate in i18n.py — next_token loop with dynamic
    // end-tag ("end%s" % bits[0]) and "plural" intermediate
    #[test]
    fn next_token_loop_blocktrans_pattern() {
        let func = django_function("django/templatetags/i18n.py", "do_block_translate")
            .expect("expected Django fixture function should exist");
        let spec = extract_block_spec(&func).expect("should extract block spec");
        assert_eq!(spec.end_tag, EndTagEvidence::SelfNamed);
        assert_eq!(spec.intermediates, vec!["plural".to_string()]);
        assert_eq!(
            spec.body_analysis_evidence,
            BodyAnalysisEvidence::NotDetected
        );
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
        assert_eq!(spec.end_tag.as_literal(), Some("endcustom"));
        assert!(spec.intermediates.is_empty());
        assert_eq!(
            spec.body_analysis_evidence,
            BodyAnalysisEvidence::NotDetected
        );
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
        assert_eq!(spec.end_tag.as_literal(), Some("endcustom"));
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
