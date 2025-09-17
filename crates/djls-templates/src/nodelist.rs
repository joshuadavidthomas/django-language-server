use djls_source::Span;

use crate::db::Db as TemplateDb;

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
        var: VariableName<'db>,
        filters: Vec<FilterName<'db>>,
        span: Span,
    },
}

impl<'db> Node<'db> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Node::Tag { span, .. }
            | Node::Variable { span, .. }
            | Node::Comment { span, .. }
            | Node::Text { span } => *span,
        }
    }

    #[must_use]
    pub fn full_span(&self) -> Span {
        match self {
            // account for delimiters
            Node::Variable { span, .. } | Node::Comment { span, .. } | Node::Tag { span, .. } => {
                Span {
                    start: span.start.saturating_sub(3),
                    length: span.length + 6,
                }
            }
            Node::Text { span } => *span,
        }
    }

    pub fn identifier_span(&self, db: &'db dyn TemplateDb) -> Option<Span> {
        match self {
            Node::Tag { name, span, .. } => {
                // Just the tag name (e.g., "if" in "{% if user.is_authenticated %}")
                let name_len = name.text(db).len();
                Some(Span {
                    start: span.start,
                    length: u32::try_from(name_len).unwrap_or(0),
                })
            }
            Node::Variable { var, span, .. } => {
                // Just the variable name (e.g., "user" in "{{ user.name|title }}")
                let var_len = var.text(db).len();
                Some(Span {
                    start: span.start,
                    length: u32::try_from(var_len).unwrap_or(0),
                })
            }
            Node::Comment { .. } | Node::Text { .. } => None,
        }
    }
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
