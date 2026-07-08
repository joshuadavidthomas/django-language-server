use crate::File;
use crate::Span;

/// A source file location where a derived fact originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Origin {
    pub file: File,
    pub span: Span,
}

impl Origin {
    #[must_use]
    pub fn new(file: File, span: Span) -> Self {
        Self { file, span }
    }
}
