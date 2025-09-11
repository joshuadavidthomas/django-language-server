use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub enum TokenType {
    // Django constructs
    DjangoBlock(String),     // {% ... %} content
    DjangoVariable(String),  // {{ ... }} content
    Comment(String, String, Option<String>), // {# ... #} content

    // Text and whitespace
    Text(String),       // All non-Django content (HTML, JS, CSS, plain text)
    Whitespace(usize),  // Non-newline whitespace (for formatting)
    Newline,           // Essential for line tracking!

    // Error recovery
    Error(String),      // Malformed Django constructs only

    // End
    Eof,
}

impl TokenType {
    pub fn len(&self) -> usize {
        match self {
            TokenType::DjangoBlock(s)
            | TokenType::DjangoVariable(s)
            | TokenType::Error(s)
            | TokenType::Text(s) => s.len(),
            TokenType::Comment(content, _, _) => content.len(),
            TokenType::Whitespace(n) => *n,
            TokenType::Newline => 1,
            TokenType::Eof => 0,
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
            TokenType::Comment(_, start, end) => match end {
                Some(end) => format!("{} {} {}", start, self.content(), end),
                None => format!("{} {}", start, self.content()),
            },
            TokenType::DjangoBlock(_) => format!("{{% {} %}}", self.content()),
            TokenType::DjangoVariable(_) => format!("{{{{ {} }}}}", self.content()),
            TokenType::Eof => String::new(),
            TokenType::Error(_) => self.content(), // Return the raw malformed content
            TokenType::Newline | TokenType::Text(_) | TokenType::Whitespace(_) => self.content(),
        }
    }

    pub fn content(&self) -> String {
        match &self.token_type {
            TokenType::Comment(s, _, _)
            | TokenType::DjangoBlock(s)
            | TokenType::DjangoVariable(s)
            | TokenType::Error(s)
            | TokenType::Text(s) => s.to_string(),
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
    pub fn is_empty(self, db: &'db dyn crate::db::Db) -> bool {
        self.stream(db).is_empty()
    }

    /// Get the number of tokens
    pub fn len(self, db: &'db dyn crate::db::Db) -> usize {
        self.stream(db).len()
    }
}
