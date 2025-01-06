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

    pub fn add_error(&mut self, error: AstError) {
        self.errors.push(error);
    }

    pub fn set_line_offsets(&mut self, line_offsets: LineOffsets) {
        self.line_offsets = line_offsets
    }

    pub fn finalize(&mut self) -> Result<Ast, AstError> {
        if self.nodes.is_empty() {
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
        // Find which line contains this offset by looking for the first line start
        // that's greater than our position
        let line = match self.0.binary_search(&offset) {
            Ok(exact_line) => exact_line, // We're exactly at a line start, so we're on that line
            Err(next_line) => {
                if next_line == 0 {
                    0 // Before first line start, so we're on line 0
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
        ((line as u32) + 1, col)
    }

    pub fn line_col_to_position(&self, line: u32, col: u32) -> u32 {
        self.0[(line - 1) as usize] + col
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Span {
    start: u32,
    length: u32,
}

impl Span {
    pub fn new(start: u32, length: u32) -> Self {
        Self { start, length }
    }

    pub fn start(&self) -> &u32 {
        &self.start
    }

    pub fn length(&self) -> &u32 {
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
    Variable {
        bits: Vec<String>,
        filters: Vec<DjangoFilter>,
        span: Span,
    },
    Block(Block),
}

impl Node {
    pub fn span(&self) -> Option<&Span> {
        match self {
            Node::Text { span, .. } => Some(span),
            Node::Comment { span, .. } => Some(span),
            Node::Variable { span, .. } => Some(span),
            Node::Block(block) => Some(&block.tag().span),
        }
    }

    pub fn children(&self) -> Option<&Vec<Node>> {
        match self {
            Node::Block(block) => block.nodes(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum Block {
    Block {
        tag: Tag,
        nodes: Vec<Node>,
        closing: Option<Box<Block>>,
        assignments: Option<Vec<Assignment>>,
    },
    Branch {
        tag: Tag,
        nodes: Vec<Node>,
    },
    Tag {
        tag: Tag,
    },
    Inclusion {
        tag: Tag,
        template_name: String,
    },
    Variable {
        tag: Tag,
    },
    Closing {
        tag: Tag,
    },
}

impl Block {
    pub fn tag(&self) -> &Tag {
        match self {
            Self::Block { tag, .. }
            | Self::Branch { tag, .. }
            | Self::Tag { tag }
            | Self::Inclusion { tag, .. }
            | Self::Variable { tag }
            | Self::Closing { tag } => tag,
        }
    }

    pub fn nodes(&self) -> Option<&Vec<Node>> {
        match self {
            Block::Block { nodes, .. } => Some(nodes),
            Block::Branch { nodes, .. } => Some(nodes),
            _ => None,
        }
    }

    pub fn closing(&self) -> Option<&Box<Block>> {
        match self {
            Block::Block { closing, .. } => closing.as_ref(),
            _ => None,
        }
    }

    pub fn assignments(&self) -> Option<&Vec<Assignment>> {
        match self {
            Block::Block { assignments, .. } => assignments.as_ref(),
            _ => None,
        }
    }

    pub fn template_name(&self) -> Option<&String> {
        match self {
            Block::Inclusion { template_name, .. } => Some(template_name),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Tag {
    pub name: String,
    pub bits: Vec<String>,
    pub span: Span,
    pub tag_span: Span,
    pub assignment: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Assignment {
    pub target: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DjangoFilter {
    pub name: String,
    pub args: Vec<String>,
    pub span: Span,
}

impl DjangoFilter {
    pub fn new(name: String, args: Vec<String>, span: Span) -> Self {
        Self { name, args, span }
    }
}

#[derive(Clone, Debug, Error, Serialize)]
pub enum AstError {
    #[error("Empty AST")]
    EmptyAst,
    #[error("Invalid tag: {0}")]
    InvalidTag(String),
    #[error("Unclosed block: {0}")]
    UnclosedBlock(String),
    #[error("Unclosed tag: {0}")]
    UnclosedTag(String),
    #[error("Stream error: {0}")]
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
            assert_eq!(offsets.position_to_line_col(0), (1, 0));
        }

        #[test]
        fn test_start_of_lines() {
            let mut offsets = LineOffsets::new();
            offsets.add_line(10); // Line 1
            offsets.add_line(25); // Line 2

            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Line 1
            assert_eq!(offsets.position_to_line_col(10), (2, 0)); // Line 2
            assert_eq!(offsets.position_to_line_col(25), (3, 0)); // Line 3
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
            let (ast, errors) = parser.parse().unwrap();
            assert!(errors.is_empty());

            // Find the variable node
            let nodes = ast.nodes();
            let var_node = nodes
                .iter()
                .find(|n| matches!(n, Node::Variable { .. }))
                .unwrap();

            if let Node::Variable { span, .. } = var_node {
                // Variable starts after newline + "{{"
                let (line, col) = ast.line_offsets().position_to_line_col(*span.start());
                assert_eq!(
                    (line, col),
                    (2, 3),
                    "Variable should start at line 2, col 3"
                );

                // Span should be exactly "user.name"
                assert_eq!(*span.length(), 9, "Variable span should cover 'user.name'");
            }
        }

        #[test]
        fn test_block_spans() {
            let nodes = vec![Node::Block(Block::Block {
                tag: Tag {
                    name: "if".to_string(),
                    bits: vec!["user.is_authenticated".to_string()],
                    span: Span::new(0, 35),
                    tag_span: Span::new(0, 35),
                    assignment: None,
                },
                nodes: vec![],
                closing: None,
                assignments: None,
            })];

            let ast = Ast {
                nodes,
                line_offsets: LineOffsets::new(),
                errors: vec![],
            };

            let node = &ast.nodes()[0];
            if let Node::Block(block) = node {
                assert_eq!(block.tag().span.start(), &0);
                assert_eq!(block.tag().span.length(), &35);
            } else {
                panic!("Expected Block node");
            }
        }

        #[test]
        fn test_multiline_template() {
            let template = "{% if user.active %}\n  Welcome!\n{% endif %}";
            let tokens = Lexer::new(template).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            assert!(errors.is_empty());

            let nodes = ast.nodes();
            if let Node::Block(Block::Block {
                tag,
                nodes,
                closing,
                ..
            }) = &nodes[0]
            {
                // Check block tag
                assert_eq!(tag.name, "if");
                assert_eq!(tag.bits, vec!["if", "user.active"]);

                // Check nodes
                eprintln!("Nodes: {:?}", nodes);
                assert_eq!(nodes.len(), 4);
                if let Node::Text { content, span } = &nodes[2] {
                    assert_eq!(content, "Welcome!");
                    eprintln!("Line offsets: {:?}", ast.line_offsets());
                    eprintln!("Span: {:?}", span);
                    let (line, col) = ast.line_offsets().position_to_line_col(span.start);
                    assert_eq!((line, col), (2, 2), "Content should be on line 2, col 2");

                    // Check closing tag
                    if let Block::Closing { tag } =
                        closing.as_ref().expect("Expected closing tag").as_ref()
                    {
                        assert_eq!(tag.name, "endif");
                    } else {
                        panic!("Expected closing block");
                    }
                } else {
                    panic!("Expected text node");
                }
            } else {
                panic!("Expected block node");
            }
        }
    }
}
