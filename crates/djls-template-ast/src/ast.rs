use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Ast {
    nodes: Vec<Node>,
}

impl Ast {
    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
    }

    pub fn add_node(&mut self, node: Node) {
        self.nodes.push(node);
    }

    pub fn finalize(&mut self) -> Result<Ast, AstError> {
        if self.nodes.is_empty() {
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
    Tag {
        kind: DjangoTagKind,
        bits: Vec<String>,
        children: Vec<Node>,
    },
    Variable {
        bits: Vec<String>,
        filters: Vec<DjangoFilter>,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct DjangoTagKind {
    pub name: String,
    pub spec_path: String,
    pub is_intermediate_of: Option<String>,
}

impl DjangoTagKind {
    pub fn from_str(name: &str, spec_path: &str) -> Self {
        DjangoTagKind {
            name: name.to_string(),
            spec_path: spec_path.to_string(),
            is_intermediate_of: None,
        }
    }

    pub fn as_intermediate(name: &str, spec_path: &str, parent: &str) -> Self {
        DjangoTagKind {
            name: name.to_string(),
            spec_path: spec_path.to_string(),
            is_intermediate_of: Some(parent.to_string()),
        }
    }
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

#[derive(Error, Debug)]
pub enum AstError {
    #[error("error parsing django tag, recieved empty tag name")]
    EmptyTag,
    #[error("empty ast")]
    EmptyAst,
}
