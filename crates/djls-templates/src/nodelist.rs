use djls_source::Span;

use crate::db::Db as TemplateDb;
use crate::parser::ParseError;
use crate::spans::SpanPair;

#[salsa::tracked(debug)]
pub struct NodeList<'db> {
    #[tracked]
    #[returns(ref)]
    pub nodelist: Vec<Node<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum Node<'db> {
    Tag {
        name: TagName<'db>,
        bits: Vec<TagBit<'db>>,
        spans: SpanPair,
    },
    Comment {
        content: String,
        spans: SpanPair,
    },
    Text {
        spans: SpanPair,
    },
    Variable {
        var: VariableName<'db>,
        filters: Vec<FilterName<'db>>,
        spans: SpanPair,
    },
    Error {
        node: ErrorNode,
    },
}

impl<'db> Node<'db> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Node::Tag { spans, .. }
            | Node::Variable { spans, .. }
            | Node::Comment { spans, .. }
            | Node::Text { spans, .. } => spans.content,
            Node::Error { node, .. } => node.spans.content,
        }
    }

    #[must_use]
    pub fn full_span(&self) -> Span {
        match self {
            Node::Variable { spans, .. }
            | Node::Comment { spans, .. }
            | Node::Tag { spans, .. }
            | Node::Text { spans, .. } => spans.lexeme,
            Node::Error { node } => node.spans.lexeme,
        }
    }

    pub fn identifier_span(&self, db: &'db dyn TemplateDb) -> Option<Span> {
        match self {
            Node::Tag { name, spans, .. } => {
                // Just the tag name (e.g., "if" in "{% if user.is_authenticated %}")
                let name_len = name.text(db).len();
                Some(Span {
                    start: spans.content.start,
                    length: u32::try_from(name_len).unwrap_or(0),
                })
            }
            Node::Variable { var, spans, .. } => {
                // Just the variable name (e.g., "user" in "{{ user.name|title }}")
                let var_len = var.text(db).len();
                Some(Span {
                    start: spans.content.start,
                    length: u32::try_from(var_len).unwrap_or(0),
                })
            }
            Node::Comment { .. } | Node::Text { .. } | Node::Error { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ErrorNode {
    pub spans: SpanPair,
    pub error: ParseError,
}

#[salsa::interned(debug)]
pub struct TagName<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct TagBit<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct VariableName<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct FilterName<'db> {
    pub text: String,
}
