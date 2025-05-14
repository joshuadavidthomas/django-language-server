use std::ops::Deref;
use std::ops::DerefMut;

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum TokenType {
    Comment(String, String, Option<String>),
    DjangoBlock(String),
    DjangoVariable(String),
    Eof,
    HtmlTagOpen(String),
    HtmlTagClose(String),
    HtmlTagVoid(String),
    Newline,
    ScriptTagOpen(String),
    ScriptTagClose(String),
    StyleTagOpen(String),
    StyleTagClose(String),
    Text(String),
    Whitespace(usize),
}

impl TokenType {
    pub fn len(&self) -> Option<usize> {
        match self {
            TokenType::DjangoBlock(s)
            | TokenType::DjangoVariable(s)
            | TokenType::HtmlTagOpen(s)
            | TokenType::HtmlTagClose(s)
            | TokenType::HtmlTagVoid(s)
            | TokenType::ScriptTagOpen(s)
            | TokenType::ScriptTagClose(s)
            | TokenType::StyleTagOpen(s)
            | TokenType::StyleTagClose(s)
            | TokenType::Text(s) => Some(s.len()),
            TokenType::Comment(content, _, _) => Some(content.len()),
            TokenType::Whitespace(n) => Some(*n),
            TokenType::Newline => Some(1),
            TokenType::Eof => Some(0),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Token {
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
            TokenType::HtmlTagOpen(_)
            | TokenType::ScriptTagOpen(_)
            | TokenType::StyleTagOpen(_) => format!("<{}>", self.content()),
            TokenType::HtmlTagClose(_)
            | TokenType::StyleTagClose(_)
            | TokenType::ScriptTagClose(_) => format!("</{}>", self.content()),
            TokenType::HtmlTagVoid(_) => format!("<{}/>", self.content()),
            TokenType::Newline | TokenType::Text(_) | TokenType::Whitespace(_) => self.content(),
        }
    }

    pub fn content(&self) -> String {
        match &self.token_type {
            TokenType::Comment(s, _, _)
            | TokenType::DjangoBlock(s)
            | TokenType::DjangoVariable(s)
            | TokenType::Text(s)
            | TokenType::HtmlTagOpen(s)
            | TokenType::HtmlTagClose(s)
            | TokenType::HtmlTagVoid(s)
            | TokenType::ScriptTagOpen(s)
            | TokenType::ScriptTagClose(s)
            | TokenType::StyleTagOpen(s)
            | TokenType::StyleTagClose(s) => s.to_string(),
            TokenType::Whitespace(len) => " ".repeat(*len),
            TokenType::Newline => "\n".to_string(),
            TokenType::Eof => "".to_string(),
        }
    }

    pub fn token_type(&self) -> &TokenType {
        &self.token_type
    }

    pub fn line(&self) -> &usize {
        &self.line
    }

    pub fn start(&self) -> Option<u32> {
        self.start.map(|s| s as u32)
    }

    pub fn length(&self) -> Option<u32> {
        self.token_type.len().map(|l| l as u32)
    }

    pub fn is_token_type(&self, token_type: &TokenType) -> bool {
        &self.token_type == token_type
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TokenStream(Vec<Token>);

impl TokenStream {
    pub fn tokens(&self) -> &Vec<Token> {
        &self.0
    }

    pub fn add_token(&mut self, token: Token) {
        self.0.push(token);
    }

    pub fn finalize(&mut self, line: usize) -> TokenStream {
        let eof_token = Token {
            token_type: TokenType::Eof,
            line,
            start: None,
        };
        self.add_token(eof_token);
        self.clone()
    }
}

impl AsRef<[Token]> for TokenStream {
    fn as_ref(&self) -> &[Token] {
        &self.0
    }
}

impl Deref for TokenStream {
    type Target = Vec<Token>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TokenStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl IntoIterator for TokenStream {
    type Item = Token;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a TokenStream {
    type Item = &'a Token;
    type IntoIter = std::slice::Iter<'a, Token>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
