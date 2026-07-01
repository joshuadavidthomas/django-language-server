use djls_source::Span;

use crate::bits::TagBit;
use crate::filters::Filter;
use crate::parser::ParseError;
use crate::tokens::TagDelimiter;

#[salsa::tracked(debug)]
pub struct NodeList<'db> {
    #[tracked]
    #[returns(ref)]
    pub nodelist: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Tag {
        name: String,
        name_span: Span,
        bits: Vec<TagBit>,
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
        var_span: Span,
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
    pub fn full_span(&self) -> Span {
        match self {
            Node::Variable { span, .. } | Node::Comment { span, .. } | Node::Tag { span, .. } => {
                span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32)
            }
            Node::Text { span, .. } => *span,
            Node::Error { full_span, .. } => *full_span,
        }
    }
}
