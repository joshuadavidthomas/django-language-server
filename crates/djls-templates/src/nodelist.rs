use djls_source::Db;
use djls_source::Offset;
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

impl<'db> NodeList<'db> {
    #[must_use]
    pub fn node_at(self, db: &'db dyn Db, offset: Offset) -> Option<&'db Node> {
        self.nodelist(db)
            .iter()
            .find(|node| node.full_span().contains(offset))
    }
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
            Node::Tag { name_span, .. } => Some(*name_span),
            Node::Variable { var_span, .. } => Some(*var_span),
            Node::Comment { .. } | Node::Text { .. } | Node::Error { .. } => None,
        }
    }
}
