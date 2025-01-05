use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Ast {
    nodes: Vec<Node>,
    errors: Vec<AstError>,
}

impl Ast {
    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
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

    pub fn finalize(&mut self) -> Result<Ast, AstError> {
        if self.nodes.is_empty() && self.errors.is_empty() {
            return Err(AstError::EmptyAst);
        }
        Ok(self.clone())
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum Node {
    Text(String),
    Comment(String),
    Block {
        block_type: BlockType,
        name: String,
        bits: Vec<String>,
        children: Option<Vec<Node>>,
    },
    Variable {
        bits: Vec<String>,
        filters: Vec<DjangoFilter>,
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
}

impl DjangoFilter {
    pub fn new(name: String, arguments: Vec<String>) -> Self {
        Self { name, arguments }
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
