use djls_source::Span;

use crate::parser::ParseError;
use crate::tokens::TagDelimiter;

/// A parsed filter in a variable expression.
///
/// Represents `|filter_name:arg` in `{{ var|filter_name:arg }}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    pub arg: Option<FilterArg>,
    pub span: Span,
}

impl Filter {
    /// Span of just the filter name.
    #[must_use]
    pub fn name_span(&self) -> Span {
        self.span.with_length_usize_saturating(self.name.len())
    }
}

/// A filter argument.
///
/// The value is stored as the raw string from the template, including quotes.
/// E.g., for `|default:'nothing'`, the value is `'nothing'` (with quotes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterArg {
    pub value: String,
    pub span: Span,
}

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
