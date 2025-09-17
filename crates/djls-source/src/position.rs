use serde::Serialize;

/// A byte offset within a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ByteOffset(pub u32);

/// A line and column position within a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineCol(pub (u32, u32));

impl LineCol {
    #[must_use]
    pub fn line(&self) -> u32 {
        self.0 .0
    }

    #[must_use]
    pub fn column(&self) -> u32 {
        self.0 .1
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex(Vec<u32>);

impl LineIndex {
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let mut starts = Vec::with_capacity(256);
        starts.push(0);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(u32::try_from(i).unwrap_or_default() + 1);
            }
        }
        LineIndex(starts)
    }

    #[must_use]
    pub fn to_line_col(&self, offset: ByteOffset) -> LineCol {
        if self.0.is_empty() {
            return LineCol((0, 0));
        }

        let line = match self.0.binary_search(&offset.0) {
            Ok(exact) => exact,
            Err(0) => 0,
            Err(next) => next - 1,
        };

        let line_start = self.0[line];
        let column = offset.0.saturating_sub(line_start);

        LineCol((u32::try_from(line).unwrap_or_default(), column))
    }

    #[must_use]
    pub fn line_start(&self, line: u32) -> Option<u32> {
        self.0.get(line as usize).copied()
    }

    #[must_use]
    pub fn lines(&self) -> &[u32] {
        &self.0
    }
}
