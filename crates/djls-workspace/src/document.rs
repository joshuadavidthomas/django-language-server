//! LSP text document representation with efficient line indexing
//!
//! [`TextDocument`] stores open file content with version tracking for the LSP protocol.
//! Pre-computed line indices enable O(1) position lookups, which is critical for
//! performance when handling frequent position-based operations like hover, completion,
//! and diagnostics.

use camino::Utf8Path;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FileKind;
use djls_source::LineIndex;
use djls_source::PositionEncoding;
use djls_source::Range;

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
    /// The file kind for analyzer routing (python, template, other)
    kind: FileKind,
    /// Line index for efficient position lookups
    line_index: LineIndex,
    /// The Salsa file this document represents
    file: File,
}

impl TextDocument {
    #[must_use]
    pub fn new(content: String, version: i32, kind: FileKind, file: File) -> Self {
        let line_index = LineIndex::from(content.as_str());
        Self {
            content,
            version,
            kind,
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
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    #[must_use]
    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }

    pub fn path<'db>(&self, db: &'db dyn SourceDb) -> &'db Utf8Path {
        self.file.path(db)
    }

    pub fn update(
        &mut self,
        changes: Vec<DocumentChange>,
        version: i32,
        encoding: PositionEncoding,
    ) {
        if changes.len() == 1 && changes[0].range.is_none() {
            self.content.clone_from(&changes[0].text);
            self.line_index = LineIndex::from(self.content.as_str());
            self.version = version;
            return;
        }

        let mut content = self.content.clone();
        let mut line_index = self.line_index.clone();

        for change in changes {
            content = change.apply(&content, &line_index, encoding);
            line_index = LineIndex::from(content.as_str());
        }

        self.content = content;
        self.line_index = line_index;
        self.version = version;
    }
}

pub struct DocumentChange {
    range: Option<Range>,
    text: String,
}

impl DocumentChange {
    #[must_use]
    pub fn new(range: Option<Range>, text: String) -> Self {
        Self { range, text }
    }

    #[must_use]
    pub fn range(&self) -> &Option<Range> {
        &self.range
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Apply this change to content, returning the new content
    #[must_use]
    pub fn apply(
        &self,
        content: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> String {
        if let Some(range) = &self.range {
            let start_offset = line_index.offset(content, range.start(), encoding).get() as usize;
            let end_offset = line_index.offset(content, range.end(), encoding).get() as usize;

            let mut result = String::with_capacity(content.len() + self.text.len());
            result.push_str(&content[..start_offset]);
            result.push_str(&self.text);
            result.push_str(&content[end_offset..]);
            result
        } else {
            self.text.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::LineCol;
    use djls_source::Range;

    use super::*;

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

    fn text_document(content: &str, version: i32, kind: FileKind) -> TextDocument {
        let db = TestDb::default();
        let path = Utf8Path::new("/test.txt");
        let file = File::new(&db, path.into(), 0);
        TextDocument::new(content.to_string(), version, kind, file)
    }

    #[test]
    fn test_incremental_update_single_change() {
        let mut doc = text_document("Hello world", 1, FileKind::Other);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 6), LineCol::new(0, 11))),
            "Rust".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello Rust");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn test_incremental_update_multiple_changes() {
        let mut doc = text_document("First line\nSecond line\nThird line", 1, FileKind::Other);

        let changes = vec![
            DocumentChange::new(
                Some(Range::new(LineCol::new(0, 0), LineCol::new(0, 5))),
                "1st".to_string(),
            ),
            DocumentChange::new(
                Some(Range::new(LineCol::new(2, 0), LineCol::new(2, 5))),
                "3rd".to_string(),
            ),
        ];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "1st line\nSecond line\n3rd line");
    }

    #[test]
    fn test_incremental_update_insertion() {
        let mut doc = text_document("Hello world", 1, FileKind::Other);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 5), LineCol::new(0, 5))),
            " beautiful".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello beautiful world");
    }

    #[test]
    fn test_incremental_update_deletion() {
        let mut doc = text_document("Hello beautiful world", 1, FileKind::Other);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 6), LineCol::new(0, 16))),
            String::new(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello world");
    }

    #[test]
    fn test_full_document_replacement() {
        let mut doc = text_document("Old content", 1, FileKind::Other);

        let changes = vec![DocumentChange::new(
            None,
            "Completely new content".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Completely new content");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn test_incremental_update_multiline() {
        let mut doc = text_document("Line 1\nLine 2\nLine 3", 1, FileKind::Other);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 5), LineCol::new(2, 4))),
            "A\nB\nC".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Line A\nB\nC 3");
    }

    #[test]
    fn test_incremental_update_with_emoji() {
        let mut doc = text_document("Hello 🌍 world", 1, FileKind::Other);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 9), LineCol::new(0, 14))),
            "Rust".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello 🌍 Rust");
    }

    #[test]
    fn test_incremental_update_newline_at_end() {
        let mut doc = text_document("Hello", 1, FileKind::Other);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 5), LineCol::new(0, 5))),
            "\nWorld".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello\nWorld");
    }
}
