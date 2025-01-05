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
pub struct LineOffsets(Vec<u32>);

impl LineOffsets {
    pub fn new() -> Self {
        let offsets = vec![0];
        Self(offsets)
    }

    pub fn add_line(&mut self, offset: u32) {
        self.0.push(offset);
    }

    pub fn position_to_line_col(&self, offset: u32) -> (u32, u32) {
        let line = match self.0.binary_search(&offset) {
            Ok(line) => line,
            Err(line) => line - 1,
        };
        let col = offset - self.0[line];
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

    #[test]
    fn test_line_offsets() {
        let mut offsets = LineOffsets::new();
        offsets.add_line(10); // Line 1 starts at offset 10
        offsets.add_line(25); // Line 2 starts at offset 25
        offsets.add_line(40); // Line 3 starts at offset 40

        // Test position_to_line_col
        assert_eq!(offsets.position_to_line_col(0), (0, 0)); // Start of first line
        assert_eq!(offsets.position_to_line_col(5), (0, 5)); // Middle of first line
        assert_eq!(offsets.position_to_line_col(10), (1, 0)); // Start of second line
        assert_eq!(offsets.position_to_line_col(15), (1, 5)); // Middle of second line
        assert_eq!(offsets.position_to_line_col(25), (2, 0)); // Start of third line
        assert_eq!(offsets.position_to_line_col(35), (2, 10)); // Middle of third line
        assert_eq!(offsets.position_to_line_col(40), (3, 0)); // Start of fourth line
        assert_eq!(offsets.position_to_line_col(45), (3, 5)); // Middle of fourth line

        // Test line_col_to_position
        assert_eq!(offsets.line_col_to_position(0, 0), 0); // Start of first line
        assert_eq!(offsets.line_col_to_position(0, 5), 5); // Middle of first line
        assert_eq!(offsets.line_col_to_position(1, 0), 10); // Start of second line
        assert_eq!(offsets.line_col_to_position(1, 5), 15); // Middle of second line
        assert_eq!(offsets.line_col_to_position(2, 0), 25); // Start of third line
        assert_eq!(offsets.line_col_to_position(2, 10), 35); // Middle of third line
        assert_eq!(offsets.line_col_to_position(3, 0), 40); // Start of fourth line
        assert_eq!(offsets.line_col_to_position(3, 5), 45); // Middle of fourth line
    }
}
