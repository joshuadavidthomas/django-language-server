use std::fmt;

use crate::line::LineIndex;
use crate::position::LineCol;
use crate::position::Offset;

/// Specifies how column positions are counted in text.
///
/// While motivated by LSP (Language Server Protocol) requirements, this enum
/// represents a fundamental choice about text position measurement that any
/// text processing system must make. Different systems count "column" positions
/// differently:
///
/// - Some count bytes (fast but breaks on multi-byte characters)
/// - Some count UTF-16 code units (common in JavaScript/Windows ecosystems)
/// - Some count Unicode codepoints (intuitive but slower)
///
/// This crate provides encoding-aware position conversion to support different
/// client expectations without coupling to specific protocol implementations.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    /// Column positions count UTF-8 code units (bytes from line start)
    Utf8,
    /// Column positions count UTF-16 code units (common in VS Code and Windows editors)
    #[default]
    Utf16,
    /// Column positions count Unicode scalar values (codepoints)
    Utf32,
}

impl fmt::Display for PositionEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utf8 => write!(f, "utf-8"),
            Self::Utf16 => write!(f, "utf-16"),
            Self::Utf32 => write!(f, "utf-32"),
        }
    }
}

impl PositionEncoding {
    /// Convert a line/column position to a byte offset with encoding awareness.
    ///
    /// The encoding specifies how the column value should be interpreted:
    /// - `PositionEncoding::Utf8`: column is a byte offset from line start
    /// - `PositionEncoding::Utf16`: column counts UTF-16 code units
    /// - `PositionEncoding::Utf32`: column counts Unicode codepoints
    ///
    /// This method is primarily used to convert protocol-specific positions
    /// (which may use different column counting methods) into byte offsets
    /// that can be used to index into the actual UTF-8 text.
    ///
    /// # Examples
    ///
    /// ```
    /// # use djls_source::{LineIndex, LineCol, Offset, PositionEncoding};
    /// let text = "Hello 🌍 world";
    /// let index = LineIndex::from(text);
    ///
    /// // UTF-16: "Hello " (6) + "🌍" (2 UTF-16 units) = position 8
    /// let offset = PositionEncoding::Utf16.line_col_to_offset(
    ///     &index,
    ///     LineCol::new(0, 8),
    ///     text
    /// );
    /// assert_eq!(offset, Some(Offset::new(10))); // "Hello 🌍" is 10 bytes
    /// ```
    #[must_use]
    pub fn line_col_to_offset(
        &self,
        index: &LineIndex,
        line_col: LineCol,
        text: &str,
    ) -> Option<Offset> {
        let line = line_col.line();
        let character = line_col.column();

        // Handle line bounds - if line > line_count, return document length
        let line_start_utf8 = match index.lines().get(line as usize) {
            Some(start) => *start,
            None => return Offset::try_from(text.len()).ok(),
        };

        if character == 0 {
            return Some(Offset::new(line_start_utf8));
        }

        let next_line_start = index
            .lines()
            .get(line as usize + 1)
            .copied()
            .unwrap_or_else(|| u32::try_from(text.len()).unwrap_or(u32::MAX));

        let line_text = text.get(line_start_utf8 as usize..next_line_start as usize)?;

        // Fast path optimization for ASCII text, all encodings are equivalent to byte offsets
        if line_text.is_ascii() {
            let char_offset = character.min(u32::try_from(line_text.len()).unwrap_or(u32::MAX));
            return Some(Offset::new(line_start_utf8 + char_offset));
        }

        match self {
            PositionEncoding::Utf8 => {
                // UTF-8: character positions are already byte offsets
                let char_offset = character.min(u32::try_from(line_text.len()).unwrap_or(u32::MAX));
                Some(Offset::new(line_start_utf8 + char_offset))
            }
            PositionEncoding::Utf16 => {
                // UTF-16: count UTF-16 code units
                let mut utf16_pos = 0;
                let mut utf8_pos = 0;

                for c in line_text.chars() {
                    if utf16_pos >= character {
                        break;
                    }
                    utf16_pos += u32::try_from(c.len_utf16()).unwrap_or(0);
                    utf8_pos += u32::try_from(c.len_utf8()).unwrap_or(0);
                }

                // If character position exceeds line length, clamp to line end
                Some(Offset::new(line_start_utf8 + utf8_pos))
            }
            PositionEncoding::Utf32 => {
                // UTF-32: count Unicode code points (characters)
                let mut utf8_pos = 0;

                for (char_count, c) in line_text.chars().enumerate() {
                    if char_count >= character as usize {
                        break;
                    }
                    utf8_pos += u32::try_from(c.len_utf8()).unwrap_or(0);
                }

                // If character position exceeds line length, clamp to line end
                Some(Offset::new(line_start_utf8 + utf8_pos))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_utf16() {
        assert_eq!(PositionEncoding::default(), PositionEncoding::Utf16);
    }

    #[test]
    fn test_position_encoding_display() {
        assert_eq!(PositionEncoding::Utf8.to_string(), "utf-8");
        assert_eq!(PositionEncoding::Utf16.to_string(), "utf-16");
        assert_eq!(PositionEncoding::Utf32.to_string(), "utf-32");
    }

    #[test]
    fn test_line_col_to_offset_utf16() {
        let text = "Hello 🌍 world";
        let index = LineIndex::from(text);

        // "Hello " = 6 UTF-16 units, "🌍" = 2 UTF-16 units
        // So position (0, 8) in UTF-16 should be after the emoji
        let offset = PositionEncoding::Utf16
            .line_col_to_offset(&index, LineCol::new(0, 8), text)
            .expect("Should get offset");
        assert_eq!(offset, Offset::new(10)); // "Hello 🌍" is 10 bytes

        // In UTF-8, character 10 would be at the 'r' in 'world'
        let offset_utf8 = PositionEncoding::Utf8
            .line_col_to_offset(&index, LineCol::new(0, 10), text)
            .expect("Should get offset");
        assert_eq!(offset_utf8, Offset::new(10));
    }

    #[test]
    fn test_line_col_to_offset_ascii_fast_path() {
        let text = "Hello world";
        let index = LineIndex::from(text);

        // For ASCII text, all encodings should give the same result
        let offset_utf8 = PositionEncoding::Utf8
            .line_col_to_offset(&index, LineCol::new(0, 5), text)
            .expect("Should get offset");
        let offset_utf16 = PositionEncoding::Utf16
            .line_col_to_offset(&index, LineCol::new(0, 5), text)
            .expect("Should get offset");
        let offset_utf32 = PositionEncoding::Utf32
            .line_col_to_offset(&index, LineCol::new(0, 5), text)
            .expect("Should get offset");

        assert_eq!(offset_utf8, Offset::new(5));
        assert_eq!(offset_utf16, Offset::new(5));
        assert_eq!(offset_utf32, Offset::new(5));
    }
}
