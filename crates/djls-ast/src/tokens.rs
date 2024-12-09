use serde::Serialize;
use std::fmt;
use std::ops::{Deref, DerefMut};

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
            TokenType::Comment(content, start, end) => {
                Some(content.len() + start.len() + end.as_ref().map_or(0, |e| e.len()))
            }
            TokenType::Whitespace(len) => Some(len.clone()),
            TokenType::Newline => Some(1),
            TokenType::Eof => None,
        }
    }

    pub fn lexeme(&self) -> &str {
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
            | TokenType::Text(s) => s,
            TokenType::Comment(content, _, _) => content, // Just return the content
            TokenType::Whitespace(_) => " ",
            TokenType::Newline => "\n",
            TokenType::Eof => "",
        }
    }
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use TokenType::*;

        match self {
            Comment(content, start, end) => match end {
                Some(end) => write!(f, "{}{}{}", start, content, end),
                None => write!(f, "{}{}", start, content),
            },
            DjangoBlock(s) => write!(f, "{{% {} %}}", s),
            DjangoVariable(s) => write!(f, "{{{{ {} }}}}", s),
            Eof => Ok(()),
            HtmlTagOpen(s) => write!(f, "<{}>", s),
            HtmlTagClose(s) => write!(f, "</{}>", s),
            HtmlTagVoid(s) => write!(f, "<{}/>", s),
            Newline => f.write_str("\n"),
            ScriptTagOpen(s) => write!(f, "<script{}>", s),
            ScriptTagClose(_) => f.write_str("</script>"),
            StyleTagOpen(s) => write!(f, "<style{}>", s),
            StyleTagClose(_) => f.write_str("</style>"),
            Text(s) => f.write_str(s),
            Whitespace(len) => f.write_str(&" ".repeat(*len)),
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

    pub fn lexeme_from_source<'a>(&self, source: &'a str) -> Option<&'a str> {
        match (self.start, self.token_type.len()) {
            (Some(start), Some(len)) => Some(&source[start..start + len]),
            _ => None,
        }
    }

    pub fn lexeme(&self) -> &str {
        self.token_type.lexeme()
    }

    pub fn token_type(&self) -> &TokenType {
        &self.token_type
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
