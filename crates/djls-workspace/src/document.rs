//! LSP text document representation with efficient line indexing
//!
//! [`TextDocument`] stores open file content with version tracking for the LSP protocol.
//! Pre-computed line indices enable O(1) position lookups, which is critical for
//! performance when handling frequent position-based operations like hover, completion,
//! and diagnostics.

use camino::Utf8Path;
use djls_source::File;
use djls_source::LineIndex;
use djls_source::PositionEncoding;
use tower_lsp_server::lsp_types::Position;
use tower_lsp_server::lsp_types::Range;

use crate::language::LanguageId;

/// In-memory representation of an open document in the LSP.
///
/// Combines document content with metadata needed for LSP operations,
/// including version tracking for synchronization and pre-computed line
/// indices for efficient position lookups.
///
/// Links to the corresponding Salsa [`File`] for integration with incremental
/// computation and invalidation tracking.
#[derive(Clone)]
pub struct TextDocument {
    /// The document's content
    content: String,
    /// The version number of this document (from LSP)
    version: i32,
    /// The language identifier (python, htmldjango, etc.)
    language_id: LanguageId,
    /// Line index for efficient position lookups
    line_index: LineIndex,
    /// The Salsa file this document represents
    file: File,
}

impl TextDocument {
    pub fn new(
        content: String,
        version: i32,
        language_id: LanguageId,
        path: &Utf8Path,
        db: &dyn djls_source::Db,
    ) -> Self {
        let file = db.get_or_create_file(path);
        let line_index = LineIndex::from(content.as_str());
        Self {
            content,
            version,
            language_id,
            line_index,
            file,
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
    pub fn file(&self) -> File {
        self.file
    }

    pub fn open(&self, db: &mut dyn djls_source::Db) {
        db.bump_file_revision(self.file);
    }

    pub fn save(&self, db: &mut dyn djls_source::Db) {
        db.bump_file_revision(self.file);
    }

    pub fn close(&self, db: &mut dyn djls_source::Db) {
        db.bump_file_revision(self.file);
    }

    #[must_use]
    pub fn get_line(&self, line: u32) -> Option<String> {
        let line_start = *self.line_index.lines().get(line as usize)?;
        let line_end = self
            .line_index
            .lines()
            .get(line as usize + 1)
            .copied()
            .unwrap_or_else(|| u32::try_from(self.content.len()).unwrap_or(u32::MAX));

        Some(self.content[line_start as usize..line_end as usize].to_string())
    }

    #[must_use]
    pub fn get_text_range(&self, range: Range, encoding: PositionEncoding) -> Option<String> {
        let start_offset =
            Self::calculate_offset(&self.line_index, range.start, &self.content, encoding) as usize;
        let end_offset =
            Self::calculate_offset(&self.line_index, range.end, &self.content, encoding) as usize;

        Some(self.content[start_offset..end_offset].to_string())
    }

    /// Update the document content with LSP text changes
    ///
    /// Supports both full document replacement and incremental updates.
    /// Following ruff's approach: incremental sync is used for network efficiency,
    /// but we rebuild the full document text internally.
    pub fn update(
        &mut self,
        db: &mut dyn djls_source::Db,
        changes: Vec<tower_lsp_server::lsp_types::TextDocumentContentChangeEvent>,
        version: i32,
        encoding: PositionEncoding,
    ) {
        // Fast path: single change without range = full document replacement
        if changes.len() == 1 && changes[0].range.is_none() {
            self.content.clone_from(&changes[0].text);
            self.line_index = LineIndex::from(self.content.as_str());
            self.version = version;
            db.bump_file_revision(self.file);
            return;
        }

        // Incremental path: apply changes to rebuild the document
        // We need to track both content and line index together as we apply changes
        let mut new_content = self.content.clone();
        let mut new_line_index = self.line_index.clone();

        for change in changes {
            if let Some(range) = change.range {
                // Convert LSP range to byte offsets using the current line index
                // that matches the current state of new_content
                let start_offset =
                    Self::calculate_offset(&new_line_index, range.start, &new_content, encoding)
                        as usize;
                let end_offset =
                    Self::calculate_offset(&new_line_index, range.end, &new_content, encoding)
                        as usize;

                // Apply change
                new_content.replace_range(start_offset..end_offset, &change.text);
            } else {
                // No range means full replacement
                new_content = change.text;
            }

            // Rebuild line index to match the new content state
            new_line_index = LineIndex::from(new_content.as_str());
        }

        // Update all document state at once
        self.content = new_content;
        self.line_index = new_line_index;
        self.version = version;
        db.bump_file_revision(self.file);
    }

    /// Calculate byte offset from an LSP position using the given line index and text.
    fn calculate_offset(
        line_index: &LineIndex,
        position: Position,
        text: &str,
        encoding: PositionEncoding,
    ) -> u32 {
        let line_col = djls_source::LineCol::new(position.line, position.character);
        line_index.offset(line_col, text, encoding).get()
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;

    use super::*;
    use crate::language::LanguageId;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    impl Default for TestDb {
        fn default() -> Self {
            Self {
                storage: salsa::Storage::new(None),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn create_file(&self, path: &Utf8Path) -> File {
            File::new(self, path.to_path_buf(), 0)
        }

        fn get_file(&self, _path: &Utf8Path) -> Option<File> {
            None
        }

        fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    fn text_document(
        db: &mut TestDb,
        content: &str,
        version: i32,
        language_id: LanguageId,
    ) -> TextDocument {
        TextDocument::new(
            content.to_string(),
            version,
            language_id,
            Utf8Path::new("/test.txt"),
            db,
        )
    }

    #[test]
    fn test_incremental_update_single_change() {
        let mut db = TestDb::default();
        let mut doc = text_document(&mut db, "Hello world", 1, LanguageId::Other);

        // Replace "world" with "Rust"
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 6), Position::new(0, 11))),
            range_length: None,
            text: "Rust".to_string(),
        }];

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello Rust");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn test_incremental_update_multiple_changes() {
        let mut db = TestDb::default();
        let mut doc = text_document(
            &mut db,
            "First line\nSecond line\nThird line",
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

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "1st line\nSecond line\n3rd line");
    }

    #[test]
    fn test_incremental_update_insertion() {
        let mut db = TestDb::default();
        let mut doc = text_document(&mut db, "Hello world", 1, LanguageId::Other);

        // Insert text at position (empty range)
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(0, 5))),
            range_length: None,
            text: " beautiful".to_string(),
        }];

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello beautiful world");
    }

    #[test]
    fn test_incremental_update_deletion() {
        let mut db = TestDb::default();
        let mut doc = text_document(&mut db, "Hello beautiful world", 1, LanguageId::Other);

        // Delete "beautiful " (replace with empty string)
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 6), Position::new(0, 16))),
            range_length: None,
            text: String::new(),
        }];

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello world");
    }

    #[test]
    fn test_full_document_replacement() {
        let mut db = TestDb::default();
        let mut doc = text_document(&mut db, "Old content", 1, LanguageId::Other);

        // Full document replacement (no range)
        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "Completely new content".to_string(),
        }];

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Completely new content");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn test_incremental_update_multiline() {
        let mut db = TestDb::default();
        let mut doc = text_document(&mut db, "Line 1\nLine 2\nLine 3", 1, LanguageId::Other);

        // Replace across multiple lines
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(2, 4))),
            range_length: None,
            text: "A\nB\nC".to_string(),
        }];

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Line A\nB\nC 3");
    }

    #[test]
    fn test_incremental_update_with_emoji() {
        let mut db = TestDb::default();
        let mut doc = text_document(&mut db, "Hello 🌍 world", 1, LanguageId::Other);

        // Replace "world" after emoji - must handle UTF-16 positions correctly
        // "Hello " = 6 UTF-16 units, "🌍" = 2 UTF-16 units, " " = 1 unit, "world" starts at 9
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 9), Position::new(0, 14))),
            range_length: None,
            text: "Rust".to_string(),
        }];

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello 🌍 Rust");
    }

    #[test]
    fn test_incremental_update_newline_at_end() {
        let mut db = TestDb::default();
        let mut doc = text_document(&mut db, "Hello", 1, LanguageId::Other);

        // Add newline and new line at end
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(0, 5))),
            range_length: None,
            text: "\nWorld".to_string(),
        }];

        doc.update(&mut db, changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello\nWorld");
    }

    #[test]
    fn test_utf16_position_handling() {
        let mut db = TestDb::default();
        // Test document with emoji and multi-byte characters
        let doc = text_document(
            &mut db,
            "Hello 🌍!\nSecond 行 line",
            1,
            LanguageId::HtmlDjango,
        );

        // Test position after emoji by extracting text up to that position
        // "Hello 🌍!" - the 🌍 emoji is 4 UTF-8 bytes but 2 UTF-16 code units
        // "Hello " = 6 UTF-16 units, emoji = 2 UTF-16 units, so position 8 is after emoji
        let range_to_after_emoji = Range::new(Position::new(0, 0), Position::new(0, 8));
        let text_to_after_emoji = doc
            .get_text_range(range_to_after_emoji, PositionEncoding::Utf16)
            .expect("Should get text range");
        assert_eq!(text_to_after_emoji, "Hello 🌍");

        // Verify the next character is "!"
        let range_exclamation = Range::new(Position::new(0, 8), Position::new(0, 9));
        let exclamation = doc
            .get_text_range(range_exclamation, PositionEncoding::Utf16)
            .expect("Should get exclamation");
        assert_eq!(exclamation, "!");

        // Test range extraction with non-ASCII characters
        let range = Range::new(Position::new(0, 0), Position::new(0, 8));
        let text = doc
            .get_text_range(range, PositionEncoding::Utf16)
            .expect("Should get text range");
        assert_eq!(text, "Hello 🌍");

        // Test position on second line with CJK character
        // "Second 行 line" - 行 is 3 UTF-8 bytes but 1 UTF-16 code unit
        // Position after the CJK character should be at UTF-16 position 8
        let range_to_after_cjk = Range::new(Position::new(1, 0), Position::new(1, 8));
        let text_to_after_cjk = doc
            .get_text_range(range_to_after_cjk, PositionEncoding::Utf16)
            .expect("Should get text range");
        assert_eq!(text_to_after_cjk, "Second 行");

        // Verify the next character is a space
        let range_space = Range::new(Position::new(1, 8), Position::new(1, 9));
        let space = doc
            .get_text_range(range_space, PositionEncoding::Utf16)
            .expect("Should get space");
        assert_eq!(space, " ");
    }

    #[test]
    fn test_get_text_range_with_emoji() {
        let mut db = TestDb::default();
        let doc = text_document(&mut db, "Hello 🌍 world", 1, LanguageId::HtmlDjango);

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
}
