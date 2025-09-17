use djls_source::Span;

use crate::db::Db as TemplateDb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagDelimiter {
    Block,
    Variable,
    Comment,
}

impl TagDelimiter {
    pub const CHAR_OPEN: char = '{';
    pub const LENGTH: usize = 2;
    pub const LENGTH_U32: u32 = 2;

    #[must_use]
    pub fn from_input(input: &str) -> Option<TagDelimiter> {
        [Self::Block, Self::Variable, Self::Comment]
            .into_iter()
            .find(|kind| input.starts_with(kind.opener()))
    }

    #[must_use]
    pub fn opener(self) -> &'static str {
        match self {
            TagDelimiter::Block => "{%",
            TagDelimiter::Variable => "{{",
            TagDelimiter::Comment => "{#",
        }
    }

    #[must_use]
    pub fn closer(self) -> &'static str {
        match self {
            TagDelimiter::Block => "%}",
            TagDelimiter::Variable => "}}",
            TagDelimiter::Comment => "#}",
        }
    }
}

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
            Token::Block { content, .. } => format!(
                "{} {} {}",
                TagDelimiter::Block.opener(),
                content.text(db),
                TagDelimiter::Block.closer()
            ),
            Token::Variable { content, .. } => format!(
                "{} {} {}",
                TagDelimiter::Variable.opener(),
                content.text(db),
                TagDelimiter::Variable.closer()
            ),
            Token::Comment { content, .. } => format!(
                "{} {} {}",
                TagDelimiter::Comment.opener(),
                content.text(db),
                TagDelimiter::Comment.closer()
            ),
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
            | Token::Variable { span, .. } => {
                Some(span.start.saturating_sub(TagDelimiter::LENGTH_U32))
            }
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
            | Token::Variable { span, .. } => {
                Some(span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32))
            }
            Token::Error { span, .. } => Some(span.expand(TagDelimiter::LENGTH_U32, 0)),
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

    pub fn content_span_or_fallback(&self, db: &dyn TemplateDb) -> Span {
        self.content_span()
            .unwrap_or_else(|| Span::new(self.offset().unwrap_or(0), self.length(db)))
    }

    pub fn spans(&self, db: &'db dyn TemplateDb) -> (Span, Span) {
        let content = self.content_span_or_fallback(db);
        let full = self.full_span().unwrap_or(content);
        (content, full)
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
            Token::Block { span, .. } => TokenSnapshot::Block {
                content: self.content(db),
                span: span.as_tuple(),
                full_span: self.full_span().unwrap().as_tuple(),
            },
            Token::Comment { span, .. } => TokenSnapshot::Comment {
                content: self.content(db),
                span: span.as_tuple(),
                full_span: self.full_span().unwrap().as_tuple(),
            },
            Token::Eof => TokenSnapshot::Eof,
            Token::Error { span, .. } => TokenSnapshot::Error {
                content: self.content(db),
                span: span.as_tuple(),
                full_span: self.full_span().unwrap().as_tuple(),
            },
            Token::Newline { span } => TokenSnapshot::Newline {
                span: span.as_tuple(),
            },
            Token::Text { span, .. } => TokenSnapshot::Text {
                content: self.content(db),
                span: span.as_tuple(),
                full_span: span.as_tuple(),
            },
            Token::Variable { span, .. } => TokenSnapshot::Variable {
                content: self.content(db),
                span: span.as_tuple(),
                full_span: self.full_span().unwrap().as_tuple(),
            },
            Token::Whitespace { span } => TokenSnapshot::Whitespace {
                span: span.as_tuple(),
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
