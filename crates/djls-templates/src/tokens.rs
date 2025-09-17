use djls_source::Span;

use crate::db::Db as TemplateDb;

const DJANGO_DELIM_LEN: u32 = 2;

#[derive(Clone, Debug, PartialEq, Hash, salsa::Update)]
pub enum Token<'db> {
    Block {
        content: TokenContent<'db>,
        span: Span,
    },
    Comment {
        content: TokenContent<'db>,
        span: Span,
    },
    Error {
        content: TokenContent<'db>,
        span: Span,
    },
    Eof,
    Newline {
        span: Span,
    },
    Text {
        content: TokenContent<'db>,
        span: Span,
    },
    Variable {
        content: TokenContent<'db>,
        span: Span,
    },
    Whitespace {
        span: Span,
    },
}

#[salsa::interned(debug)]
pub struct TokenContent<'db> {
    #[returns(ref)]
    pub text: String,
}

impl<'db> Token<'db> {
    /// Get the content text for content-bearing tokens
    pub fn content(&self, db: &'db dyn TemplateDb) -> String {
        match self {
            Token::Block { content, .. }
            | Token::Comment { content, .. }
            | Token::Error { content, .. }
            | Token::Text { content, .. }
            | Token::Variable { content, .. } => content.text(db).clone(),
            Token::Whitespace { span, .. } => " ".repeat(span.length as usize),
            Token::Newline { span, .. } => {
                if span.length == 2 {
                    "\r\n".to_string()
                } else {
                    "\n".to_string()
                }
            }
            Token::Eof => String::new(),
        }
    }

    /// Get the lexeme as it appears in source
    pub fn lexeme(&self, db: &'db dyn TemplateDb) -> String {
        match self {
            Token::Block { content, .. } => format!("{{% {} %}}", content.text(db)),
            Token::Variable { content, .. } => format!("{{{{ {} }}}}", content.text(db)),
            Token::Comment { content, .. } => format!("{{# {} #}}", content.text(db)),
            Token::Text { content, .. } | Token::Error { content, .. } => content.text(db).clone(),
            Token::Whitespace { span, .. } => " ".repeat(span.length as usize),
            Token::Newline { span, .. } => {
                if span.length == 2 {
                    "\r\n".to_string()
                } else {
                    "\n".to_string()
                }
            }
            Token::Eof => String::new(),
        }
    }

    pub fn offset(&self) -> Option<u32> {
        match self {
            Token::Block { span, .. }
            | Token::Comment { span, .. }
            | Token::Error { span, .. }
            | Token::Variable { span, .. } => Some(span.start.saturating_sub(DJANGO_DELIM_LEN)),
            Token::Text { span, .. }
            | Token::Whitespace { span, .. }
            | Token::Newline { span, .. } => Some(span.start),
            Token::Eof => None,
        }
    }

    /// Get the length of the token content
    pub fn length(&self, db: &'db dyn TemplateDb) -> u32 {
        let len = match self {
            Token::Block { content, .. }
            | Token::Comment { content, .. }
            | Token::Error { content, .. }
            | Token::Text { content, .. }
            | Token::Variable { content, .. } => content.text(db).len(),
            Token::Whitespace { span, .. } | Token::Newline { span, .. } => span.length as usize,
            Token::Eof => 0,
        };
        u32::try_from(len).expect("Token length should fit in u32")
    }

    pub fn full_span(&self) -> Option<Span> {
        match self {
            Token::Block { span, .. }
            | Token::Comment { span, .. }
            | Token::Variable { span, .. } => Some(expand_with_delimiters(
                *span,
                DJANGO_DELIM_LEN,
                DJANGO_DELIM_LEN,
            )),
            Token::Error { span, .. } => Some(expand_with_delimiters(*span, DJANGO_DELIM_LEN, 0)),
            Token::Newline { span, .. }
            | Token::Text { span, .. }
            | Token::Whitespace { span, .. } => Some(*span),
            Token::Eof => None,
        }
    }

    pub fn content_span(&self) -> Option<Span> {
        match self {
            Token::Block { span, .. }
            | Token::Comment { span, .. }
            | Token::Error { span, .. }
            | Token::Text { span, .. }
            | Token::Variable { span, .. }
            | Token::Whitespace { span, .. }
            | Token::Newline { span, .. } => Some(*span),
            Token::Eof => None,
        }
    }
}

fn expand_with_delimiters(span: Span, opening: u32, closing: u32) -> Span {
    let start = span.start.saturating_sub(opening);
    Span {
        start,
        length: opening + span.length + closing,
    }
}

#[cfg(test)]
#[derive(Debug, serde::Serialize)]
pub enum TokenSnapshot {
    Block {
        content: String,
        span: (u32, u32),
        full_span: (u32, u32),
    },
    Comment {
        content: String,
        span: (u32, u32),
        full_span: (u32, u32),
    },
    Eof,
    Error {
        content: String,
        span: (u32, u32),
        full_span: (u32, u32),
    },
    Newline {
        span: (u32, u32),
    },
    Text {
        content: String,
        span: (u32, u32),
        full_span: (u32, u32),
    },
    Variable {
        content: String,
        span: (u32, u32),
        full_span: (u32, u32),
    },
    Whitespace {
        span: (u32, u32),
    },
}

#[cfg(test)]
impl<'db> Token<'db> {
    pub fn to_snapshot(&self, db: &'db dyn TemplateDb) -> TokenSnapshot {
        let span_tuple = |span: Span| (span.start, span.length);
        match self {
            Token::Block { span, .. } => TokenSnapshot::Block {
                content: self.content(db),
                span: span_tuple(*span),
                full_span: span_tuple(self.full_span().unwrap()),
            },
            Token::Comment { span, .. } => TokenSnapshot::Comment {
                content: self.content(db),
                span: span_tuple(*span),
                full_span: span_tuple(self.full_span().unwrap()),
            },
            Token::Eof => TokenSnapshot::Eof,
            Token::Error { span, .. } => TokenSnapshot::Error {
                content: self.content(db),
                span: span_tuple(*span),
                full_span: span_tuple(self.full_span().unwrap()),
            },
            Token::Newline { span } => TokenSnapshot::Newline {
                span: span_tuple(*span),
            },
            Token::Text { span, .. } => TokenSnapshot::Text {
                content: self.content(db),
                span: span_tuple(*span),
                full_span: span_tuple(*span),
            },
            Token::Variable { span, .. } => TokenSnapshot::Variable {
                content: self.content(db),
                span: span_tuple(*span),
                full_span: span_tuple(self.full_span().unwrap()),
            },
            Token::Whitespace { span } => TokenSnapshot::Whitespace {
                span: span_tuple(*span),
            },
        }
    }
}

#[cfg(test)]
pub struct TokenSnapshotVec<'db>(pub Vec<Token<'db>>);

#[cfg(test)]
impl TokenSnapshotVec<'_> {
    pub fn to_snapshot(&self, db: &dyn TemplateDb) -> Vec<TokenSnapshot> {
        self.0.iter().map(|t| t.to_snapshot(db)).collect()
    }
}

#[salsa::tracked]
pub struct TokenStream<'db> {
    #[tracked]
    #[returns(ref)]
    pub stream: Vec<Token<'db>>,
}

impl<'db> TokenStream<'db> {
    /// Check if the token stream is empty
    pub fn is_empty(self, db: &'db dyn TemplateDb) -> bool {
        self.stream(db).is_empty()
    }

    /// Get the number of tokens
    pub fn len(self, db: &'db dyn TemplateDb) -> usize {
        self.stream(db).len()
    }
}

pub fn span_from_token(token: &Token<'_>, db: &dyn TemplateDb) -> Span {
    token
        .content_span()
        .unwrap_or_else(|| Span::new(token.offset().unwrap_or(0), token.length(db)))
}
