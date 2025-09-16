use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Span {
    pub start: u32,
    pub length: u32,
}

impl Span {
    #[must_use]
    pub fn new(start: u32, length: u32) -> Self {
        Self { start, length }
    }
}
