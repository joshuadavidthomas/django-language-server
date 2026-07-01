use djls_source::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagDelimiter {
    Block,
    Variable,
    Comment,
}

impl TagDelimiter {
    pub(crate) const CHAR_OPEN: char = '{';
    pub(crate) const LENGTH: usize = 2;
    pub const LENGTH_U32: u32 = 2;

    #[must_use]
    pub(crate) fn from_input(input: &str) -> Option<Self> {
        let bytes = input.as_bytes();

        if bytes.len() < Self::LENGTH {
            return None;
        }

        if bytes[0] != Self::CHAR_OPEN as u8 {
            return None;
        }

        match bytes[1] {
            b'%' => Some(Self::Block),
            b'{' => Some(Self::Variable),
            b'#' => Some(Self::Comment),
            _ => None,
        }
    }

    #[must_use]
    pub(crate) fn opener(self) -> &'static str {
        match self {
            Self::Block => "{%",
            Self::Variable => "{{",
            Self::Comment => "{#",
        }
    }

    #[must_use]
    pub(crate) fn closer(self) -> &'static str {
        match self {
            Self::Block => "%}",
            Self::Variable => "}}",
            Self::Comment => "#}",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Block {
        content: String,
        span: Span,
    },
    Comment {
        content: String,
        span: Span,
    },
    Error {
        content: String,
        span: Span,
        delimiter: TagDelimiter,
    },
    Eof,
    Newline {
        span: Span,
    },
    Text {
        content: String,
        span: Span,
    },
    Variable {
        content: String,
        span: Span,
    },
    Whitespace {
        span: Span,
    },
}

impl Token {
    /// Get the content text for content-bearing tokens
    #[must_use]
    pub(crate) fn content(&self) -> String {
        match self {
            Token::Block { content, .. }
            | Token::Comment { content, .. }
            | Token::Error { content, .. }
            | Token::Text { content, .. }
            | Token::Variable { content, .. } => content.clone(),
            Token::Whitespace { span, .. } => " ".repeat(span.length_usize()),
            Token::Newline { span, .. } => {
                if span.length() == 2 {
                    "\r\n".to_string()
                } else {
                    "\n".to_string()
                }
            }
            Token::Eof => String::new(),
        }
    }

    #[must_use]
    fn offset(&self) -> Option<u32> {
        match self {
            Token::Block { span, .. }
            | Token::Comment { span, .. }
            | Token::Error { span, .. }
            | Token::Variable { span, .. } => {
                Some(span.start().saturating_sub(TagDelimiter::LENGTH_U32))
            }
            Token::Text { span, .. }
            | Token::Whitespace { span, .. }
            | Token::Newline { span, .. } => Some(span.start()),
            Token::Eof => None,
        }
    }

    /// Get the length of the token content
    #[must_use]
    fn length(&self) -> u32 {
        let len = match self {
            Token::Block { content, .. }
            | Token::Comment { content, .. }
            | Token::Error { content, .. }
            | Token::Text { content, .. }
            | Token::Variable { content, .. } => content.len(),
            Token::Whitespace { span, .. } | Token::Newline { span, .. } => span.length_usize(),
            Token::Eof => 0,
        };
        u32::try_from(len).unwrap_or(u32::MAX)
    }

    #[must_use]
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

    #[must_use]
    fn content_span(&self) -> Option<Span> {
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

    #[must_use]
    pub(crate) fn full_span_or_fallback(&self) -> Span {
        self.full_span()
            .unwrap_or_else(|| self.content_span_or_fallback())
    }

    #[must_use]
    pub(crate) fn content_span_or_fallback(&self) -> Span {
        self.content_span()
            .unwrap_or_else(|| Span::new(self.offset().unwrap_or(0), self.length()))
    }

    #[must_use]
    pub(crate) fn spans(&self) -> (Span, Span) {
        let content = self.content_span_or_fallback();
        let full = self.full_span().unwrap_or(content);
        (content, full)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TokenStream(Vec<Token>);

impl TokenStream {
    const CHARS_PER_TOKEN: usize = 6;
    const MIN_CAPACITY: usize = 32;
    const MAX_CAPACITY: usize = 1024;

    #[must_use]
    pub(crate) fn with_estimated_capacity(source: &str) -> Self {
        let capacity =
            (source.len() / Self::CHARS_PER_TOKEN).clamp(Self::MIN_CAPACITY, Self::MAX_CAPACITY);
        Self(Vec::with_capacity(capacity))
    }

    #[inline]
    pub(crate) fn push(&mut self, token: Token) {
        self.0.push(token);
    }
}

impl From<TokenStream> for Vec<Token> {
    fn from(val: TokenStream) -> Self {
        val.0
    }
}

impl IntoIterator for TokenStream {
    type Item = Token;
    type IntoIter = std::vec::IntoIter<Token>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
