use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Ast {
    nodes: Vec<Node>,
    line_offsets: LineOffsets,
    errors: Vec<AstError>,
}

impl Ast {
    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
    }

    pub fn line_offsets(&self) -> &LineOffsets {
        &self.line_offsets
    }

    pub fn errors(&self) -> &Vec<AstError> {
        &self.errors
    }

    pub fn add_node(&mut self, node: Node) {
        self.nodes.push(node);
    }

    pub fn set_line_offsets(&mut self, line_offsets: LineOffsets) {
        self.line_offsets = line_offsets
    }

    pub fn add_error(&mut self, error: AstError) {
        self.errors.push(error);
    }

    pub fn finalize(&mut self) -> Result<Ast, AstError> {
        if self.nodes.is_empty() && self.errors.is_empty() {
            return Err(AstError::EmptyAst);
        }
        Ok(self.clone())
    }
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct LineOffsets(pub Vec<u32>);

impl LineOffsets {
    pub fn new() -> Self {
        let offsets = vec![0];
        Self(offsets)
    }

    pub fn add_line(&mut self, offset: u32) {
        self.0.push(offset);
    }

    pub fn position_to_line_col(&self, offset: u32) -> (u32, u32) {
        eprintln!("LineOffsets: Converting position {} to line/col. Offsets: {:?}", offset, self.0);
        
        // Find which line contains this offset by looking for the first line start
        // that's greater than our position
        let line = match self.0.binary_search(&offset) {
            Ok(exact_line) => exact_line,  // We're exactly at a line start, so we're on that line
            Err(next_line) => {
                if next_line == 0 {
                    0  // Before first line start, so we're on line 0
                } else {
                    let prev_line = next_line - 1;
                    // If we're at the start of the next line, we're on that line
                    if offset == self.0[next_line] - 1 {
                        prev_line
                    } else {
                        // Otherwise we're on the previous line
                        prev_line
                    }
                }
            }
        };
        
        // Calculate column as offset from line start
        let col = offset - self.0[line];
        
        eprintln!("LineOffsets: Found line {} starting at offset {}", line, self.0[line]);
        eprintln!("LineOffsets: Calculated col {} as {} - {}", col, offset, self.0[line]);
        (line as u32, col)
    }

    pub fn line_col_to_position(&self, line: u32, col: u32) -> u32 {
        self.0[line as usize] + col
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Span {
    start: u32,
    length: u16,
}

impl Span {
    pub fn new(start: u32, length: u16) -> Self {
        Self { start, length }
    }

    pub fn start(&self) -> &u32 {
        &self.start
    }

    pub fn length(&self) -> &u16 {
        &self.length
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum Node {
    Text {
        content: String,
        span: Span,
    },
    Comment {
        content: String,
        span: Span,
    },
    Block {
        block_type: BlockType,
        name: String,
        bits: Vec<String>,
        children: Option<Vec<Node>>,
        span: Span,
        tag_span: Span,
    },
    Variable {
        bits: Vec<String>,
        filters: Vec<DjangoFilter>,
        span: Span,
    },
}

#[derive(Clone, Debug, Serialize)]
pub enum BlockType {
    Standard,
    Branch,
    Closing,
}

#[derive(Clone, Debug, Serialize)]
pub struct DjangoFilter {
    name: String,
    arguments: Vec<String>,
    span: Span,
}

impl DjangoFilter {
    pub fn new(name: String, arguments: Vec<String>, span: Span) -> Self {
        Self {
            name,
            arguments,
            span,
        }
    }
}

#[derive(Clone, Debug, Error, Serialize)]
pub enum AstError {
    #[error("Empty AST")]
    EmptyAst,
    #[error("Empty tag")]
    EmptyTag,
    #[error("unclosed tag: {0}")]
    UnclosedTag(String),
    #[error("unexpected tag: {0}")]
    UnexpectedTag(String),
    #[error("stream error: {0}")]
    StreamError(String),
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
            let offsets = LineOffsets::new();
            assert_eq!(offsets.position_to_line_col(0), (0, 0));
        }

        #[test]
        fn test_start_of_lines() {
            let mut offsets = LineOffsets::new();
            offsets.add_line(10); // Line 1
            offsets.add_line(25); // Line 2

            assert_eq!(offsets.position_to_line_col(0), (0, 0)); // Line 0
            assert_eq!(offsets.position_to_line_col(10), (1, 0)); // Line 1
            assert_eq!(offsets.position_to_line_col(25), (2, 0)); // Line 2
        }
    }

    mod spans_and_positions {
        use super::*;

        #[test]
        fn test_variable_spans() {
            let template = "Hello\n{{ user.name }}\nWorld";
            let tokens = Lexer::new(template).tokenize().unwrap();
            println!("Tokens: {:#?}", tokens); // Add debug print
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();

            // Find the variable node
            let nodes = ast.nodes();
            let var_node = nodes
                .iter()
                .find(|n| matches!(n, Node::Variable { .. }))
                .unwrap();

            if let Node::Variable { span, .. } = var_node {
                // Variable starts after newline + "{{"
                let (line, col) = ast.line_offsets.position_to_line_col(*span.start());
                assert_eq!(
                    (line, col),
                    (1, 3),
                    "Variable should start at line 1, col 3"
                );

                // Span should be exactly "user.name"
                assert_eq!(*span.length(), 9, "Variable span should cover 'user.name'");
            }
        }

        #[test]
        fn test_block_spans() {
            let template = "{% if user.active %}\n  Welcome!\n{% endif %}";
            let tokens = Lexer::new(template).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();

            // Find the block node
            let nodes = ast.nodes();
            if let Node::Block {
                span,
                tag_span,
                children,
                ..
            } = &nodes[0]
            {
                // Check opening tag span
                let (tag_line, tag_col) = ast.line_offsets.position_to_line_col(*tag_span.start());
                assert_eq!(
                    (tag_line, tag_col),
                    (0, 0),
                    "Opening tag should start at beginning"
                );

                // Check content span
                if let Some(content) = children {
                    if let Node::Text { span, .. } = &content[0] {
                        eprintln!("content {:?}", content);
                        eprintln!("span start {:?}", span.start());
                        let (content_line, content_col) =
                            ast.line_offsets.position_to_line_col(*span.start());
                        assert_eq!(
                            (content_line, content_col),
                            (1, 2),
                            "Content should be indented"
                        );
                    }
                }

                // Full block span should cover entire template
                assert_eq!(*span.length() as u32, template.len() as u32);
            }
        }

        #[test]
        fn test_multiline_template() {
            let template = "\
<div>
    {% if user.is_authenticated %}
        {{ user.name }}
        {% if user.is_staff %}
            (Staff)
        {% endif %}
    {% endif %}
</div>";
            let tokens = Lexer::new(template).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();

            // Test nested block positions
            let (outer_if, inner_if) = {
                let nodes = ast.nodes();
                let outer = nodes
                    .iter()
                    .find(|n| matches!(n, Node::Block { .. }))
                    .unwrap();
                let inner = if let Node::Block { children, .. } = outer {
                    children
                        .as_ref()
                        .unwrap()
                        .iter()
                        .find(|n| matches!(n, Node::Block { .. }))
                        .unwrap()
                } else {
                    panic!("Expected block with children");
                };
                (outer, inner)
            };

            if let (
                Node::Block {
                    span: outer_span, ..
                },
                Node::Block {
                    span: inner_span, ..
                },
            ) = (outer_if, inner_if)
            {
                // Verify outer if starts at the right line/column
                let (outer_line, outer_col) =
                    ast.line_offsets.position_to_line_col(*outer_span.start());
                assert_eq!(
                    (outer_line, outer_col),
                    (1, 4),
                    "Outer if should be indented"
                );

                // Verify inner if is more indented than outer if
                let (inner_line, inner_col) =
                    ast.line_offsets.position_to_line_col(*inner_span.start());
                assert!(inner_col > outer_col, "Inner if should be more indented");
                assert!(inner_line > outer_line, "Inner if should be on later line");
            }
        }
    }
}
