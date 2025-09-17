use djls_source::Span;

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct SpanPair {
    pub content: Span,
    pub lexeme: Span,
}

impl SpanPair {
    #[must_use]
    pub fn new(content: Span, lexeme: Span) -> Self {
        Self { content, lexeme }
    }

    #[must_use]
    pub fn content_tuple(&self) -> (u32, u32) {
        (self.content.start, self.content.length)
    }

    #[must_use]
    pub fn lexeme_tuple(&self) -> (u32, u32) {
        (self.lexeme.start, self.lexeme.length)
    }
}
