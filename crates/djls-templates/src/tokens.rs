use crate::db::Db as TemplateDb;

#[derive(Clone, Debug, PartialEq, Hash, salsa::Update)]
pub enum Token<'db> {
    Block {
        content: TokenContent<'db>,
        line: usize,
        start: usize,
    },
    Comment {
        content: TokenContent<'db>,
        line: usize,
        start: usize,
    },
    Error {
        content: TokenContent<'db>,
        line: usize,
        start: usize,
    },
    Eof {
        line: usize,
    },
    Newline {
        line: usize,
        start: usize,
    },
    Text {
        content: TokenContent<'db>,
        line: usize,
        start: usize,
    },
    Variable {
        content: TokenContent<'db>,
        line: usize,
        start: usize,
    },
    Whitespace {
        count: usize,
        line: usize,
        start: usize,
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

    pub fn start(&self) -> Option<u32> {
        match self {
            Token::Block { start, .. }
            | Token::Comment { start, .. }
            | Token::Error { start, .. }
            | Token::Newline { start, .. }
            | Token::Text { start, .. }
            | Token::Variable { start, .. }
            | Token::Whitespace { start, .. } => {
                Some(u32::try_from(*start).expect("Start position should fit in u32"))
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
    Block {
        content: String,
        line: usize,
        start: usize,
    },
    Comment {
        content: String,
        line: usize,
        start: usize,
    },
    Error {
        content: String,
        line: usize,
        start: usize,
    },
    Text {
        content: String,
        line: usize,
        start: usize,
    },
    Variable {
        content: String,
        line: usize,
        start: usize,
    },
    Whitespace {
        count: usize,
        line: usize,
        start: usize,
    },
    Newline {
        line: usize,
        start: usize,
    },
    Eof {
        line: usize,
    },
}

#[cfg(test)]
impl<'db> Token<'db> {
    pub fn to_snapshot(&self, db: &'db dyn TemplateDb) -> TokenSnapshot {
        match self {
            Token::Block {
                content,
                line,
                start,
            } => TokenSnapshot::Block {
                content: content.text(db).to_string(),
                line: *line,
                start: *start,
            },
            Token::Comment {
                content,
                line,
                start,
            } => TokenSnapshot::Comment {
                content: content.text(db).to_string(),
                line: *line,
                start: *start,
            },
            Token::Error {
                content,
                line,
                start,
            } => TokenSnapshot::Error {
                content: content.text(db).to_string(),
                line: *line,
                start: *start,
            },
            Token::Text {
                content,
                line,
                start,
            } => TokenSnapshot::Text {
                content: content.text(db).to_string(),
                line: *line,
                start: *start,
            },
            Token::Variable {
                content,
                line,
                start,
            } => TokenSnapshot::Variable {
                content: content.text(db).to_string(),
                line: *line,
                start: *start,
            },
            Token::Whitespace { count, line, start } => TokenSnapshot::Whitespace {
                count: *count,
                line: *line,
                start: *start,
            },
            Token::Newline { line, start } => TokenSnapshot::Newline {
                line: *line,
                start: *start,
            },
            Token::Eof { line } => TokenSnapshot::Eof { line: *line },
        }
    }
}

#[cfg(test)]
pub struct TokenSnapshotVec<'db>(pub Vec<Token<'db>>);

#[cfg(test)]
impl<'db> TokenSnapshotVec<'db> {
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
