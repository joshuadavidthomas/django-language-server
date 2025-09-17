use djls_source::Span;

use crate::db::Db as TemplateDb;
use crate::tokens::Token;
use crate::tokens::TokenContent;

const BLOCK_TAG_START: &str = "{%";
const BLOCK_TAG_END: &str = "%}";
const VARIABLE_TAG_START: &str = "{{";
const VARIABLE_TAG_END: &str = "}}";
const COMMENT_TAG_START: &str = "{#";
const COMMENT_TAG_END: &str = "#}";

pub struct Lexer<'db> {
    db: &'db dyn TemplateDb,
    source: String,
    start: usize,
    current: usize,
}

impl<'db> Lexer<'db> {
    #[must_use]
    pub fn new(db: &'db dyn TemplateDb, source: &str) -> Self {
        Lexer {
            db,
            source: String::from(source),
            start: 0,
            current: 0,
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token<'db>> {
        let mut tokens = Vec::new();

        while !self.is_at_end() {
            self.start = self.current;

            let token = match self.peek() {
                '{' => match self.peek_next() {
                    '%' => self.lex_django_construct(BLOCK_TAG_END, |content, span| Token::Block {
                        content,
                        span,
                    }),
                    '{' => self.lex_django_construct(VARIABLE_TAG_END, |content, span| {
                        Token::Variable { content, span }
                    }),
                    '#' => self.lex_django_construct(COMMENT_TAG_END, |content, span| {
                        Token::Comment { content, span }
                    }),
                    _ => self.lex_text(),
                },
                c if c.is_whitespace() => self.lex_whitespace(c),
                _ => self.lex_text(),
            };

            tokens.push(token);
        }

        tokens.push(Token::Eof);

        tokens
    }

    fn lex_django_construct(
        &mut self,
        end: &str,
        token_fn: impl FnOnce(TokenContent<'db>, Span) -> Token<'db>,
    ) -> Token<'db> {
        let opening_len = 2;
        let content_start = self.start + opening_len;

        self.consume_n(opening_len);

        match self.consume_until(end) {
            Ok(text) => {
                let content = TokenContent::new(self.db, text);
                let content_end = self.current;
                let span = Span::from_bounds(content_start, content_end);
                self.consume_n(end.len());
                token_fn(content, span)
            }
            Err(err_text) => {
                let content_end = self.current;
                let span = Span::from_bounds(content_start, content_end);
                let content = TokenContent::new(self.db, err_text);
                Token::Error { content, span }
            }
        }
    }

    fn lex_whitespace(&mut self, c: char) -> Token<'db> {
        if c == '\n' || c == '\r' {
            self.consume(); // \r or \n
            if c == '\r' && self.peek() == '\n' {
                self.consume(); // \n of \r\n
            }
            let span = Span::from_bounds(self.start, self.current);
            Token::Newline { span }
        } else {
            self.consume(); // Consume the first whitespace
            while !self.is_at_end() && self.peek().is_whitespace() {
                if self.peek() == '\n' || self.peek() == '\r' {
                    break;
                }
                self.consume();
            }
            let span = Span::from_bounds(self.start, self.current);
            Token::Whitespace { span }
        }
    }

    fn lex_text(&mut self) -> Token<'db> {
        let text_start = self.current;

        while !self.is_at_end() {
            if self.source[self.current..].starts_with(BLOCK_TAG_START)
                || self.source[self.current..].starts_with(VARIABLE_TAG_START)
                || self.source[self.current..].starts_with(COMMENT_TAG_START)
                || self.source[self.current..].starts_with('\n')
            {
                break;
            }
            self.consume();
        }

        let text = &self.source[text_start..self.current];
        let content = TokenContent::new(self.db, text.to_string());
        let span = Span::from_bounds(self.start, self.current);
        Token::Text { content, span }
    }

    #[inline]
    fn peek(&self) -> char {
        self.source[self.current..].chars().next().unwrap_or('\0')
    }

    fn peek_next(&self) -> char {
        let mut chars = self.source[self.current..].chars();
        chars.next(); // Skip current
        chars.next().unwrap_or('\0')
    }

    #[inline]
    fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }

    #[inline]
    fn consume(&mut self) {
        if let Some(ch) = self.source[self.current..].chars().next() {
            self.current += ch.len_utf8();
        }
    }

    fn consume_n(&mut self, count: usize) {
        for _ in 0..count {
            self.consume();
        }
    }

    fn consume_until(&mut self, delimiter: &str) -> Result<String, String> {
        let offset = self.current;
        let mut fallback: Option<usize> = None;

        while self.current < self.source.len() {
            if self.source[self.current..].starts_with(delimiter) {
                return Ok(self.source[offset..self.current].to_string());
            }

            if fallback.is_none()
                && (self.source[self.current..].starts_with(BLOCK_TAG_START)
                    || self.source[self.current..].starts_with(VARIABLE_TAG_START)
                    || self.source[self.current..].starts_with(COMMENT_TAG_START))
            {
                fallback = Some(self.current);
            }

            let ch = self.peek();
            if fallback.is_none() && matches!(ch, '\n' | '\r') {
                fallback = Some(self.current);
            }

            self.consume();
        }

        let end = fallback.unwrap_or(self.current);
        let text = self.source[offset..end].to_string();
        self.current = end;
        Err(text)
    }
}
