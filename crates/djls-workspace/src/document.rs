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
use crate::template::ClosingBrace;
use crate::template::TemplateTagContext;

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
    pub fn update(
        &mut self,
        changes: Vec<tower_lsp_server::lsp_types::TextDocumentContentChangeEvent>,
        version: i32,
    ) {
        // For now, we'll just handle full document updates
        // TODO: Handle incremental updates
        for change in changes {
            // TextDocumentContentChangeEvent has a `text` field that's a String, not Option<String>
            self.content = change.text;
            self.line_index = LineIndex::new(&self.content);
        }
        self.version = version;
    }

    #[must_use]
    pub fn get_template_tag_context(
        &self,
        position: Position,
        encoding: PositionEncoding,
    ) -> Option<TemplateTagContext> {
        let start = self.line_index.line_starts.get(position.line as usize)?;
        let end = self
            .line_index
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.line_index.length);

        let line = &self.content[*start as usize..end as usize];

        // Use the new offset method with the specified encoding
        let char_offset = self.line_index.offset(position, &self.content, encoding) as usize;
        let char_pos = char_offset - *start as usize;

        let prefix = &line[..char_pos];
        let rest_of_line = &line[char_pos..];
        let rest_trimmed = rest_of_line.trim_start();

        prefix.rfind("{%").map(|tag_start| {
            // Check if we're immediately after {% with no space
            let needs_leading_space = prefix.ends_with("{%");

            let closing_brace = if rest_trimmed.starts_with("%}") {
                ClosingBrace::FullClose
            } else if rest_trimmed.starts_with('}') {
                ClosingBrace::PartialClose
            } else {
                ClosingBrace::None
            };

            TemplateTagContext {
                partial_tag: prefix[tag_start + 2..].trim().to_string(),
                needs_leading_space,
                closing_brace,
            }
        })
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
        let mut line_starts = vec![0];
        let mut pos_utf8 = 0;

        // Check if text is pure ASCII for optimization
        let kind = if text.is_ascii() {
            IndexKind::Ascii
        } else {
            IndexKind::Utf8
        };

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

        // If position is at start of line, return line start
        if position.character == 0 {
            return line_start_utf8;
        }

        // Find the line text
        let next_line_start = self
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.length);

        let Some(line_text) = text.get(line_start_utf8 as usize..next_line_start as usize) else {
            return line_start_utf8;
        };

        // ASCII fast path optimization
        if matches!(self.kind, IndexKind::Ascii) {
            // For ASCII text, all encodings are equivalent to byte offsets
            let char_offset = position.character.min(line_text.len() as u32);
            return line_start_utf8 + char_offset;
        }

        // Handle different encodings for non-ASCII text
        match encoding {
            PositionEncoding::Utf8 => {
                // UTF-8: character positions are already byte offsets
                let char_offset = position.character.min(line_text.len() as u32);
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
                    utf16_pos += c.len_utf16() as u32;
                    utf8_pos += c.len_utf8() as u32;
                }

                // If character position exceeds line length, clamp to line end
                if utf16_pos < position.character && utf8_pos == line_text.len() as u32 {
                    line_start_utf8 + utf8_pos
                } else {
                    line_start_utf8 + utf8_pos
                }
            }
            PositionEncoding::Utf32 => {
                // UTF-32: count Unicode code points (characters)
                let mut char_count = 0;
                let mut utf8_pos = 0;

                for c in line_text.chars() {
                    if char_count >= position.character {
                        break;
                    }
                    char_count += 1;
                    utf8_pos += c.len_utf8() as u32;
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
    use super::*;
    use crate::language::LanguageId;

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
        assert_eq!(&doc.content()[offset as usize..offset as usize + 1], "!");

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
        assert_eq!(&line2[line2_offset..line2_offset + 1], " ");
    }

    #[test]
    fn test_template_tag_context_with_utf16() {
        // Test template with non-ASCII characters before template tag
        let content = "Título 🌍: {% for";
        let doc = TextDocument::new(content.to_string(), 1, LanguageId::HtmlDjango);

        // Position after "for" - UTF-16 position 17 (after 'r')
        let pos = Position::new(0, 17);
        let context = doc
            .get_template_tag_context(pos, PositionEncoding::Utf16)
            .expect("Should get template context");

        assert_eq!(context.partial_tag, "for");
        assert!(!context.needs_leading_space);
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
        assert_eq!(&text[offset as usize..offset as usize + 1], "!");

        // Test position conversion with CJK on second line
        // "World 行 test"
        // W(1) o(1) r(1) l(1) d(1) space(1) 行(1) space(1) t(1)...
        // Position after CJK character should be at UTF-16 position 7
        let pos_cjk = Position::new(1, 7);
        let offset_cjk = line_index.offset(pos_cjk, text, PositionEncoding::Utf16);
        assert_eq!(&text[offset_cjk as usize..offset_cjk as usize + 1], " ");
    }
}
