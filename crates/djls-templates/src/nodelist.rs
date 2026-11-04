use djls_source::Span;

use crate::bits::TagBit;
use crate::filters::Filter;
use crate::parser::ParseError;
use crate::tokens::TagDelimiter;

/// Owned text for tag names, tag arguments, variables and filters. Most fit inline, and
/// each template's nodes are copied into its layout and structure tree.
pub type NodeText = compact_str::CompactString;

#[salsa::tracked(debug)]
pub struct NodeList<'db> {
    #[tracked]
    #[returns(ref)]
    pub nodelist: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Tag {
        name: NodeText,
        name_span: Span,
        bits: Box<[TagBit]>,
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
        var: NodeText,
        var_span: Span,
        filters: Box<[Filter]>,
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
