use djls_source::Span;
use memchr::memmem;

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

    pub(crate) fn find_closer(self, source: &str) -> Option<usize> {
        memmem::find(source.as_bytes(), self.closer().as_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token<T = String> {
    Block {
        content: T,
        span: Span,
    },
    Comment {
        content: T,
        span: Span,
    },
    Error {
        content: T,
        span: Span,
        delimiter: TagDelimiter,
    },
    Eof,
    Newline {
        span: Span,
    },
    Text {
        content: T,
        span: Span,
    },
    Variable {
        content: T,
        span: Span,
    },
    Whitespace {
        span: Span,
    },
}

impl<T: AsRef<str>> Token<T> {
    /// Get the content text for content-bearing tokens
    #[must_use]
    pub(crate) fn content(&self) -> String {
        match self {
            Token::Block { content, .. }
            | Token::Comment { content, .. }
            | Token::Error { content, .. }
            | Token::Text { content, .. }
            | Token::Variable { content, .. } => content.as_ref().to_string(),
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
            | Token::Variable { content, .. } => content.as_ref().len(),
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

impl Token<&str> {
    pub(crate) fn into_owned(self) -> Token {
        match self {
            Self::Block { content, span } => Token::Block {
                content: content.to_string(),
                span,
            },
            Self::Comment { content, span } => Token::Comment {
                content: content.to_string(),
                span,
            },
            Self::Error {
                content,
                span,
                delimiter,
            } => Token::Error {
                content: content.to_string(),
                span,
                delimiter,
            },
            Self::Text { content, span } => Token::Text {
                content: content.to_string(),
                span,
            },
            Self::Variable { content, span } => Token::Variable {
                content: content.to_string(),
                span,
            },
            Self::Eof => Token::Eof,
            Self::Newline { span } => Token::Newline { span },
            Self::Whitespace { span } => Token::Whitespace { span },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TokenStream<'src>(Vec<Token<&'src str>>);

impl<'src> TokenStream<'src> {
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
    pub(crate) fn push(&mut self, token: Token<&'src str>) {
        self.0.push(token);
    }
}

impl<'src> From<TokenStream<'src>> for Vec<Token<&'src str>> {
    fn from(val: TokenStream<'src>) -> Self {
        val.0
    }
}

impl<'src> IntoIterator for TokenStream<'src> {
    type Item = Token<&'src str>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
