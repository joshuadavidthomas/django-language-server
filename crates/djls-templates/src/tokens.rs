use djls_source::Span;

use crate::db::Db as TemplateDb;

#[derive(Clone, Debug, PartialEq, Hash, salsa::Update)]
pub enum Token<'db> {
    Block {
        content: TokenContent<'db>,
        spans: TokenSpans,
    },
    Comment {
        content: TokenContent<'db>,
        spans: TokenSpans,
    },
    Error {
        content: TokenContent<'db>,
        spans: TokenSpans,
    },
    Eof,
    Newline {
        spans: TokenSpans,
    },
    Text {
        content: TokenContent<'db>,
        spans: TokenSpans,
    },
    Variable {
        content: TokenContent<'db>,
        spans: TokenSpans,
    },
    Whitespace {
        spans: TokenSpans,
    },
}

#[salsa::interned(debug)]
pub struct TokenContent<'db> {
    #[returns(ref)]
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TokenSpans {
    pub content: Span,
    pub lexeme: Span,
}

impl TokenSpans {
    #[must_use]
    pub fn new(content: Span, lexeme: Span) -> Self {
        Self { content, lexeme }
    }

    #[must_use]
    pub fn content_tuple(&self) -> (u32, u32) {
        (self.content.start, self.content.length)
    }

    #[must_use]
    pub fn lexeme_tuple(&self) -> (u32, u32) {
        (self.lexeme.start, self.lexeme.length)
    }
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
            Token::Whitespace { spans, .. } => " ".repeat(spans.lexeme.length as usize),
            Token::Newline { spans, .. } => {
                if spans.lexeme.length == 2 {
                    "\r\n".to_string()
                } else {
                    "\n".to_string()
                }
            }
            Token::Eof { .. } => String::new(),
        }
    }

    /// Get the lexeme as it appears in source
    pub fn lexeme(&self, db: &'db dyn TemplateDb) -> String {
        match self {
            Token::Block { content, .. } => format!("{{% {} %}}", content.text(db)),
            Token::Variable { content, .. } => format!("{{{{ {} }}}}", content.text(db)),
            Token::Comment { content, .. } => format!("{{# {} #}}", content.text(db)),
            Token::Text { content, .. } | Token::Error { content, .. } => content.text(db).clone(),
            Token::Whitespace { spans, .. } => " ".repeat(spans.lexeme.length as usize),
            Token::Newline { spans, .. } => {
                if spans.lexeme.length == 2 {
                    "\r\n".to_string()
                } else {
                    "\n".to_string()
                }
            }
            Token::Eof { .. } => String::new(),
        }
    }

    pub fn offset(&self) -> Option<u32> {
        match self {
            Token::Block { spans, .. }
            | Token::Comment { spans, .. }
            | Token::Error { spans, .. }
            | Token::Newline { spans, .. }
            | Token::Text { spans, .. }
            | Token::Variable { spans, .. }
            | Token::Whitespace { spans, .. } => Some(spans.lexeme.start),
            Token::Eof { .. } => None,
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
            Token::Whitespace { spans, .. } | Token::Newline { spans, .. } => {
                spans.lexeme.length as usize
            }
            Token::Eof { .. } => 0,
        };
        u32::try_from(len).expect("Token length should fit in u32")
    }

    pub fn full_span(&self) -> Option<Span> {
        match self {
            Token::Block { spans, .. }
            | Token::Comment { spans, .. }
            | Token::Error { spans, .. }
            | Token::Newline { spans, .. }
            | Token::Text { spans, .. }
            | Token::Variable { spans, .. }
            | Token::Whitespace { spans, .. } => Some(spans.lexeme),
            Token::Eof { .. } => None,
        }
    }

    pub fn content_span(&self) -> Option<Span> {
        match self {
            Token::Block { spans, .. }
            | Token::Comment { spans, .. }
            | Token::Error { spans, .. }
            | Token::Text { spans, .. }
            | Token::Variable { spans, .. } => Some(spans.content),
            Token::Whitespace { spans, .. } | Token::Newline { spans, .. } => Some(spans.lexeme),
            Token::Eof { .. } => None,
        }
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
        match self {
            Token::Block { spans, .. } => TokenSnapshot::Block {
                content: self.content(db),
                span: spans.content_tuple(),
                full_span: spans.lexeme_tuple(),
            },
            Token::Comment { spans, .. } => TokenSnapshot::Comment {
                content: self.content(db),
                span: spans.content_tuple(),
                full_span: spans.lexeme_tuple(),
            },
            Token::Eof => TokenSnapshot::Eof,
            Token::Error { spans, .. } => TokenSnapshot::Error {
                content: self.content(db),
                span: spans.content_tuple(),
                full_span: spans.lexeme_tuple(),
            },
            Token::Newline { spans } => TokenSnapshot::Newline {
                span: spans.lexeme_tuple(),
            },
            Token::Text { spans, .. } => TokenSnapshot::Text {
                content: self.content(db),
                span: spans.content_tuple(),
                full_span: spans.lexeme_tuple(),
            },
            Token::Variable { spans, .. } => TokenSnapshot::Variable {
                content: self.content(db),
                span: spans.content_tuple(),
                full_span: spans.lexeme_tuple(),
            },
            Token::Whitespace { spans } => TokenSnapshot::Whitespace {
                span: spans.lexeme_tuple(),
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
