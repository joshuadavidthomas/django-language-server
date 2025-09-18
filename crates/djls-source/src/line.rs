use crate::LineCol;
use crate::Offset;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
    Cr,
}

impl LineEnding {
    #[inline]
    #[allow(dead_code)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
            Self::Cr => "\r",
        }
    }

    #[inline]
    pub const fn len(self) -> usize {
        match self {
            Self::Cr | Self::Lf => 1,
            Self::Crlf => 2,
        }
    }

    #[allow(dead_code)]
    pub const fn is_line_feed(self) -> bool {
        matches!(self, Self::Lf)
    }

    #[allow(dead_code)]
    pub const fn is_carriage_return_line_feed(self) -> bool {
        matches!(self, Self::Crlf)
    }

    #[allow(dead_code)]
    pub const fn is_carriage_return(self) -> bool {
        matches!(self, Self::Cr)
    }

    #[inline]
    pub fn match_at(bytes: &[u8], i: usize) -> Option<Self> {
        match bytes.get(i) {
            Some(b'\n') => Some(Self::Lf),
            Some(b'\r') if bytes.get(i + 1) == Some(&b'\n') => Some(Self::Crlf),
            Some(b'\r') => Some(Self::Cr),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex(Vec<u32>);

impl LineIndex {
    #[must_use]
    pub fn lines(&self) -> &[u32] {
        &self.0
    }

    #[must_use]
    pub fn line_start(&self, line: u32) -> Option<u32> {
        self.0.get(line as usize).copied()
    }

    #[must_use]
    pub fn to_line_col(&self, offset: Offset) -> LineCol {
        if self.lines().is_empty() {
            return LineCol::new(0, 0);
        }

        let offset_u32 = offset.as_ref();

        let line = match self.lines().binary_search(offset_u32) {
            Ok(exact) => exact,
            Err(0) => 0,
            Err(next) => next - 1,
        };
        let column = offset_u32.saturating_sub(self.0[line]);

        LineCol::new(u32::try_from(line).unwrap_or_default(), column)
    }
}

impl From<&[u8]> for LineIndex {
    fn from(bytes: &[u8]) -> Self {
        let mut starts = Vec::with_capacity(256);
        starts.push(0);

        let mut i = 0;
        while i < bytes.len() {
            if let Some(ending) = LineEnding::match_at(bytes, i) {
                let len = ending.len();
                starts.push(u32::try_from(i + len).unwrap_or(u32::MAX));
                i += len;
            } else {
                i += 1;
            }
        }

        Self(starts)
    }
}

impl From<&str> for LineIndex {
    fn from(text: &str) -> Self {
        let bytes = text.as_bytes();
        Self::from(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_index_unix_endings() {
        let text = "line1\nline2\nline3";
        let index = LineIndex::from(text);
        assert_eq!(index.lines(), &[0, 6, 12]);
    }

    #[test]
    fn test_line_index_windows_endings() {
        let text = "line1\r\nline2\r\nline3";
        let index = LineIndex::from(text);
        // After "line1\r\n" (7 bytes), next line starts at byte 7
        // After "line2\r\n" (7 bytes), next line starts at byte 14
        assert_eq!(index.lines(), &[0, 7, 14]);
    }

    #[test]
    fn test_line_index_mixed_endings() {
        let text = "line1\nline2\r\nline3\rline4";
        let index = LineIndex::from(text);
        // "line1\n" -> next at 6
        // "line2\r\n" -> next at 13
        // "line3\r" -> next at 19
        assert_eq!(index.lines(), &[0, 6, 13, 19]);
    }

    #[test]
    fn test_line_index_empty() {
        let text = "";
        let index = LineIndex::from(text);
        assert_eq!(index.lines(), &[0]);
    }

    #[test]
    fn test_to_line_col_with_crlf() {
        let text = "hello\r\nworld";
        let index = LineIndex::from(text);

        // "hello" is 5 bytes, then \r\n, so "world" starts at byte 7
        assert_eq!(index.to_line_col(Offset::new(0)), LineCol::new(0, 0));
        assert_eq!(index.to_line_col(Offset::new(7)), LineCol::new(1, 0));
        assert_eq!(index.to_line_col(Offset::new(8)), LineCol::new(1, 1));
    }
}
