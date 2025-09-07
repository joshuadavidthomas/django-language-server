use serde::Serialize;
use thiserror::Error;

use crate::tokens::Token;
use crate::tokens::TokenStream;
use crate::tokens::TokenType;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Ast {
    nodelist: Vec<Node>,
    line_offsets: LineOffsets,
}

impl Ast {
    #[must_use]
    pub fn nodelist(&self) -> &Vec<Node> {
        &self.nodelist
    }

    #[must_use]
    pub fn line_offsets(&self) -> &LineOffsets {
        &self.line_offsets
    }

    pub fn add_node(&mut self, node: Node) {
        self.nodelist.push(node);
    }

    pub fn set_line_offsets(&mut self, tokens: &TokenStream) {
        for token in tokens.tokens() {
            if let TokenType::Newline = token.token_type() {
                if let Some(start) = token.start() {
                    // Add offset for next line
                    self.line_offsets.add_line(start + 1);
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LineOffsets(pub Vec<u32>);

impl LineOffsets {
    /// Create a new LineOffsets from source text
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut offsets = Self::default();
        let mut pos = 0;
        for ch in text.chars() {
            if ch == '\n' {
                offsets.add_line((pos + 1) as u32);
            }
            pos += ch.len_utf8();
        }
        offsets
    }

    pub fn add_line(&mut self, offset: u32) {
        self.0.push(offset);
    }

    #[must_use]
    pub fn position_to_line_col(&self, position: usize) -> (usize, usize) {
        let position = u32::try_from(position).unwrap_or_default();
        let line = match self.0.binary_search(&position) {
            Ok(exact_line) => exact_line,    // Position is at start of this line
            Err(0) => 0,                     // Before first line start
            Err(next_line) => next_line - 1, // We're on the previous line
        };

        // Calculate column as offset from line start
        let col = if line == 0 {
            position as usize
        } else {
            (position - self.0[line]) as usize
        };

        // Convert to 1-based line number
        (line + 1, col)
    }

    #[must_use]
    pub fn line_col_to_position(&self, line: u32, col: u32) -> u32 {
        // line is 1-based, so subtract 1 to get the index
        self.0[(line - 1) as usize] + col
    }
}

impl Default for LineOffsets {
    fn default() -> Self {
        Self(vec![0])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Node {
    Tag {
        name: String,
        bits: Vec<String>,
        span: Span,
    },
    Comment {
        content: String,
        span: Span,
    },
    Text {
        content: String,
        span: Span,
    },
    Variable {
        var: String,
        filters: Vec<String>,
        span: Span,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Span {
    start: u32,
    length: u32,
}

impl Span {
    #[must_use]
    pub fn new(start: u32, length: u32) -> Self {
        Self { start, length }
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    #[must_use]
    pub fn start(&self) -> u32 {
        self.start
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    #[must_use]
    pub fn length(&self) -> u32 {
        self.length
    }
}

impl From<Token> for Span {
    fn from(token: Token) -> Self {
        let start = token.start().unwrap_or(0);
        let length = u32::try_from(token.content().len()).unwrap_or(0);
        Span::new(start, length)
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum AstError {
    #[error("Empty AST")]
    EmptyAst,
    #[error("Invalid tag '{tag}' structure: {reason}")]
    InvalidTagStructure {
        tag: String,
        reason: String,
        span: Span,
    },
    #[error("Unbalanced structure: '{opening_tag}' at {opening_span:?} missing closing '{expected_closing}'")]
    UnbalancedStructure {
        opening_tag: String,
        expected_closing: String,
        opening_span: Span,
        closing_span: Option<Span>,
    },
    #[error("Invalid {node_type} node: {reason}")]
    InvalidNode {
        node_type: String,
        reason: String,
        span: Span,
    },
    #[error("Unclosed tag: {tag}")]
    UnclosedTag { tag: String, span: Span },
    #[error("Orphaned tag '{tag}' - {context}")]
    OrphanedTag {
        tag: String,
        context: String,
        span: Span,
    },
    #[error("endblock '{name}' does not match any open block")]
    UnmatchedBlockName { name: String, span: Span },
    #[error("Tag '{tag}' requires at least {min} argument{}", if *.min == 1 { "" } else { "s" })]
    MissingRequiredArguments { 
        tag: String, 
        min: usize,
        span: Span 
    },
    #[error("Tag '{tag}' accepts at most {max} argument{}", if *.max == 1 { "" } else { "s" })]
    TooManyArguments {
        tag: String,
        max: usize,
        span: Span
    },
}

impl AstError {
    /// Get the span of this error, if available
    #[must_use]
    pub fn span(&self) -> Option<Span> {
        match self {
            AstError::EmptyAst => None,
            AstError::InvalidTagStructure { span, .. } => Some(*span),
            AstError::UnbalancedStructure { opening_span, .. } => Some(*opening_span),
            AstError::InvalidNode { span, .. } => Some(*span),
            AstError::UnclosedTag { span, .. } => Some(*span),
            AstError::OrphanedTag { span, .. } => Some(*span),
            AstError::UnmatchedBlockName { span, .. } => Some(*span),
            AstError::MissingRequiredArguments { span, .. } => Some(*span),
            AstError::TooManyArguments { span, .. } => Some(*span),
        }
    }

    /// Get a numeric error code for this error type
    #[must_use]
    pub fn error_code(&self) -> u16 {
        match self {
            AstError::EmptyAst => 1,
            AstError::InvalidTagStructure { .. } => 2,
            AstError::UnbalancedStructure { .. } => 3,
            AstError::InvalidNode { .. } => 4,
            AstError::UnclosedTag { .. } => 5,
            AstError::OrphanedTag { .. } => 6,
            AstError::UnmatchedBlockName { .. } => 7,
            AstError::MissingRequiredArguments { .. } => 8,
            AstError::TooManyArguments { .. } => 9,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    mod line_offsets {
        use super::*;

        #[test]
        fn test_new_starts_at_zero() {
            let offsets = LineOffsets::default();
            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Line 1, column 0
        }

        #[test]
        fn test_start_of_lines() {
            let mut offsets = LineOffsets::default();
            offsets.add_line(10); // Line 2 starts at offset 10
            offsets.add_line(25); // Line 3 starts at offset 25

            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Line 1, start
            assert_eq!(offsets.position_to_line_col(10), (2, 0)); // Line 2, start
            assert_eq!(offsets.position_to_line_col(25), (3, 0)); // Line 3, start
        }
    }

    mod spans_and_positions {
        use super::*;

        #[test]
        fn test_variable_spans() {
            let template = "Hello\n{{ user.name }}\nWorld";
            let tokens = Lexer::new(template).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            assert!(errors.is_empty());

            // Find the variable node
            let nodes = nodelist.nodelist();
            let var_node = nodes
                .iter()
                .find(|n| matches!(n, Node::Variable { .. }))
                .unwrap();

            if let Node::Variable { span, .. } = var_node {
                // Variable starts after newline + "{{"
                let (line, col) = nodelist
                    .line_offsets()
                    .position_to_line_col(span.start() as usize);
                assert_eq!(
                    (line, col),
                    (2, 0),
                    "Variable should start at line 2, col 3"
                );

                assert_eq!(span.length(), 9, "Variable span should cover 'user.name'");
            }
        }
    }
}
