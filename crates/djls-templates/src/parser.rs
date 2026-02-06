use djls_source::Span;
use serde::Serialize;
use thiserror::Error;

use crate::nodelist::Filter;
use crate::nodelist::FilterArg;
use crate::nodelist::Node;
use crate::tokens::Token;

pub struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    #[must_use]
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    pub fn parse(&mut self) -> (Vec<Node>, Vec<ParseError>) {
        let mut nodelist = Vec::with_capacity(self.tokens.len() / 2);
        let mut errors = Vec::with_capacity(4);

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    nodelist.push(node);
                }
                Err(error) => {
                    let (span, full_span) = self
                        .peek_previous()
                        .ok()
                        .or_else(|| self.peek().ok())
                        .map_or(
                            {
                                let empty = Span::new(0, 0);
                                (empty, empty)
                            },
                            super::tokens::Token::spans,
                        );

                    errors.push(error.clone());

                    nodelist.push(Node::Error {
                        span,
                        full_span,
                        error,
                    });

                    if !self.is_at_end() {
                        // Continue parsing even if synchronization fails
                        let _ = self.synchronize();
                    }
                }
            }
        }

        (nodelist, errors)
    }

    fn next_node(&mut self) -> Result<Node, ParseError> {
        let token = self.consume()?;

        match token {
            Token::Block { .. } => self.parse_block(),
            Token::Comment { .. } => self.parse_comment(),
            Token::Eof => Err(ParseError::stream_error(StreamError::AtEnd)),
            Token::Error { .. } => self.parse_error(),
            Token::Newline { .. } | Token::Text { .. } | Token::Whitespace { .. } => {
                self.parse_text()
            }
            Token::Variable { .. } => self.parse_variable(),
        }
    }

    pub fn parse_block(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        let Token::Block {
            content: content_ref,
            ..
        } = token
        else {
            return Err(ParseError::InvalidSyntax {
                context: "Expected Block token".to_string(),
            });
        };

        let (name, bits) = Self::parse_tag_args(content_ref)?;
        let span = token.content_span_or_fallback();

        Ok(Node::Tag { name, bits, span })
    }

    fn parse_tag_args(content: &str) -> Result<(String, Vec<String>), ParseError> {
        let mut pieces = Vec::with_capacity((content.len() / 8).clamp(2, 8));
        let mut start = None;
        let mut quote: Option<char> = None;
        let mut escape = false;
        for (idx, ch) in content.char_indices() {
            if start.is_none() && !ch.is_whitespace() {
                start = Some(idx);
            }
            if escape {
                escape = false;
                continue;
            }
            match ch {
                '\\' if quote.is_some() => escape = true,
                '"' | '\'' if quote == Some(ch) => quote = None,
                '"' | '\'' if quote.is_none() => quote = Some(ch),
                c if quote.is_none() && c.is_whitespace() => {
                    if let Some(s) = start.take() {
                        pieces.push(content[s..idx].to_owned());
                    }
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            pieces.push(content[s..].to_owned());
        }
        let mut iter = pieces.into_iter();
        let name = iter.next().ok_or(ParseError::EmptyTag)?;
        Ok((name, iter.collect()))
    }

    fn parse_comment(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        let span = token.content_span_or_fallback();
        Ok(Node::Comment {
            content: token.content(),
            span,
        })
    }

    fn parse_error(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        match token {
            Token::Error { content, span, .. } => {
                let error_text = content.clone();
                let full_span = token.full_span().unwrap_or(*span);
                Err(ParseError::MalformedConstruct {
                    position: full_span.start_usize(),
                    content: error_text,
                })
            }
            _ => Err(ParseError::InvalidSyntax {
                context: "Expected Error token".to_string(),
            }),
        }
    }

    fn parse_text(&mut self) -> Result<Node, ParseError> {
        let first_span = self.peek_previous()?.full_span_or_fallback();
        let start = first_span.start();
        let mut end = first_span.end();

        while let Ok(token) = self.peek() {
            match token {
                Token::Block { .. }
                | Token::Variable { .. }
                | Token::Comment { .. }
                | Token::Error { .. }
                | Token::Eof => break, // Stop at Django constructs, errors, or EOF
                Token::Text { .. } | Token::Whitespace { .. } | Token::Newline { .. } => {
                    // Update end position
                    let token_end = token.full_span_or_fallback().end();
                    end = end.max(token_end);
                    self.consume()?;
                }
            }
        }

        let length = end.saturating_sub(start);
        let span = Span::new(start, length);

        Ok(Node::Text { span })
    }

    fn parse_variable(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        let Token::Variable {
            content: content_ref,
            ..
        } = token
        else {
            return Err(ParseError::InvalidSyntax {
                context: "Expected Variable token".to_string(),
            });
        };

        let content_span = token.content_span_or_fallback();
        let content_start = content_span.start();

        // Parse variable and filters using quote-aware scanner
        let (var, filters) = parse_variable_expression(content_ref, content_start)?;

        Ok(Node::Variable {
            var,
            filters,
            span: content_span,
        })
    }

    #[inline]
    fn peek(&self) -> Result<&Token, ParseError> {
        self.tokens.get(self.current).ok_or_else(|| {
            if self.tokens.is_empty() {
                ParseError::stream_error(StreamError::Empty)
            } else {
                ParseError::stream_error(StreamError::AtEnd)
            }
        })
    }

    #[inline]
    fn peek_previous(&self) -> Result<&Token, ParseError> {
        if self.current == 0 {
            return Err(ParseError::stream_error(StreamError::BeforeStart));
        }
        self.tokens
            .get(self.current - 1)
            .ok_or_else(|| ParseError::stream_error(StreamError::InvalidAccess))
    }

    #[inline]
    fn is_at_end(&self) -> bool {
        self.current + 1 >= self.tokens.len()
    }

    #[inline]
    fn consume(&mut self) -> Result<&Token, ParseError> {
        if self.is_at_end() {
            return Err(ParseError::stream_error(StreamError::AtEnd));
        }
        self.current += 1;
        self.peek_previous()
    }

    fn synchronize(&mut self) -> Result<(), ParseError> {
        while !self.is_at_end() {
            let current = self.peek()?;
            match current {
                Token::Block { .. }
                | Token::Variable { .. }
                | Token::Comment { .. }
                | Token::Eof => {
                    return Ok(());
                }
                _ => {}
            }
            self.consume()?;
        }
        Ok(())
    }
}

/// Parse a variable expression like `value|default:'nothing'|title`.
///
/// Uses a single-pass quote-aware scanner to correctly handle:
/// - `{{ x|default:"a|b" }}` — pipe inside quotes is NOT a filter separator
/// - `{{ x|default:'value:with:colons' }}` — colons inside quotes are argument content
/// - `{{ x | filter1 | filter2 }}` — whitespace around pipes
/// - `{{x|filter}}` — no whitespace
///
/// Returns `(variable_name, filters)` with byte-accurate spans.
fn parse_variable_expression(
    content: &str,
    content_start: u32,
) -> Result<(String, Vec<Filter>), ParseError> {
    let scanner = VariableScanner::new(content, content_start);
    scanner.parse()
}

/// State-machine scanner for variable expressions with quote awareness.
///
/// Tracks quote state to distinguish `|` as filter separator vs literal character.
struct VariableScanner<'a> {
    content: &'a str,
    content_start: u32,
    bytes: &'a [u8],
    pos: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum QuoteState {
    None,
    Single, // Inside '...'
    Double, // Inside "..."
}

impl<'a> VariableScanner<'a> {
    fn new(content: &'a str, content_start: u32) -> Self {
        Self {
            content,
            content_start,
            bytes: content.as_bytes(),
            pos: 0,
        }
    }

    fn parse(mut self) -> Result<(String, Vec<Filter>), ParseError> {
        // Skip leading whitespace
        self.skip_whitespace();
        let var_start = self.pos;

        // Scan variable name (up to first unquoted | or end)
        let var_end = self.scan_until_unquoted_pipe();
        let var = self.content[var_start..var_end].trim().to_string();

        if var.is_empty() {
            return Err(ParseError::EmptyTag);
        }

        let mut filters = Vec::new();

        // Parse filters
        while self.pos < self.bytes.len() {
            // We're at a | character - skip it
            assert_eq!(self.bytes[self.pos], b'|');
            self.pos += 1;

            // Skip whitespace after |
            self.skip_whitespace();

            if self.pos >= self.bytes.len() {
                // Trailing pipe with nothing after - skip
                break;
            }

            // Parse single filter
            if let Some(filter) = self.parse_single_filter() {
                filters.push(filter);
            }
        }

        Ok((var, filters))
    }

    /// Parse a single filter: `name` or `name:arg`
    #[allow(clippy::cast_possible_truncation)]
    fn parse_single_filter(&mut self) -> Option<Filter> {
        let filter_byte_start = self.pos;
        let filter_start = self.content_start + filter_byte_start as u32;

        // Scan filter name (letters, digits, underscores - up to : or | or end)
        let name_start = self.pos;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b':' || b == b'|' || b.is_ascii_whitespace() {
                break;
            }
            self.pos += 1;
        }
        let name = self.content[name_start..self.pos].to_string();

        if name.is_empty() {
            // Empty filter name (e.g., `||` or `| |`)
            self.scan_until_unquoted_pipe();
            return None;
        }

        // Skip whitespace after name
        self.skip_whitespace();

        // Check for argument
        let arg = if self.pos < self.bytes.len() && self.bytes[self.pos] == b':' {
            self.pos += 1; // Skip :
            let arg_raw_start = self.pos;

            // Scan argument (quote-aware, up to unquoted | or end)
            let arg_raw_end = self.scan_until_unquoted_pipe();
            let arg_raw = &self.content[arg_raw_start..arg_raw_end];

            // Compute trimmed bounds for accurate span
            let leading_ws = arg_raw.len() - arg_raw.trim_start().len();
            let trailing_ws = arg_raw.len() - arg_raw.trim_end().len();
            let arg_trimmed_start = arg_raw_start + leading_ws;
            let arg_trimmed_end = arg_raw_end - trailing_ws;

            let arg_value = arg_raw.trim().to_string();

            if arg_value.is_empty() {
                None
            } else {
                let arg_span_start = self.content_start + arg_trimmed_start as u32;
                let arg_span_len = (arg_trimmed_end - arg_trimmed_start) as u32;

                Some(FilterArg {
                    value: arg_value,
                    span: Span::new(arg_span_start, arg_span_len),
                })
            }
        } else {
            // No argument - scan to next | anyway
            self.scan_until_unquoted_pipe();
            None
        };

        // Calculate filter span (from name start to end of arg or name)
        let filter_byte_end = self.pos;
        let trimmed_content = self.content[filter_byte_start..filter_byte_end].trim_end();
        let filter_span = Span::new(filter_start, trimmed_content.len() as u32);

        Some(Filter {
            name,
            arg,
            span: filter_span,
        })
    }

    /// Scan forward until we hit an unquoted `|` or end of content.
    /// Returns the byte position (exclusive).
    ///
    /// Handles escape sequences inside quoted strings:
    /// - `\'` inside single quotes does NOT close the quote
    /// - `\"` inside double quotes does NOT close the quote
    /// - `\\` is an escaped backslash (consumes the next byte)
    fn scan_until_unquoted_pipe(&mut self) -> usize {
        let mut quote_state = QuoteState::None;

        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];

            // Handle escape sequences inside quotes
            // When we see \, consume the next byte unconditionally
            if quote_state != QuoteState::None && b == b'\\' {
                self.pos += 1; // Skip the backslash
                if self.pos < self.bytes.len() {
                    self.pos += 1; // Skip the escaped character
                }
                continue;
            }

            match (quote_state, b) {
                // Opening quotes
                (QuoteState::None, b'\'') => quote_state = QuoteState::Single,
                (QuoteState::None, b'"') => quote_state = QuoteState::Double,

                // Closing quotes (only if not escaped - handled above)
                (QuoteState::Single, b'\'') | (QuoteState::Double, b'"') => {
                    quote_state = QuoteState::None;
                }

                // Unquoted pipe - stop here
                (QuoteState::None, b'|') => return self.pos,

                // Everything else - continue
                _ => {}
            }

            self.pos += 1;
        }

        self.pos
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum StreamError {
    AtBeginning,
    BeforeStart,
    AtEnd,
    Empty,
    InvalidAccess,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum ParseError {
    #[error("Unexpected token: expected {expected:?}, found {found} at position {position}")]
    UnexpectedToken {
        expected: Vec<String>,
        found: String,
        position: usize,
    },

    #[error("Missing condition in '{tag}' tag at position {position}")]
    MissingCondition { tag: String, position: usize },

    #[error("Missing iterator in 'for' tag at position {position}")]
    MissingIterator { position: usize },

    #[error("Malformed variable at position {position}: {content}")]
    MalformedVariable { position: usize, content: String },

    #[error("Invalid filter syntax at position {position}: {reason}")]
    InvalidFilterSyntax { position: usize, reason: String },

    #[error("Unclosed tag at position {opener}: expected '{expected_closer}'")]
    UnclosedTag {
        opener: usize,
        expected_closer: String,
    },

    #[error("Invalid syntax: {context}")]
    InvalidSyntax { context: String },

    #[error("Empty tag")]
    EmptyTag,

    #[error("Malformed Django construct at position {position}: {content}")]
    MalformedConstruct { position: usize, content: String },

    #[error("Stream error: {kind:?}")]
    StreamError { kind: StreamError },
}

impl ParseError {
    pub fn stream_error(kind: impl Into<StreamError>) -> Self {
        Self::StreamError { kind: kind.into() }
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::lexer::Lexer;

    fn parse_test_template(source: &str) -> Vec<Node> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        let (nodes, _errors) = parser.parse();
        nodes
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct TestNodeList {
        nodelist: Vec<TestNode>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct TestFilter {
        name: String,
        arg: Option<String>,
        span: (u32, u32),
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    #[serde(tag = "type")]
    enum TestNode {
        Tag {
            name: String,
            bits: Vec<String>,
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
                Node::Tag { name, bits, span } => TestNode::Tag {
                    name: name.clone(),
                    bits: bits.clone(),
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
                Node::Variable { var, filters, span } => TestNode::Variable {
                    var: var.clone(),
                    filters: filters
                        .iter()
                        .map(|f| TestFilter {
                            name: f.name.clone(),
                            arg: f.arg.as_ref().map(|a| a.value.clone()),
                            span: f.span.into(),
                        })
                        .collect(),
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

        // TODO: fix this so we can test against errors returned by parsing
        // #[test]
        // fn test_parse_error_recovery() {
        //     let source = r#"<div class="container">
        //     <h1>Header</h1>
        //     {% %}
        //         {# This if is unclosed which does matter #}
        //         <p>Welcome {{ user.name }}</p>
        //         <div>
        //             {# This div is unclosed which doesn't matter #}
        //         {% for item in items %}
        //             <span>{{ item }}</span>
        //         {% endfor %}
        //     <footer>Page Footer</footer>
        // </div>"#;
        //     let tokens = Lexer::new(source).tokenize().unwrap();
        //     let mut parser = create_test_parser(tokens);
        //     let (nodelist, errors) = parser.parse().unwrap();
        //     let nodelist = convert_nodelist_for_testing(ast.nodelist(parser.db), parser.db);
        //     insta::assert_yaml_snapshot!(nodelist);
        //     assert_eq!(errors.len(), 1);
        //     assert!(matches!(&errors[0], ParserError::EmptyTag));
        // }
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

    mod filter_parsing_tests {
        use super::*;

        #[test]
        fn test_pipe_inside_quotes_not_split() {
            // CRITICAL: This is the main bug the quote-aware scanner fixes
            let source = r#"{{ x|default:"a|b" }}"#;
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
            // Should produce ONE filter "default" with arg "\"a|b\"", NOT two filters
        }

        #[test]
        fn test_pipe_inside_single_quotes() {
            let source = "{{ x|default:'a|b|c' }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_colon_inside_quotes() {
            let source = r#"{{ x|date:"H:i:s" }}"#;
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
            // Arg should be "\"H:i:s\"", not split at colons
        }

        #[test]
        fn test_whitespace_preserved_in_spans() {
            // Whitespace around var and pipes
            let source = "{{ value | filter1 | filter2 }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_no_whitespace() {
            let source = "{{value|filter1|filter2}}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_leading_whitespace_before_var() {
            let source = "{{  value|filter }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_trailing_pipe() {
            // Trailing pipe with nothing after - should not panic
            let source = "{{ value| }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_empty_between_pipes() {
            // Empty filter between pipes - should skip gracefully
            let source = "{{ value||filter }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_filter_span_accuracy() {
            // Verify spans point to exact bytes
            let source = "{{ v|ab|cd:x }}";
            //              0123456789...
            // v is at 3, ab is at 5-6, cd:x is at 8-11
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_filter_with_variable_arg() {
            // Filter argument that's a variable, not quoted
            let source = "{{ value|default:other_var }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_filter_with_numeric_arg() {
            let source = "{{ value|truncatewords:30 }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_filter_arg_span_with_whitespace_after_colon() {
            // Whitespace after : should NOT be included in arg span
            let source = "{{ x|default: 'value' }}";
            //                         ^--- arg should start here, not at space
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
            // Arg span should point to 'value', not " 'value'"
        }

        #[test]
        fn test_filter_arg_span_with_whitespace_both_sides() {
            let source = "{{ x|default:  'value'  }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
            // Arg value should be "'value'" and span should exclude surrounding whitespace
        }

        #[test]
        fn test_complex_chain() {
            let source = r#"{{ value|default:"N/A"|title|truncatewords:5 }}"#;
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        // === ESCAPE SEQUENCE TESTS ===

        #[test]
        fn test_escaped_quote_in_double_quotes() {
            // \" inside double quotes should NOT close the quote
            let source = r#"{{ x|default:"say \"hello\"" }}"#;
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
            // Should be ONE filter with arg: "say \"hello\""
        }

        #[test]
        fn test_escaped_quote_in_single_quotes() {
            // \\' inside single quotes should NOT close the quote
            let source = r"{{ x|default:'it\'s fine' }}";
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_escaped_backslash() {
            // \\ is an escaped backslash
            let source = r#"{{ x|default:"path\\to\\file" }}"#;
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
        }

        #[test]
        fn test_escaped_pipe_in_quotes() {
            // Pipe after escaped quote should still be recognized as separator
            let source = r#"{{ x|default:"a\""|upper }}"#;
            let nodelist = parse_test_template(source);
            insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
            // Should be TWO filters: default with arg "a\"", then upper
        }
    }
}
