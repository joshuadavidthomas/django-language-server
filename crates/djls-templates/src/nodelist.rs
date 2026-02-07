use djls_source::Span;
use serde::Serialize;

use crate::parser::ParseError;
use crate::tokens::TagDelimiter;

#[salsa::tracked(debug)]
pub struct NodeList<'db> {
    #[tracked]
    #[returns(ref)]
    pub nodelist: Vec<Node>,
}

/// A parsed filter expression within a Django variable node.
///
/// Represents a single filter in a chain like `{{ value|default:'nothing'|title }}`.
/// Each filter has a name, an optional argument, and a span covering its position
/// within the source text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Filter {
    pub name: String,
    pub arg: Option<String>,
    pub span: Span,
}

impl Filter {
    #[must_use]
    pub fn new(name: String, arg: Option<String>, span: Span) -> Self {
        Self { name, arg, span }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
        span: Span,
    },
    Variable {
        var: String,
        filters: Vec<Filter>,
        span: Span,
    },
    Error {
        span: Span,
        full_span: Span,
        error: ParseError,
    },
}

impl Node {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Node::Tag { span, .. }
            | Node::Variable { span, .. }
            | Node::Comment { span, .. }
            | Node::Text { span, .. }
            | Node::Error { span, .. } => *span,
        }
    }

    #[must_use]
    pub fn full_span(&self) -> Span {
        match self {
            Node::Variable { span, .. } | Node::Comment { span, .. } | Node::Tag { span, .. } => {
                span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
            }
            Node::Text { span, .. } => *span,
            Node::Error { full_span, .. } => *full_span,
        }
    }

    #[must_use]
    pub fn identifier_span(&self) -> Option<Span> {
        match self {
            Node::Tag { name, span, .. } => {
                // Just the tag name (e.g., "if" in "{% if user.is_authenticated %}")
                Some(span.with_length_usize_saturating(name.len()))
            }
            Node::Variable { var, span, .. } => {
                // Just the variable name (e.g., "user" in "{{ user.name|title }}")
                Some(span.with_length_usize_saturating(var.len()))
            }
            Node::Comment { .. } | Node::Text { .. } | Node::Error { .. } => None,
        }
    }
}
