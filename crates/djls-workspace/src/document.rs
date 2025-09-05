//! LSP text document representation with efficient line indexing
//!
//! [`TextDocument`] stores open file content with version tracking for the LSP protocol.
//! Pre-computed line indices enable O(1) position lookups, which is critical for
//! performance when handling frequent position-based operations like hover, completion,
//! and diagnostics.

use tower_lsp_server::lsp_types::Position;
use tower_lsp_server::lsp_types::Range;

use crate::encoding::PositionEncoding;
use crate::language::LanguageId;

/// In-memory representation of an open document in the LSP.
///
/// Combines document content with metadata needed for LSP operations,
/// including version tracking for synchronization and pre-computed line
/// indices for efficient position lookups.
#[derive(Clone, Debug)]
pub struct TextDocument {
    /// The document's content
    content: String,
    /// The version number of this document (from LSP)
    version: i32,
    /// The language identifier (python, htmldjango, etc.)
    language_id: LanguageId,
    /// Line index for efficient position lookups
    line_index: LineIndex,
}

impl TextDocument {
    #[must_use]
    pub fn new(content: String, version: i32, language_id: LanguageId) -> Self {
        let line_index = LineIndex::new(&content);
        Self {
            content,
            version,
            language_id,
            line_index,
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    #[must_use]
    pub fn version(&self) -> i32 {
        self.version
    }

    #[must_use]
    pub fn language_id(&self) -> LanguageId {
        self.language_id.clone()
    }

    #[must_use]
    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    #[must_use]
    pub fn get_line(&self, line: u32) -> Option<String> {
        let line_start = *self.line_index.line_starts.get(line as usize)?;
        let line_end = self
            .line_index
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.line_index.length);

        Some(self.content[line_start as usize..line_end as usize].to_string())
    }

    #[must_use]
    pub fn get_text_range(&self, range: Range, encoding: PositionEncoding) -> Option<String> {
        let start_offset = self.line_index.offset(range.start, &self.content, encoding) as usize;
        let end_offset = self.line_index.offset(range.end, &self.content, encoding) as usize;

        Some(self.content[start_offset..end_offset].to_string())
    }

    /// Update the document content with LSP text changes
    ///
    /// Supports both full document replacement and incremental updates.
    /// Following ruff's approach: incremental sync is used for network efficiency,
    /// but we rebuild the full document text internally.
    pub fn update(
        &mut self,
        changes: Vec<tower_lsp_server::lsp_types::TextDocumentContentChangeEvent>,
        version: i32,
        encoding: PositionEncoding,
    ) {
        // Fast path: single change without range = full document replacement
        if changes.len() == 1 && changes[0].range.is_none() {
            self.content.clone_from(&changes[0].text);
            self.line_index = LineIndex::new(&self.content);
            self.version = version;
            return;
        }

        // Incremental path: apply changes to rebuild the document
        let mut new_content = self.content.clone();

        for change in changes {
            if let Some(range) = change.range {
                // Convert LSP range to byte offsets using the negotiated encoding
                let start_offset =
                    self.line_index.offset(range.start, &new_content, encoding) as usize;
                let end_offset = self.line_index.offset(range.end, &new_content, encoding) as usize;

                // Apply change
                new_content.replace_range(start_offset..end_offset, &change.text);

                // Rebuild line index after each change since positions shift
                // This is necessary for subsequent changes to have correct offsets
                self.line_index = LineIndex::new(&new_content);
            } else {
                // No range means full replacement
                new_content = change.text;
                self.line_index = LineIndex::new(&new_content);
            }
        }

        self.content = new_content;
        self.version = version;
    }

    #[must_use]
    pub fn position_to_offset(
        &self,
        position: Position,
        encoding: PositionEncoding,
    ) -> Option<u32> {
        Some(self.line_index.offset(position, &self.content, encoding))
    }

    #[must_use]
    pub fn offset_to_position(&self, offset: u32) -> Position {
        self.line_index.position(offset)
    }
}

/// Pre-computed line start positions for efficient position/offset conversion.
///
/// Computing line positions on every lookup would be O(n) where n is the document size.
/// By pre-computing during document creation/updates, we get O(1) lookups for line starts
/// and O(log n) for position-to-offset conversions via binary search.
#[derive(Clone, Debug)]
pub struct LineIndex {
    pub line_starts: Vec<u32>,
    pub length: u32,
    pub kind: IndexKind,
}

impl LineIndex {
    #[must_use]
    pub fn new(text: &str) -> Self {
        let kind = if text.is_ascii() {
            IndexKind::Ascii
        } else {
            IndexKind::Utf8
        };

        let mut line_starts = vec![0];
        let mut pos_utf8 = 0;

        for c in text.chars() {
            pos_utf8 += u32::try_from(c.len_utf8()).unwrap_or(0);
            if c == '\n' {
                line_starts.push(pos_utf8);
            }
        }

        Self {
            line_starts,
            length: pos_utf8,
            kind,
        }
    }

    /// Convert position to text offset using the specified encoding
    ///
    /// Returns a valid offset, clamping out-of-bounds positions to document/line boundaries
    pub fn offset(&self, position: Position, text: &str, encoding: PositionEncoding) -> u32 {
        // Handle line bounds - if line > line_count, return document length
        let line_start_utf8 = match self.line_starts.get(position.line as usize) {
            Some(start) => *start,
            None => return self.length, // Past end of document
        };

        if position.character == 0 {
            return line_start_utf8;
        }

        let next_line_start = self
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.length);

        let Some(line_text) = text.get(line_start_utf8 as usize..next_line_start as usize) else {
            return line_start_utf8;
        };

        // Fast path optimization for ASCII text, all encodings are equivalent to byte offsets
        if matches!(self.kind, IndexKind::Ascii) {
            let char_offset = position
                .character
                .min(u32::try_from(line_text.len()).unwrap_or(u32::MAX));
            return line_start_utf8 + char_offset;
        }

        match encoding {
            PositionEncoding::Utf8 => {
                // UTF-8: character positions are already byte offsets
                let char_offset = position
                    .character
                    .min(u32::try_from(line_text.len()).unwrap_or(u32::MAX));
                line_start_utf8 + char_offset
            }
            PositionEncoding::Utf16 => {
                // UTF-16: count UTF-16 code units
                let mut utf16_pos = 0;
                let mut utf8_pos = 0;

                for c in line_text.chars() {
                    if utf16_pos >= position.character {
                        break;
                    }
                    utf16_pos += u32::try_from(c.len_utf16()).unwrap_or(0);
                    utf8_pos += u32::try_from(c.len_utf8()).unwrap_or(0);
                }

                // If character position exceeds line length, clamp to line end
                line_start_utf8 + utf8_pos
            }
            PositionEncoding::Utf32 => {
                // UTF-32: count Unicode code points (characters)
                let mut utf8_pos = 0;

                for (char_count, c) in line_text.chars().enumerate() {
                    if char_count >= position.character as usize {
                        break;
                    }
                    utf8_pos += u32::try_from(c.len_utf8()).unwrap_or(0);
                }

                // If character position exceeds line length, clamp to line end
                line_start_utf8 + utf8_pos
            }
        }
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn position(&self, offset: u32) -> Position {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(line) => line - 1,
        };

        let line_start = self.line_starts[line];
        let character = offset - line_start;

        Position::new(u32::try_from(line).unwrap_or(0), character)
    }
}

/// Index kind for ASCII optimization
#[derive(Clone, Debug)]
pub enum IndexKind {
    /// Document contains only ASCII characters - enables fast path optimization
    Ascii,
    /// Document contains multi-byte UTF-8 characters - requires full UTF-8 processing
    Utf8,
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;

    use super::*;
    use crate::language::LanguageId;

    #[test]
    fn test_incremental_update_single_change() {
        let mut doc = TextDocument::new("Hello world".to_string(), 1, LanguageId::Other);

        // Replace "world" with "Rust"
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 6), Position::new(0, 11))),
            range_length: None,
            text: "Rust".to_string(),
        }];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello Rust");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn test_incremental_update_multiple_changes() {
        let mut doc = TextDocument::new(
            "First line\nSecond line\nThird line".to_string(),
            1,
            LanguageId::Other,
        );

        // Multiple changes: replace "First" with "1st" and "Third" with "3rd"
        let changes = vec![
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 0), Position::new(0, 5))),
                range_length: None,
                text: "1st".to_string(),
            },
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(2, 0), Position::new(2, 5))),
                range_length: None,
                text: "3rd".to_string(),
            },
        ];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "1st line\nSecond line\n3rd line");
    }

    #[test]
    fn test_incremental_update_insertion() {
        let mut doc = TextDocument::new("Hello world".to_string(), 1, LanguageId::Other);

        // Insert text at position (empty range)
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(0, 5))),
            range_length: None,
            text: " beautiful".to_string(),
        }];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello beautiful world");
    }

    #[test]
    fn test_incremental_update_deletion() {
        let mut doc = TextDocument::new("Hello beautiful world".to_string(), 1, LanguageId::Other);

        // Delete "beautiful " (replace with empty string)
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 6), Position::new(0, 16))),
            range_length: None,
            text: String::new(),
        }];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello world");
    }

    #[test]
    fn test_full_document_replacement() {
        let mut doc = TextDocument::new("Old content".to_string(), 1, LanguageId::Other);

        // Full document replacement (no range)
        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "Completely new content".to_string(),
        }];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Completely new content");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn test_incremental_update_multiline() {
        let mut doc = TextDocument::new("Line 1\nLine 2\nLine 3".to_string(), 1, LanguageId::Other);

        // Replace across multiple lines
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(2, 4))),
            range_length: None,
            text: "A\nB\nC".to_string(),
        }];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Line A\nB\nC 3");
    }

    #[test]
    fn test_incremental_update_with_emoji() {
        let mut doc = TextDocument::new("Hello 🌍 world".to_string(), 1, LanguageId::Other);

        // Replace "world" after emoji - must handle UTF-16 positions correctly
        // "Hello " = 6 UTF-16 units, "🌍" = 2 UTF-16 units, " " = 1 unit, "world" starts at 9
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 9), Position::new(0, 14))),
            range_length: None,
            text: "Rust".to_string(),
        }];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello 🌍 Rust");
    }

    #[test]
    fn test_incremental_update_newline_at_end() {
        let mut doc = TextDocument::new("Hello".to_string(), 1, LanguageId::Other);

        // Add newline and new line at end
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(0, 5))),
            range_length: None,
            text: "\nWorld".to_string(),
        }];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello\nWorld");
    }

    #[test]
    fn test_utf16_position_handling() {
        // Test document with emoji and multi-byte characters
        let content = "Hello 🌍!\nSecond 行 line";
        let doc = TextDocument::new(content.to_string(), 1, LanguageId::HtmlDjango);

        // Test position after emoji
        // "Hello 🌍!" - the 🌍 emoji is 4 UTF-8 bytes but 2 UTF-16 code units
        // Position after the emoji should be at UTF-16 position 7 (Hello + space + emoji)
        let pos_after_emoji = Position::new(0, 7);
        let offset = doc
            .position_to_offset(pos_after_emoji, PositionEncoding::Utf16)
            .expect("Should get offset");

        // The UTF-8 byte offset should be at the "!" character
        assert_eq!(doc.content().chars().nth(7).unwrap(), '!');
        assert_eq!(&doc.content()[(offset as usize)..=(offset as usize)], "!");

        // Test range extraction with non-ASCII characters
        let range = Range::new(Position::new(0, 0), Position::new(0, 7));
        let text = doc
            .get_text_range(range, PositionEncoding::Utf16)
            .expect("Should get text range");
        assert_eq!(text, "Hello 🌍");

        // Test position on second line with CJK character
        // "Second 行 line" - 行 is 3 UTF-8 bytes but 1 UTF-16 code unit
        // Position after the CJK character should be at UTF-16 position 8
        let pos_after_cjk = Position::new(1, 8);
        let offset_cjk = doc
            .position_to_offset(pos_after_cjk, PositionEncoding::Utf16)
            .expect("Should get offset");

        // Find the start of line 2 in UTF-8 bytes
        let line2_start = doc.content().find('\n').unwrap() + 1;
        let line2_offset = offset_cjk as usize - line2_start;
        let line2 = &doc.content()[line2_start..];
        assert_eq!(&line2[line2_offset..=line2_offset], " ");
    }

    #[test]
    fn test_get_text_range_with_emoji() {
        let content = "Hello 🌍 world";
        let doc = TextDocument::new(content.to_string(), 1, LanguageId::HtmlDjango);

        // Range that spans across the emoji
        // "Hello 🌍 world"
        // H(1) e(1) l(1) l(1) o(1) space(1) 🌍(2) space(1) w(1)...
        // From position 5 (space before emoji) to position 8 (space after emoji)
        let range = Range::new(Position::new(0, 5), Position::new(0, 8));
        let text = doc
            .get_text_range(range, PositionEncoding::Utf16)
            .expect("Should get text range");
        assert_eq!(text, " 🌍");
    }

    #[test]
    fn test_line_index_utf16_conversion() {
        let text = "Hello 🌍!\nWorld 行 test";
        let line_index = LineIndex::new(text);

        // Test position conversion with emoji on first line
        let pos_emoji = Position::new(0, 7); // After emoji
        let offset = line_index.offset(pos_emoji, text, PositionEncoding::Utf16);
        assert_eq!(&text[(offset as usize)..=(offset as usize)], "!");

        // Test position conversion with CJK on second line
        // "World 行 test"
        // W(1) o(1) r(1) l(1) d(1) space(1) 行(1) space(1) t(1)...
        // Position after CJK character should be at UTF-16 position 7
        let pos_cjk = Position::new(1, 7);
        let offset_cjk = line_index.offset(pos_cjk, text, PositionEncoding::Utf16);
        assert_eq!(&text[(offset_cjk as usize)..=(offset_cjk as usize)], " ");
    }
}
