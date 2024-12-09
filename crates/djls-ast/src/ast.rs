use serde::Serialize;
use std::collections::BTreeMap;
use std::str::FromStr;
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

#[derive(Clone, Debug, Serialize)]
pub enum DjangoTagKind {
    Autoescape,
    Block,
    Comment,
    CsrfToken,
    Cycle,
    Debug,
    Elif,
    Else,
    Empty,
    Extends,
    Filter,
    FirstOf,
    For,
    If,
    IfChanged,
    Include,
    Load,
    Lorem,
    Now,
    Other(String),
    Querystring, // 5.1
    Regroup,
    ResetCycle,
    Spaceless,
    TemplateTag,
    Url,
    Verbatim,
    WidthRatio,
    With,
}

impl DjangoTagKind {
    const AUTOESCAPE: &'static str = "autoescape";
    const BLOCK: &'static str = "block";
    const COMMENT: &'static str = "comment";
    const CSRF_TOKEN: &'static str = "csrf_token";
    const CYCLE: &'static str = "cycle";
    const DEBUG: &'static str = "debug";
    const ELIF: &'static str = "elif";
    const ELSE: &'static str = "else";
    const EMPTY: &'static str = "empty";
    const EXTENDS: &'static str = "extends";
    const FILTER: &'static str = "filter";
    const FIRST_OF: &'static str = "firstof";
    const FOR: &'static str = "for";
    const IF: &'static str = "if";
    const IF_CHANGED: &'static str = "ifchanged";
    const INCLUDE: &'static str = "include";
    const LOAD: &'static str = "load";
    const LOREM: &'static str = "lorem";
    const NOW: &'static str = "now";
    const QUERYSTRING: &'static str = "querystring";
    const REGROUP: &'static str = "regroup";
    const RESET_CYCLE: &'static str = "resetcycle";
    const SPACELESS: &'static str = "spaceless";
    const TEMPLATE_TAG: &'static str = "templatetag";
    const URL: &'static str = "url";
    const VERBATIM: &'static str = "verbatim";
    const WIDTH_RATIO: &'static str = "widthratio";
    const WITH: &'static str = "with";
}

impl FromStr for DjangoTagKind {
    type Err = AstError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(AstError::EmptyTag);
        }

        match s {
            Self::AUTOESCAPE => Ok(Self::Autoescape),
            Self::BLOCK => Ok(Self::Block),
            Self::COMMENT => Ok(Self::Comment),
            Self::CSRF_TOKEN => Ok(Self::CsrfToken),
            Self::CYCLE => Ok(Self::Cycle),
            Self::DEBUG => Ok(Self::Debug),
            Self::ELIF => Ok(Self::Elif),
            Self::ELSE => Ok(Self::Else),
            Self::EMPTY => Ok(Self::Empty),
            Self::EXTENDS => Ok(Self::Extends),
            Self::FILTER => Ok(Self::Filter),
            Self::FIRST_OF => Ok(Self::FirstOf),
            Self::FOR => Ok(Self::For),
            Self::IF => Ok(Self::If),
            Self::IF_CHANGED => Ok(Self::IfChanged),
            Self::INCLUDE => Ok(Self::Include),
            Self::LOAD => Ok(Self::Load),
            Self::LOREM => Ok(Self::Lorem),
            Self::NOW => Ok(Self::Now),
            Self::QUERYSTRING => Ok(Self::Querystring),
            Self::REGROUP => Ok(Self::Regroup),
            Self::RESET_CYCLE => Ok(Self::ResetCycle),
            Self::SPACELESS => Ok(Self::Spaceless),
            Self::TEMPLATE_TAG => Ok(Self::TemplateTag),
            Self::URL => Ok(Self::Url),
            Self::VERBATIM => Ok(Self::Verbatim),
            Self::WIDTH_RATIO => Ok(Self::WidthRatio),
            Self::WITH => Ok(Self::With),
            other => Ok(Self::Other(other.to_string())),
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
