//! LSP text document representation with efficient line indexing
//!
//! [`TextDocument`] stores open file content with version tracking for the LSP protocol.
//! Pre-computed line indices enable O(1) position lookups, which is critical for
//! performance when handling frequent position-based operations like hover, completion,
//! and diagnostics.

use crate::language::LanguageId;
use crate::template::ClosingBrace;
use crate::template::TemplateTagContext;
use tower_lsp_server::lsp_types::Position;
use tower_lsp_server::lsp_types::Range;

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
    /// Create a new TextDocument with the given content
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

    /// Get the document's content
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get the version number
    #[must_use]
    pub fn version(&self) -> i32 {
        self.version
    }

    /// Get the language identifier
    #[must_use]
    pub fn language_id(&self) -> LanguageId {
        self.language_id.clone()
    }

    #[must_use]
    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

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

    pub fn get_text_range(&self, range: Range) -> Option<String> {
        let start_offset = self.line_index.offset(range.start)? as usize;
        let end_offset = self.line_index.offset(range.end)? as usize;

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

    pub fn get_template_tag_context(&self, position: Position) -> Option<TemplateTagContext> {
        let start = self.line_index.line_starts.get(position.line as usize)?;
        let end = self
            .line_index
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.line_index.length);

        let line = &self.content[*start as usize..end as usize];
        let char_pos: usize = position.character.try_into().ok()?;
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

    pub fn position_to_offset(&self, position: Position) -> Option<u32> {
        self.line_index.offset(position)
    }

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
    pub line_starts_utf16: Vec<u32>,
    pub length: u32,
    pub length_utf16: u32,
}

impl LineIndex {
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        let mut line_starts_utf16 = vec![0];
        let mut pos_utf8 = 0;
        let mut pos_utf16 = 0;

        for c in text.chars() {
            pos_utf8 += u32::try_from(c.len_utf8()).unwrap_or(0);
            pos_utf16 += u32::try_from(c.len_utf16()).unwrap_or(0);
            if c == '\n' {
                line_starts.push(pos_utf8);
                line_starts_utf16.push(pos_utf16);
            }
        }

        Self {
            line_starts,
            line_starts_utf16,
            length: pos_utf8,
            length_utf16: pos_utf16,
        }
    }

    #[must_use]
    pub fn offset(&self, position: Position) -> Option<u32> {
        let line_start = self.line_starts.get(position.line as usize)?;

        Some(line_start + position.character)
    }

    /// Convert UTF-16 LSP position to UTF-8 byte offset
    pub fn offset_utf16(&self, position: Position, text: &str) -> Option<u32> {
        let line_start_utf8 = self.line_starts.get(position.line as usize)?;
        let _line_start_utf16 = self.line_starts_utf16.get(position.line as usize)?;

        // If position is at start of line, return UTF-8 line start
        if position.character == 0 {
            return Some(*line_start_utf8);
        }

        // Find the line text
        let next_line_start = self
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.length);

        let line_text = text.get(*line_start_utf8 as usize..next_line_start as usize)?;

        // Convert UTF-16 character offset to UTF-8 byte offset within the line
        let mut utf16_pos = 0;
        let mut utf8_pos = 0;

        for c in line_text.chars() {
            if utf16_pos >= position.character {
                break;
            }
            utf16_pos += u32::try_from(c.len_utf16()).unwrap_or(0);
            utf8_pos += u32::try_from(c.len_utf8()).unwrap_or(0);
        }

        Some(line_start_utf8 + utf8_pos)
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
