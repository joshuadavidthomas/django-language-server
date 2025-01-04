use serde::Serialize;
use std::collections::BTreeMap;
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
    Django(DjangoNode),
    Html(HtmlNode),
    Script(ScriptNode),
    Style(StyleNode),
    Text(String),
}

#[derive(Clone, Debug, Serialize)]
pub enum DjangoNode {
    Comment(String),
    Tag(TagNode),
    Variable {
        bits: Vec<String>,
        filters: Vec<DjangoFilter>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub enum TagNode {
    Block {
        name: String,
        bits: Vec<String>,
        children: Vec<Node>,
    },
    Branch {
        name: String,
        bits: Vec<String>,
        children: Vec<Node>,
    },
    Closing {
        name: String,
        bits: Vec<String>,
    },
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

#[derive(Clone, Debug, Serialize)]
pub enum HtmlNode {
    Comment(String),
    Doctype(String),
    Element {
        tag_name: String,
        attributes: Attributes,
        children: Vec<Node>,
    },
    Void {
        tag_name: String,
        attributes: Attributes,
    },
}

#[derive(Clone, Debug, Serialize)]
pub enum ScriptNode {
    Comment {
        content: String,
        kind: ScriptCommentKind,
    },
    Element {
        attributes: Attributes,
        children: Vec<Node>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub enum ScriptCommentKind {
    SingleLine, // //
    MultiLine,  // /* */
}

#[derive(Clone, Debug, Serialize)]
pub enum StyleNode {
    Comment(String),
    Element {
        attributes: Attributes,
        children: Vec<Node>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub enum AttributeValue {
    Value(String),
    Boolean,
}

pub type Attributes = BTreeMap<String, AttributeValue>;

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
    #[error("invalid tag: {0}")]
    InvalidTag(String),
    #[error("block error: {0} in {1}")]
    BlockError(String, String),
    #[error("stream error: {0}")]
    StreamError(String),
    #[error("token error: {0}")]
    TokenError(String),
    #[error("argument error: {0} - {1}")]
    ArgumentError(String, String),
}
