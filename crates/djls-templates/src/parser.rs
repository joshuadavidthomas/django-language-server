use std::iter::Peekable;

use compact_str::CompactString;
use djls_source::Span;
use serde::Serialize;
use thiserror::Error;

use crate::bits::TagBit;
use crate::filters::Filter;
use crate::filters::parse_filter;
use crate::filters::split_variable_expression;
use crate::lexer::Lexer;
use crate::nodelist::Node;
use crate::quotes::split_on_unquoted_whitespace;
use crate::tokens::TagDelimiter;
use crate::tokens::Token;

/// Parses tokens as the lexer produces them, without buffering a token list.
pub(crate) struct Parser<'src> {
    tokens: Peekable<Lexer<'src>>,
}

impl<'src> Parser<'src> {
    #[must_use]
    pub(crate) fn new(lexer: Lexer<'src>) -> Self {
        Self {
            tokens: lexer.peekable(),
        }
    }

    pub(crate) fn parse(&mut self) -> (Vec<Node>, Vec<ParseError>) {
        let mut nodelist = Vec::new();
        let mut errors = Vec::new();

        while let Some(token) = self.tokens.next() {
            let node = match &token {
                Token::Block { content, span } => Self::parse_block(content, *span),
                Token::Comment { content, span } => Ok(Node::Comment {
                    content: (*content).to_string(),
                    span: *span,
                }),
                Token::Error {
                    content, delimiter, ..
                } => Err(Self::parse_error(&token, content, *delimiter)),
                Token::Text { span, .. } => Ok(self.parse_text(*span)),
                Token::Variable { content, span } => Self::parse_variable(content, *span),
                Token::Eof => break,
            };

            match node {
                Ok(node) => nodelist.push(node),
                Err(error) => {
                    let (span, full_span) = token.spans();
                    errors.push(error.clone());
                    nodelist.push(Node::Error {
                        span,
                        full_span,
                        error,
                    });
                }
            }
        }

        (nodelist, errors)
    }

    // The lexer emits one Text token per run, plus a lone `{` before an
    // overlapping opener; fold adjacent ones into a single Text node.
    fn parse_text(&mut self, first: Span) -> Node {
        let mut end = first.end();
        while let Some(Token::Text { span, .. }) = self
            .tokens
            .next_if(|token| matches!(token, Token::Text { .. }))
        {
            end = span.end();
        }
        Node::Text {
            span: Span::saturating_from_bounds_usize(first.start_usize(), end as usize),
        }
    }

    fn parse_block(content: &str, span: Span) -> Result<Node, ParseError> {
        let (name, name_span, bits) = Self::parse_tag_args(content, span.start_usize())?;

        Ok(Node::Tag {
            name,
            name_span,
            bits,
            span,
        })
    }

    fn parse_tag_args(
        content: &str,
        position: usize,
    ) -> Result<(CompactString, Span, Vec<TagBit>), ParseError> {
        let mut iter = split_on_unquoted_whitespace(content);
        let name = iter.next().ok_or(ParseError::EmptyTag { position })?;
        let name_span =
            Span::saturating_from_parts_usize(position + name.start_byte, name.text.len());
        let bits = iter
            .map(|segment| {
                TagBit::new(
                    segment.text,
                    Span::saturating_from_parts_usize(
                        position + segment.start_byte,
                        segment.text.len(),
                    ),
                )
            })
            .collect();

        Ok((name.text.into(), name_span, bits))
    }

    fn parse_error(token: &Token<&str>, content: &str, delimiter: TagDelimiter) -> ParseError {
        let (_, full_span) = token.spans();
        ParseError::MalformedConstruct {
            position: full_span.start_usize(),
            delimiter,
            content: truncate_content(content),
        }
    }

    fn parse_variable(content: &str, span: Span) -> Result<Node, ParseError> {
        let base_offset = span.start();

        let mut parts = split_variable_expression(content);

        let (var_raw, var_offset) = parts.next().ok_or(ParseError::EmptyTag {
            position: span.start_usize(),
        })?;
        let var_trimmed_start = var_raw.len() - var_raw.trim_start().len();
        let var = CompactString::from(var_raw.trim());
        let var_span = Span::saturating_from_parts_usize(
            base_offset as usize + var_offset as usize + var_trimmed_start,
            var.len(),
        );

        let mut filters: Vec<Filter> = Vec::new();
        for (raw, offset_in_content) in parts {
            match parse_filter(raw, base_offset + offset_in_content) {
                Ok(filter) => filters.push(filter),
                Err(error) => {
                    return Err(ParseError::MalformedFilterExpression {
                        position: error.position as usize,
                        content: error.content,
                    });
                }
            }
        }

        Ok(Node::Variable {
            var,
            var_span,
            filters,
            span,
        })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum ParseError {
    #[error("Empty tag at position {position}")]
    EmptyTag { position: usize },

    #[error(
        "Unclosed '{}' (no matching '{}' found): {content}",
        .delimiter.opener(),
        .delimiter.closer()
    )]
    MalformedConstruct {
        position: usize,
        delimiter: TagDelimiter,
        content: String,
    },

    #[error("Malformed filter expression at position {position}: {content}")]
    MalformedFilterExpression { position: usize, content: String },
}

impl ParseError {
    #[must_use]
    pub fn diagnostic_code(&self) -> &'static str {
        "T100"
    }

    #[must_use]
    pub fn primary_span(&self) -> Option<(u32, u32)> {
        let (position, length) = match self {
            ParseError::MalformedConstruct {
                position,
                delimiter,
                ..
            } => (*position, delimiter.opener().len()),
            ParseError::EmptyTag { position }
            | ParseError::MalformedFilterExpression { position, .. } => (*position, 1),
        };

        Some((position.try_into().ok()?, length.try_into().ok()?))
    }
}

const MAX_ERROR_CONTENT_LEN: usize = 60;

fn truncate_content(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.len() <= MAX_ERROR_CONTENT_LEN {
        return trimmed.to_string();
    }
    let truncated = &trimmed[..trimmed.floor_char_boundary(MAX_ERROR_CONTENT_LEN)];
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::lexer::TagLines;

    fn parse_test_template(source: &str) -> Vec<Node> {
        crate::parse_template_impl(source, TagLines::Multi).0
    }

    #[test]
    fn parser_errors_keep_legacy_diagnostic_code() {
        let error = ParseError::MalformedConstruct {
            position: 15,
            delimiter: TagDelimiter::Variable,
            content: "value".to_string(),
        };

        assert_eq!(error.diagnostic_code(), "T100");
    }

    #[test]
    fn parser_errors_keep_primary_spans() {
        assert_eq!(
            ParseError::MalformedConstruct {
                position: 15,
                delimiter: TagDelimiter::Variable,
                content: "value".to_string(),
            }
            .primary_span(),
            Some((15, 2))
        );
        assert_eq!(
            ParseError::EmptyTag { position: 4 }.primary_span(),
            Some((4, 1))
        );
    }

    #[test]
    fn malformed_construct_span_survives_parse_pipeline() {
        let source = "Hello {{ value";
        let (_, errors) = crate::parse_template_impl(source, TagLines::Multi);

        assert_eq!(errors.len(), 1);
        let error = errors
            .into_iter()
            .next()
            .expect("malformed variable should produce one parse error");

        assert_eq!(error.diagnostic_code(), "T100");
        assert_eq!(error.primary_span(), Some((6, 2)));
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct TestNodeList {
        nodelist: Vec<TestNode>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct TestSpannedText {
        text: String,
        span: (u32, u32),
    }

    impl TestSpannedText {
        fn from_tag(bit: &crate::TagBit) -> Self {
            Self {
                text: bit.text.to_string(),
                span: (&bit.span).into(),
            }
        }

        fn from_filter(argument: &crate::FilterArgument) -> Self {
            Self {
                text: argument.text.to_string(),
                span: (&argument.span).into(),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct TestFilter {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        arg: Option<TestSpannedText>,
        span: (u32, u32),
    }

    impl TestFilter {
        fn from_filter(filter: &crate::filters::Filter) -> Self {
            Self {
                name: filter.name.clone(),
                arg: filter.arg.as_ref().map(TestSpannedText::from_filter),
                span: (&filter.span).into(),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    #[serde(tag = "type")]
    enum TestNode {
        Tag {
            name: String,
            name_span: (u32, u32),
            bits: Vec<TestSpannedText>,
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Comment {
            content: String,
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Text {
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Variable {
            var: String,
            filters: Vec<TestFilter>,
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Error {
            span: (u32, u32),
            full_span: (u32, u32),
            error: ParseError,
        },
    }

    impl TestNode {
        fn from_node(node: &Node) -> Self {
            match node {
                Node::Tag {
                    name,
                    name_span,
                    bits,
                    span,
                } => TestNode::Tag {
                    name: name.to_string(),
                    name_span: name_span.into(),
                    bits: bits.iter().map(TestSpannedText::from_tag).collect(),
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Comment { content, span } => TestNode::Comment {
                    content: content.clone(),
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Text { span } => TestNode::Text {
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Variable {
                    var,
                    var_span: _,
                    filters,
                    span,
                } => TestNode::Variable {
                    var: var.to_string(),
                    filters: filters.iter().map(TestFilter::from_filter).collect(),
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Error {
                    span,
                    full_span,
                    error,
                } => TestNode::Error {
                    span: span.into(),
                    full_span: full_span.into(),
                    error: error.clone(),
                },
            }
        }
    }

    fn convert_nodelist_for_testing(nodes: &[Node]) -> TestNodeList {
        TestNodeList {
            nodelist: nodes.iter().map(TestNode::from_node).collect(),
        }
    }

    mod html {
        use super::*;

        #[test]
        fn test_parse_html_doctype() {
            let source = "<!DOCTYPE html>";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_html_tag() {
            let source = "<div class=\"container\">Hello</div>";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_html_void() {
            let source = "<input type=\"text\" />";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod django {
        use super::*;

        #[test]
        fn test_parse_django_variable() {
            let source = "{{ user.name }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_variable_with_filter() {
            let source = "{{ user.name|title }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_filter_chains() {
            let source = "{{ value|default:'nothing'|title|upper }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_malformed_filter_expression_becomes_error_node() {
            let source = "{{ value| }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_if_block() {
            let source = "{% if user.is_authenticated %}Welcome{% endif %}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_for_block() {
            let source =
                "{% for item in items %}{{ item }}{% empty %}No items{% endfor %}".to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_complex_if_elif() {
            let source = "{% if x > 0 %}Positive{% elif x < 0 %}Negative{% else %}Zero{% endif %}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_tag_assignment() {
            let source = "{% url 'view-name' as view %}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_nested_for_if() {
            let source =
                "{% for item in items %}{% if item.active %}{{ item.name }}{% endif %}{% endfor %}"
                    .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_mixed_content() {
            let source = "Welcome, {% if user.is_authenticated %}
    {{ user.name|title|default:'Guest' }}
    {% for group in user.groups %}
        {% if forloop.first %}({% endif %}
        {{ group.name }}
        {% if not forloop.last %}, {% endif %}
        {% if forloop.last %}){% endif %}
    {% empty %}
        (no groups)
    {% endfor %}
{% else %}
    Guest
{% endif %}!";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod script {
        use super::*;

        #[test]
        fn test_parse_script() {
            let source = r#"<script type="text/javascript">
    // Single line comment
    const x = 1;
    /* Multi-line
        comment */
    console.log(x);
</script>"#
                .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod style {
        use super::*;

        #[test]
        fn test_parse_style() {
            let source = r#"<style type="text/css">
    /* Header styles */
    .header {
        color: blue;
    }
</style>"#
                .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod comments {
        use super::*;

        #[test]
        fn test_parse_comments() {
            let source = "<!-- HTML comment -->{# Django comment #}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod whitespace {
        use super::*;

        #[test]
        fn test_parse_with_leading_whitespace() {
            let source = "     hello";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_with_leading_whitespace_newline() {
            let source = "\n     hello";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_with_trailing_whitespace() {
            let source = "hello     ";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_with_trailing_whitespace_newline() {
            let source = "hello     \n";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_parse_unclosed_html_tag() {
            let source = "<div>";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_django_if() {
            let source = "{% if user.is_authenticated %}Welcome";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_django_for() {
            let source = "{% for item in items %}{{ item.name }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_script() {
            let source = "<script>console.log('test');";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_style() {
            let source = "<style>body { color: blue; ";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_variable_token() {
            let source = "{{ user";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_error_recovery_keeps_following_text_and_errors() {
            let source = "{%%}\n{{ a\n{{ b\n";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod full_templates {
        use super::*;

        #[test]
        fn test_parse_full() {
            let source = r#"<!DOCTYPE html>
<html>
    <head>
        <style type="text/css">
            /* Style header */
            .header { color: blue; }
        </style>
        <script type="text/javascript">
            // Init app
            const app = {
                /* Config */
                debug: true
            };
        </script>
    </head>
    <body>
        <!-- Header section -->
        <div class="header" id="main" data-value="123" disabled>
            {% if user.is_authenticated %}
                {# Welcome message #}
                <h1>Welcome, {{ user.name|title|default:'Guest' }}!</h1>
                {% if user.is_staff %}
                    <span>Admin</span>
                {% else %}
                    <span>User</span>
                {% endif %}
            {% endif %}
        </div>
    </body>
</html>"#
                .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }
}
