//! LSP text document representation with efficient line indexing.
//!
//! `TextDocument` stores open file content with version tracking for the LSP
//! protocol. Pre-computed line indices enable O(1) position lookups, which is
//! critical for frequent position-based operations like hover, completion, and
//! diagnostics.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileKind;
use djls_source::LineIndex;
use djls_source::PositionEncoding;
use djls_source::Range;

/// In-memory representation of an open document in the LSP.
///
/// Combines document content with metadata needed for LSP operations, including
/// version tracking for synchronization and pre-computed line indices for
/// efficient position lookups.
///
#[derive(Clone)]
pub(crate) struct TextDocument {
    /// The document's path.
    path: Utf8PathBuf,
    /// The document's current in-memory content.
    content: String,
    /// The version number reported by the LSP client.
    version: i32,
    /// The file kind reported by the LSP client.
    kind: FileKind,
    /// Line index for efficient position and range lookups.
    line_index: LineIndex,
}

impl TextDocument {
    #[must_use]
    pub(crate) fn new(path: Utf8PathBuf, content: String, version: i32, kind: FileKind) -> Self {
        let line_index = LineIndex::from(content.as_str());
        Self {
            path,
            content,
            version,
            kind,
            line_index,
        }
    }

    #[must_use]
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    #[must_use]
    pub(crate) fn version(&self) -> i32 {
        self.version
    }

    #[must_use]
    pub(crate) fn kind(&self) -> FileKind {
        self.kind
    }

    #[must_use]
    pub(crate) fn path(&self) -> &Utf8Path {
        &self.path
    }

    pub(crate) fn update(
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

pub(crate) struct DocumentChange {
    range: Option<Range>,
    text: String,
}

impl DocumentChange {
    #[must_use]
    pub(crate) fn new(range: Option<Range>, text: String) -> Self {
        Self { range, text }
    }

    #[must_use]
    pub(crate) fn range(&self) -> Option<&Range> {
        self.range.as_ref()
    }

    #[must_use]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Apply this change to content, returning the updated text.
    #[must_use]
    fn apply(&self, content: &str, line_index: &LineIndex, encoding: PositionEncoding) -> String {
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
    use djls_source::FileKind;
    use djls_source::LineCol;

    use super::*;

    fn text_document(content: &str, version: i32) -> TextDocument {
        let path = Utf8Path::new("/test.txt");
        TextDocument::new(
            path.to_path_buf(),
            content.to_string(),
            version,
            FileKind::Other,
        )
    }

    #[test]
    fn incremental_update_single_change() {
        let mut doc = text_document("Hello world", 1);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 6), LineCol::new(0, 11))),
            "Rust".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello Rust");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn incremental_update_multiple_changes() {
        let mut doc = text_document("First line\nSecond line\nThird line", 1);

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
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn full_document_replacement() {
        let mut doc = text_document("Old content", 1);

        let changes = vec![DocumentChange::new(
            None,
            "Completely new content".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Completely new content");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn incremental_update_with_emoji() {
        let mut doc = text_document("Hello 🌍 world", 1);

        let changes = vec![DocumentChange::new(
            Some(Range::new(LineCol::new(0, 9), LineCol::new(0, 14))),
            "Rust".to_string(),
        )];

        doc.update(changes, 2, PositionEncoding::Utf16);
        assert_eq!(doc.content(), "Hello 🌍 Rust");
    }
}
