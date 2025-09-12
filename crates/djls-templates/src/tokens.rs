use serde::Serialize;

use crate::db::Db as TemplateDb;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum TokenType {
    Block(String),
    Comment(String),
    Error(String),
    Eof,
    Newline,
    Text(String),
    Variable(String),
    Whitespace(usize),
}

impl TokenType {
    pub fn len(&self) -> usize {
        match self {
            TokenType::Block(s)
            | TokenType::Comment(s)
            | TokenType::Error(s)
            | TokenType::Text(s)
            | TokenType::Variable(s) => s.len(),
            TokenType::Eof => 0,
            TokenType::Newline => 1,
            TokenType::Whitespace(n) => *n,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Token {
    #[allow(clippy::struct_field_names)]
    token_type: TokenType,
    line: usize,
    start: Option<usize>,
}

impl Token {
    pub fn new(token_type: TokenType, line: usize, start: Option<usize>) -> Self {
        Self {
            token_type,
            line,
            start,
        }
    }

    pub fn lexeme(&self) -> String {
        match &self.token_type {
            TokenType::Block(_) => format!("{{% {} %}}", self.content()),
            TokenType::Comment(_)
            | TokenType::Error(_)
            | TokenType::Newline
            | TokenType::Text(_)
            | TokenType::Whitespace(_) => self.content(),
            TokenType::Eof => String::new(),
            TokenType::Variable(_) => format!("{{{{ {} }}}}", self.content()),
        }
    }

    pub fn content(&self) -> String {
        match &self.token_type {
            TokenType::Block(s)
            | TokenType::Comment(s)
            | TokenType::Error(s)
            | TokenType::Text(s)
            | TokenType::Variable(s) => s.to_string(),
            TokenType::Whitespace(len) => " ".repeat(*len),
            TokenType::Newline => "\n".to_string(),
            TokenType::Eof => String::new(),
        }
    }

    pub fn token_type(&self) -> &TokenType {
        &self.token_type
    }

    pub fn line(&self) -> &usize {
        &self.line
    }

    pub fn start(&self) -> Option<u32> {
        self.start
            .map(|s| u32::try_from(s).expect("Start position should fit in u32"))
    }

    pub fn length(&self) -> u32 {
        u32::try_from(self.token_type.len()).expect("Token length should fit in u32")
    }

    pub fn is_token_type(&self, token_type: &TokenType) -> bool {
        &self.token_type == token_type
    }
}

#[salsa::tracked]
pub struct TokenStream<'db> {
    #[tracked]
    #[returns(ref)]
    pub stream: Vec<Token>,
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
