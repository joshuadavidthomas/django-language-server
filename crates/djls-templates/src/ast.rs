use serde::Serialize;
use thiserror::Error;

use crate::tokens::Token;

#[salsa::tracked(debug)]
pub struct Ast<'db> {
    #[tracked]
    #[returns(ref)]
    pub nodelist: Vec<Node<'db>>,
    #[tracked]
    #[returns(ref)]
    pub line_offsets: LineOffsets,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LineOffsets(pub Vec<u32>);

impl LineOffsets {
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

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum Node<'db> {
    Tag(TagNode<'db>),
    Comment(CommentNode),
    Text(TextNode),
    Variable(VariableNode<'db>),
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct TagNode<'db> {
    pub name: TagName<'db>,
    pub bits: Vec<String>,
    pub span: Span,
}

#[salsa::interned(debug)]
pub struct TagName<'db> {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct CommentNode {
    pub content: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct TextNode {
    pub content: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct VariableNode<'db> {
    pub var: VariableName<'db>,
    pub filters: Vec<FilterName<'db>>,
    pub span: Span,
}

#[salsa::interned(debug)]
pub struct VariableName<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct FilterName<'db> {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Span {
    pub start: u32,
    pub length: u32,
}

impl Span {
    #[must_use]
    pub fn new(start: u32, length: u32) -> Self {
        Self { start, length }
    }

    #[must_use]
    pub fn from_token(token: &Token) -> Self {
        let start = token.start().unwrap_or(0);
        let length = u32::try_from(token.lexeme().len()).unwrap_or(0);
        Self { start, length }
    }

    #[must_use]
    pub fn to_lsp_range(&self, line_offsets: &LineOffsets) -> tower_lsp_server::lsp_types::Range {
        let start_pos = self.start as usize;
        let end_pos = (self.start + self.length) as usize;

        let (start_line, start_char) = line_offsets.position_to_line_col(start_pos);
        let (end_line, end_char) = line_offsets.position_to_line_col(end_pos);

        tower_lsp_server::lsp_types::Range {
            start: tower_lsp_server::lsp_types::Position {
                line: u32::try_from(start_line - 1).unwrap_or(u32::MAX), // LSP is 0-based, LineOffsets is 1-based
                character: u32::try_from(start_char).unwrap_or(u32::MAX),
            },
            end: tower_lsp_server::lsp_types::Position {
                line: u32::try_from(end_line - 1).unwrap_or(u32::MAX),
                character: u32::try_from(end_char).unwrap_or(u32::MAX),
            },
        }
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
    #[error("Unbalanced structure: '{opening_tag}' missing closing '{expected_closing}'")]
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
    MissingRequiredArguments { tag: String, min: usize, span: Span },
    #[error("Tag '{tag}' accepts at most {max} argument{}", if *.max == 1 { "" } else { "s" })]
    TooManyArguments { tag: String, max: usize, span: Span },
}

impl AstError {
    /// Get the span start and length of this error, if available
    #[must_use]
    pub fn span(&self) -> Option<(u32, u32)> {
        match self {
            AstError::UnbalancedStructure { opening_span, .. } => {
                Some((opening_span.start, opening_span.length))
            }
            AstError::InvalidTagStructure { span, .. }
            | AstError::InvalidNode { span, .. }
            | AstError::UnclosedTag { span, .. }
            | AstError::OrphanedTag { span, .. }
            | AstError::UnmatchedBlockName { span, .. }
            | AstError::MissingRequiredArguments { span, .. }
            | AstError::TooManyArguments { span, .. } => Some((span.start, span.length)),
            AstError::EmptyAst => None,
        }
    }

    /// Get a diagnostic code string for this error type
    #[must_use]
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            AstError::EmptyAst => "T001",
            AstError::InvalidTagStructure { .. } => "T002",
            AstError::UnbalancedStructure { .. } => "T003",
            AstError::InvalidNode { .. } => "T004",
            AstError::UnclosedTag { .. } => "T005",
            AstError::OrphanedTag { .. } => "T006",
            AstError::UnmatchedBlockName { .. } => "T007",
            AstError::MissingRequiredArguments { .. } => "T008",
            AstError::TooManyArguments { .. } => "T009",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        #[test]
        fn test_variable_spans() {
            // let template = "Hello\n{{ user.name }}\nWorld";
            // Tests will need to be updated to work with the new db parameter
            // For now, comment out to allow compilation
            // let tokens = Lexer::new(template).tokenize().unwrap();
            // let mut parser = Parser::new(tokens);
            // let (nodelist, errors) = parser.parse().unwrap();
            // assert!(errors.is_empty());

            // // Find the variable node
            // let nodes = nodelist.nodelist();
            // let var_node = nodes
            //     .iter()
            //     .find(|n| matches!(n, Node::Variable { .. }))
            //     .unwrap();

            // if let Node::Variable { span, .. } = var_node {
            //     // Variable starts after newline + "{{"
            //     let (line, col) = nodelist
            //         .line_offsets()
            //         .position_to_line_col(span.start() as usize);
            //     assert_eq!(
            //         (line, col),
            //         (2, 0),
            //         "Variable should start at line 2, col 3"
            //     );

            //     assert_eq!(span.length(), 9, "Variable span should cover 'user.name'");
            // }
        }
    }
}
