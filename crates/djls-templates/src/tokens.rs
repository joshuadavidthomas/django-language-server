use djls_source::Span;

use crate::db::Db as TemplateDb;
use crate::nodelist::LineOffsets;

#[derive(Clone, Debug, PartialEq, Hash, salsa::Update)]
pub enum Token<'db> {
    Block {
        content: TokenContent<'db>,
        offset: usize,
    },
    Comment {
        content: TokenContent<'db>,
        offset: usize,
    },
    Error {
        content: TokenContent<'db>,
        offset: usize,
    },
    Eof,
    Newline {
        offset: usize,
    },
    Text {
        content: TokenContent<'db>,
        offset: usize,
    },
    Variable {
        content: TokenContent<'db>,
        offset: usize,
    },
    Whitespace {
        count: usize,
        offset: usize,
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
            Token::Whitespace { count, .. } => " ".repeat(*count),
            Token::Newline { .. } => "\n".to_string(),
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
            Token::Whitespace { count, .. } => " ".repeat(*count),
            Token::Newline { .. } => "\n".to_string(),
            Token::Eof { .. } => String::new(),
        }
    }

    pub fn offset(&self) -> Option<u32> {
        match self {
            Token::Block { offset, .. }
            | Token::Comment { offset, .. }
            | Token::Error { offset, .. }
            | Token::Newline { offset, .. }
            | Token::Text { offset, .. }
            | Token::Variable { offset, .. }
            | Token::Whitespace { offset, .. } => {
                Some(u32::try_from(*offset).expect("Offset should fit in u33"))
            }
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
            Token::Whitespace { count, .. } => *count,
            Token::Newline { .. } => 1,
            Token::Eof { .. } => 0,
        };
        u32::try_from(len).expect("Token length should fit in u32")
    }
}

#[cfg(test)]
#[derive(Debug, serde::Serialize)]
pub enum TokenSnapshot {
    Block { content: String, offset: usize },
    Comment { content: String, offset: usize },
    Eof,
    Error { content: String, offset: usize },
    Newline { offset: usize },
    Text { content: String, offset: usize },
    Variable { content: String, offset: usize },
    Whitespace { count: usize, offset: usize },
}

#[cfg(test)]
impl<'db> Token<'db> {
    pub fn to_snapshot(&self, db: &'db dyn TemplateDb) -> TokenSnapshot {
        match self {
            Token::Block { content, offset } => TokenSnapshot::Block {
                content: content.text(db).to_string(),
                offset: *offset,
            },
            Token::Comment { content, offset } => TokenSnapshot::Comment {
                content: content.text(db).to_string(),
                offset: *offset,
            },
            Token::Eof => TokenSnapshot::Eof,
            Token::Error { content, offset } => TokenSnapshot::Error {
                content: content.text(db).to_string(),
                offset: *offset,
            },
            Token::Newline { offset } => TokenSnapshot::Newline { offset: *offset },
            Token::Text { content, offset } => TokenSnapshot::Text {
                content: content.text(db).to_string(),
                offset: *offset,
            },
            Token::Variable { content, offset } => TokenSnapshot::Variable {
                content: content.text(db).to_string(),
                offset: *offset,
            },
            Token::Whitespace { count, offset } => TokenSnapshot::Whitespace {
                count: *count,
                offset: *offset,
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
    #[tracked]
    #[returns(ref)]
    pub line_offsets: LineOffsets,
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
    let start = token.offset().unwrap_or(0);
    let length = token.length(db);
    Span::new(start, length)
}
