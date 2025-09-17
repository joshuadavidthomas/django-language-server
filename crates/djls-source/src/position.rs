use serde::Serialize;

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

/// A line and column position within a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineCol((u32, u32));

impl LineCol {
    #[must_use]
    pub fn new(line: u32, column: u32) -> Self {
        Self((line, column))
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex(Vec<u32>);

impl LineIndex {
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let mut starts = Vec::with_capacity(256);
        starts.push(0);

        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    // LF - Unix style line ending
                    starts.push(u32::try_from(i + 1).unwrap_or_default());
                    i += 1;
                }
                b'\r' => {
                    // CR - check if followed by LF for Windows style
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                        // CRLF - Windows style line ending
                        starts.push(u32::try_from(i + 2).unwrap_or_default());
                        i += 2;
                    } else {
                        // Just CR - old Mac style line ending
                        starts.push(u32::try_from(i + 1).unwrap_or_default());
                        i += 1;
                    }
                }
                _ => i += 1,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_index_unix_endings() {
        let text = "line1\nline2\nline3";
        let index = LineIndex::from_text(text);
        assert_eq!(index.lines(), &[0, 6, 12]);
    }

    #[test]
    fn test_line_index_windows_endings() {
        let text = "line1\r\nline2\r\nline3";
        let index = LineIndex::from_text(text);
        // After "line1\r\n" (7 bytes), next line starts at byte 7
        // After "line2\r\n" (7 bytes), next line starts at byte 14
        assert_eq!(index.lines(), &[0, 7, 14]);
    }

    #[test]
    fn test_line_index_mixed_endings() {
        let text = "line1\nline2\r\nline3\rline4";
        let index = LineIndex::from_text(text);
        // "line1\n" -> next at 6
        // "line2\r\n" -> next at 13
        // "line3\r" -> next at 19
        assert_eq!(index.lines(), &[0, 6, 13, 19]);
    }

    #[test]
    fn test_line_index_empty() {
        let text = "";
        let index = LineIndex::from_text(text);
        assert_eq!(index.lines(), &[0]);
    }

    #[test]
    fn test_to_line_col_with_crlf() {
        let text = "hello\r\nworld";
        let index = LineIndex::from_text(text);

        // "hello" is 5 bytes, then \r\n, so "world" starts at byte 7
        assert_eq!(index.to_line_col(ByteOffset(0)), LineCol((0, 0)));
        assert_eq!(index.to_line_col(ByteOffset(7)), LineCol((1, 0)));
        assert_eq!(index.to_line_col(ByteOffset(8)), LineCol((1, 1)));
    }
}
