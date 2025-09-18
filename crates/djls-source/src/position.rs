use std::ops::Deref;
use std::ops::DerefMut;

use serde::Serialize;

use crate::LineIndex;

/// A byte offset within a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ByteOffset(u32);

impl ByteOffset {
    #[must_use]
    pub fn new(offset: u32) -> Self {
        Self(offset)
    }

    #[must_use]
    pub fn from_usize(offset: usize) -> Self {
        Self(u32::try_from(offset).unwrap_or(u32::MAX))
    }

    #[must_use]
    pub fn offset(&self) -> u32 {
        self.0
    }
}

impl Deref for ByteOffset {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ByteOffset {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// A line and column position within a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineCol {
    line: u32,
    column: u32,
}

impl LineCol {
    #[must_use]
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }

    #[must_use]
    pub fn line(&self) -> u32 {
        self.line
    }

    #[must_use]
    pub fn column(&self) -> u32 {
        self.column
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Span {
    start: u32,
    length: u32,
}

impl Span {
    #[must_use]
    pub fn new(start: u32, length: u32) -> Self {
        Self { start, length }
    }

    #[must_use]
    pub fn from_parts(start: usize, length: usize) -> Self {
        let start_u32 = u32::try_from(start).unwrap_or(u32::MAX);
        let length_u32 = u32::try_from(length).unwrap_or(u32::MAX.saturating_sub(start_u32));
        Span::new(start_u32, length_u32)
    }

    #[must_use]
    pub fn with_length_usize(self, length: usize) -> Self {
        Self::from_parts(self.start_usize(), length)
    }

    /// Construct a span from integer bounds expressed as byte offsets.
    #[must_use]
    pub fn from_bounds(start: usize, end: usize) -> Self {
        Self::from_parts(start, end.saturating_sub(start))
    }

    #[must_use]
    pub fn expand(self, opening: u32, closing: u32) -> Self {
        let start_expand = self.start.saturating_sub(opening);
        let length_expand = opening + self.length + closing;
        Self::new(start_expand, length_expand)
    }

    #[must_use]
    pub fn as_tuple(self) -> (u32, u32) {
        (self.start, self.length)
    }

    #[must_use]
    pub fn start(self) -> u32 {
        self.start
    }

    #[must_use]
    pub fn start_usize(self) -> usize {
        self.start as usize
    }

    #[must_use]
    pub fn end(self) -> u32 {
        self.start + self.length
    }

    #[must_use]
    pub fn length(self) -> u32 {
        self.length
    }

    #[must_use]
    pub fn length_usize(self) -> usize {
        self.length as usize
    }

    #[must_use]
    pub fn start_offset(&self) -> ByteOffset {
        ByteOffset(self.start)
    }

    #[must_use]
    pub fn end_offset(&self) -> ByteOffset {
        ByteOffset(self.start.saturating_add(self.length))
    }

    /// Convert this span to start and end line/column positions using the given line index.
    #[must_use]
    pub fn to_line_col(&self, line_index: &LineIndex) -> (LineCol, LineCol) {
        let start = line_index.to_line_col(self.start_offset());
        let end = line_index.to_line_col(self.end_offset());
        (start, end)
    }
}
