use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use anyhow::Result;
use camino::Utf8PathBuf;
use djls_project::TemplateTags;
use tower_lsp_server::lsp_types::CompletionItem;
use tower_lsp_server::lsp_types::CompletionItemKind;
use tower_lsp_server::lsp_types::CompletionResponse;
use tower_lsp_server::lsp_types::Diagnostic;
use tower_lsp_server::lsp_types::DiagnosticSeverity;
use tower_lsp_server::lsp_types::DidChangeTextDocumentParams;
use tower_lsp_server::lsp_types::DidCloseTextDocumentParams;
use tower_lsp_server::lsp_types::DidOpenTextDocumentParams;
use tower_lsp_server::lsp_types::Documentation;
use tower_lsp_server::lsp_types::InsertTextFormat;
use tower_lsp_server::lsp_types::MarkupContent;
use tower_lsp_server::lsp_types::MarkupKind;
use tower_lsp_server::lsp_types::Position;
use tower_lsp_server::lsp_types::Range;
use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;

use crate::bridge::FileStore;
use crate::db::TemplateAst;
use crate::vfs::FileKind;
use crate::vfs::TextSource;
use crate::vfs::Vfs;
use crate::ClosingBrace;
use crate::LanguageId;
use crate::LineIndex;
use crate::TextDocument;

pub struct DocumentStore {
    vfs: Arc<Vfs>,
    file_store: Arc<Mutex<FileStore>>,
    documents: HashMap<String, TextDocument>,
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self {
            vfs: Arc::new(Vfs::default()),
            file_store: Arc::new(Mutex::new(FileStore::new())),
            documents: HashMap::new(),
        }
    }
}

impl DocumentStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a document with the given URI, version, language, and text content.
    /// This creates a new TextDocument and stores it internally, hiding VFS details.
    pub fn open_document(
        &mut self,
        uri: url::Url,
        version: i32,
        language_id: LanguageId,
        text: String,
    ) -> Result<()> {
        let uri_str = uri.to_string();
        let kind = FileKind::from(language_id.clone());

        // Convert URI to path - simplified for now, just use URI string
        let path = Utf8PathBuf::from(uri.as_str());

        // Store content in VFS
        let text_source = TextSource::Overlay(Arc::from(text.as_str()));
        let file_id = self.vfs.intern_file(uri, path, kind, text_source);

        // Set overlay content in VFS
        self.vfs.set_overlay(file_id, Arc::from(text.as_str()))?;

        // Sync VFS snapshot to FileStore for Salsa tracking
        let snapshot = self.vfs.snapshot();
        let mut file_store = self.file_store.lock().unwrap();
        file_store.apply_vfs_snapshot(&snapshot);

        // Create TextDocument with LineIndex
        let document = TextDocument::new(uri_str.clone(), version, language_id, file_id, &text);
        self.documents.insert(uri_str, document);

        Ok(())
    }

    /// Update a document with the given URI, version, and text changes.
    /// This applies changes to the document and updates the VFS accordingly.
    pub fn update_document(
        &mut self,
        uri: &url::Url,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Result<()> {
        let uri_str = uri.to_string();

        // Get document and file_id from the documents HashMap
        let document = self
            .documents
            .get(&uri_str)
            .ok_or_else(|| anyhow!("Document not found: {}", uri_str))?;
        let file_id = document.file_id();

        // Get current content from VFS
        let snapshot = self.vfs.snapshot();
        let current_content = snapshot
            .get_text(file_id)
            .ok_or_else(|| anyhow!("File content not found: {}", uri_str))?;

        // Get line index from the document
        let line_index = document.line_index();

        // Apply text changes using the existing function
        let new_content = apply_text_changes(&current_content, &changes, line_index)?;

        // Update TextDocument version and content
        if let Some(document) = self.documents.get_mut(&uri_str) {
            document.version = version;
            document.update_content(&new_content);
        }

        // Update VFS with new content
        self.vfs
            .set_overlay(file_id, Arc::from(new_content.as_str()))?;

        // Sync VFS snapshot to FileStore for Salsa tracking
        let snapshot = self.vfs.snapshot();
        let mut file_store = self.file_store.lock().unwrap();
        file_store.apply_vfs_snapshot(&snapshot);

        Ok(())
    }

    /// Close a document with the given URI.
    /// This removes the document from internal storage and cleans up resources.
    pub fn close_document(&mut self, uri: &url::Url) {
        let uri_str = uri.as_str();

        // Remove TextDocument metadata
        self.documents.remove(uri_str);

        // Note: We don't remove from VFS as it might be useful for caching
        // The VFS will handle cleanup internally
    }

    #[must_use]
    pub fn get_line_index(&self, uri: &str) -> Option<&LineIndex> {
        self.documents.get(uri).map(super::TextDocument::line_index)
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn get_version(&self, uri: &str) -> Option<i32> {
        self.documents.get(uri).map(super::TextDocument::version)
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn is_version_valid(&self, uri: &str, version: i32) -> bool {
        self.get_version(uri) == Some(version)
    }

    // TextDocument helper methods
    #[must_use]
    pub fn get_document(&self, uri: &str) -> Option<&TextDocument> {
        self.documents.get(uri)
    }

    pub fn get_document_mut(&mut self, uri: &str) -> Option<&mut TextDocument> {
        self.documents.get_mut(uri)
    }

    // URI-based query methods (new API)
    #[must_use]
    pub fn get_document_by_url(&self, uri: &url::Url) -> Option<&TextDocument> {
        self.get_document(uri.as_str())
    }

    #[must_use]
    pub fn get_document_text(&self, uri: &url::Url) -> Option<Arc<str>> {
        let document = self.get_document_by_url(uri)?;
        let file_id = document.file_id();
        let snapshot = self.vfs.snapshot();
        snapshot.get_text(file_id)
    }

    #[must_use]
    pub fn get_line_text(&self, uri: &url::Url, line: u32) -> Option<String> {
        let document = self.get_document_by_url(uri)?;
        let snapshot = self.vfs.snapshot();
        let content = snapshot.get_text(document.file_id())?;
        document.get_line(content.as_ref(), line)
    }

    #[must_use]
    pub fn get_word_at_position(&self, uri: &url::Url, position: Position) -> Option<String> {
        // This is a simplified implementation - get the line and extract word at position
        let line_text = self.get_line_text(uri, position.line)?;
        let char_pos: usize = position.character.try_into().ok()?;

        if char_pos >= line_text.len() {
            return None;
        }

        // Find word boundaries (simplified - considers alphanumeric and underscore as word chars)
        let line_bytes = line_text.as_bytes();
        let mut start = char_pos;
        let mut end = char_pos;

        // Find start of word
        while start > 0 && is_word_char(line_bytes[start - 1]) {
            start -= 1;
        }

        // Find end of word
        while end < line_text.len() && is_word_char(line_bytes[end]) {
            end += 1;
        }

        if start < end {
            Some(line_text[start..end].to_string())
        } else {
            None
        }
    }

    // Position mapping methods
    #[must_use]
    pub fn offset_to_position(&self, uri: &url::Url, offset: usize) -> Option<Position> {
        let document = self.get_document_by_url(uri)?;
        Some(document.offset_to_position(offset as u32))
    }

    #[must_use]
    pub fn position_to_offset(&self, uri: &url::Url, position: Position) -> Option<usize> {
        let document = self.get_document_by_url(uri)?;
        document
            .position_to_offset(position)
            .map(|offset| offset as usize)
    }

    // Template-specific methods
    #[must_use]
    pub fn get_template_ast(&self, uri: &url::Url) -> Option<Arc<TemplateAst>> {
        let document = self.get_document_by_url(uri)?;
        let file_id = document.file_id();
        let file_store = self.file_store.lock().unwrap();
        file_store.get_template_ast(file_id)
    }

    #[must_use]
    pub fn get_template_errors(&self, uri: &url::Url) -> Vec<String> {
        let Some(document) = self.get_document_by_url(uri) else {
            return vec![];
        };
        let file_id = document.file_id();
        let file_store = self.file_store.lock().unwrap();
        let errors = file_store.get_template_errors(file_id);
        errors.to_vec()
    }

    #[must_use]
    pub fn get_template_context(
        &self,
        uri: &url::Url,
        position: Position,
    ) -> Option<crate::TemplateTagContext> {
        let document = self.get_document_by_url(uri)?;
        let snapshot = self.vfs.snapshot();
        let content = snapshot.get_text(document.file_id())?;
        document.get_template_tag_context(content.as_ref(), position)
    }

    #[must_use]
    pub fn get_completions(
        &self,
        uri: &str,
        position: Position,
        tags: &TemplateTags,
    ) -> Option<CompletionResponse> {
        // Check if this is a Django template using TextDocument metadata
        let document = self.get_document(uri)?;
        if document.language_id() != LanguageId::HtmlDjango {
            return None;
        }

        // Try to get cached AST from FileStore for better context analysis
        // This demonstrates using the cached AST, though we still fall back to string parsing
        let file_id = document.file_id();
        let file_store = self.file_store.lock().unwrap();
        if let Some(_ast) = file_store.get_template_ast(file_id) {
            // TODO: In a future enhancement, we could use the AST to provide
            // more intelligent completions based on the current node context
            // For now, we continue with the existing string-based approach
        }

        // Get template tag context from document
        let vfs_snapshot = self.vfs.snapshot();
        let text_content = vfs_snapshot.get_text(file_id)?;
        let context = document.get_template_tag_context(text_content.as_ref(), position)?;

        let mut completions: Vec<CompletionItem> = tags
            .iter()
            .filter(|tag| {
                context.partial_tag.is_empty() || tag.name().starts_with(&context.partial_tag)
            })
            .map(|tag| {
                let leading_space = if context.needs_leading_space { " " } else { "" };
                CompletionItem {
                    label: tag.name().to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some(format!("Template tag from {}", tag.library())),
                    documentation: tag.doc().as_ref().map(|doc| {
                        Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: (*doc).to_string(),
                        })
                    }),
                    insert_text: Some(match context.closing_brace {
                        ClosingBrace::None => format!("{}{} %}}", leading_space, tag.name()),
                        ClosingBrace::PartialClose => format!("{}{} %", leading_space, tag.name()),
                        ClosingBrace::FullClose => format!("{}{} ", leading_space, tag.name()),
                    }),
                    insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                }
            })
            .collect();

        if completions.is_empty() {
            None
        } else {
            completions.sort_by(|a, b| a.label.cmp(&b.label));
            Some(CompletionResponse::Array(completions))
        }
    }

    /// Get template parsing diagnostics for a file.
    ///
    /// This method uses the cached template errors from Salsa to generate LSP diagnostics.
    /// The errors are only re-computed when the file content changes, providing efficient
    /// incremental error reporting.
    pub fn get_template_diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        let Some(document) = self.get_document(uri) else {
            return vec![];
        };

        // Only process template files
        if document.language_id() != LanguageId::HtmlDjango {
            return vec![];
        }

        let file_id = document.file_id();
        let Some(_line_index) = self.get_line_index(uri) else {
            return vec![];
        };

        // Get cached template errors from FileStore
        let file_store = self.file_store.lock().unwrap();
        let errors = file_store.get_template_errors(file_id);

        // Convert template errors to LSP diagnostics
        errors
            .iter()
            .map(|error| {
                // For now, we'll place all errors at the start of the file
                // In a future enhancement, we could use error spans for precise locations
                let range = Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: 0,
                    },
                };

                Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("djls-templates".to_string()),
                    message: error.clone(),
                    ..Default::default()
                }
            })
            .collect()
    }
}

/// Check if a byte represents a word character (alphanumeric or underscore)
fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Apply text changes to content, handling multiple changes correctly
fn apply_text_changes(
    content: &str,
    changes: &[TextDocumentContentChangeEvent],
    line_index: &LineIndex,
) -> Result<String> {
    if changes.is_empty() {
        return Ok(content.to_string());
    }

    // Check for full document replacement first
    for change in changes {
        if change.range.is_none() {
            return Ok(change.text.clone());
        }
    }

    // Sort changes by start position in reverse order (end to start)
    let mut sorted_changes = changes.to_vec();
    sorted_changes.sort_by(|a, b| {
        match (a.range, b.range) {
            (Some(range_a), Some(range_b)) => {
                // Primary sort: by line (reverse)
                let line_cmp = range_b.start.line.cmp(&range_a.start.line);
                if line_cmp == std::cmp::Ordering::Equal {
                    // Secondary sort: by character (reverse)
                    range_b.start.character.cmp(&range_a.start.character)
                } else {
                    line_cmp
                }
            }
            _ => std::cmp::Ordering::Equal,
        }
    });

    let mut result = content.to_string();

    for change in &sorted_changes {
        if let Some(range) = change.range {
            // Convert UTF-16 positions to UTF-8 offsets
            let start_offset = line_index
                .offset_utf16(range.start, &result)
                .ok_or_else(|| anyhow!("Invalid start position: {:?}", range.start))?;
            let end_offset = line_index
                .offset_utf16(range.end, &result)
                .ok_or_else(|| anyhow!("Invalid end position: {:?}", range.end))?;

            if start_offset as usize > result.len() || end_offset as usize > result.len() {
                return Err(anyhow!(
                    "Offset out of bounds: start={}, end={}, len={}",
                    start_offset,
                    end_offset,
                    result.len()
                ));
            }

            // Apply the change
            result.replace_range(start_offset as usize..end_offset as usize, &change.text);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::lsp_types::Range;

    use super::*;

    #[test]
    fn test_apply_single_character_insertion() {
        let content = "Hello world";
        let line_index = LineIndex::new(content);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 6), Position::new(0, 6))),
            range_length: None,
            text: "beautiful ".to_string(),
        }];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "Hello beautiful world");
    }

    #[test]
    fn test_apply_single_character_deletion() {
        let content = "Hello world";
        let line_index = LineIndex::new(content);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(0, 6))),
            range_length: None,
            text: String::new(),
        }];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "Helloworld");
    }

    #[test]
    fn test_apply_multiple_changes_in_reverse_order() {
        let content = "line 1\nline 2\nline 3";
        let line_index = LineIndex::new(content);

        // Insert "new " at position (1, 0) and "another " at position (0, 0)
        let changes = vec![
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 0), Position::new(0, 0))),
                range_length: None,
                text: "another ".to_string(),
            },
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(1, 0), Position::new(1, 0))),
                range_length: None,
                text: "new ".to_string(),
            },
        ];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "another line 1\nnew line 2\nline 3");
    }

    #[test]
    fn test_apply_multiline_replacement() {
        let content = "line 1\nline 2\nline 3";
        let line_index = LineIndex::new(content);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 0), Position::new(2, 6))),
            range_length: None,
            text: "completely new content".to_string(),
        }];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "completely new content");
    }

    #[test]
    fn test_apply_full_document_replacement() {
        let content = "old content";
        let line_index = LineIndex::new(content);

        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "brand new content".to_string(),
        }];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "brand new content");
    }

    #[test]
    fn test_utf16_line_index_basic() {
        let content = "hello world";
        let line_index = LineIndex::new(content);

        // ASCII characters should have 1:1 UTF-8:UTF-16 mapping
        let pos = Position::new(0, 6);
        let offset = line_index.offset_utf16(pos, content).unwrap();
        assert_eq!(offset, 6);
        assert_eq!(&content[6..7], "w");
    }

    #[test]
    fn test_utf16_line_index_with_emoji() {
        let content = "hello 👋 world";
        let line_index = LineIndex::new(content);

        // 👋 is 2 UTF-16 code units but 4 UTF-8 bytes
        let pos_after_emoji = Position::new(0, 8); // UTF-16 position after "hello 👋"
        let offset = line_index.offset_utf16(pos_after_emoji, content).unwrap();

        // Should point to the space before "world"
        assert_eq!(offset, 10); // UTF-8 byte offset
        assert_eq!(&content[10..11], " ");
    }

    #[test]
    fn test_utf16_line_index_multiline() {
        let content = "first line\nsecond line";
        let line_index = LineIndex::new(content);

        let pos = Position::new(1, 7); // Position at 'l' in "line" on second line
        let offset = line_index.offset_utf16(pos, content).unwrap();
        assert_eq!(offset, 18); // 11 (first line + \n) + 7
        assert_eq!(&content[18..19], "l");
    }

    #[test]
    fn test_apply_changes_with_emoji() {
        let content = "hello 👋 world";
        let line_index = LineIndex::new(content);

        // Insert text after the space following the emoji (UTF-16 position 9)
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 9), Position::new(0, 9))),
            range_length: None,
            text: "beautiful ".to_string(),
        }];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "hello 👋 beautiful world");
    }

    #[test]
    fn test_line_index_utf16_tracking() {
        let content = "a👋b";
        let line_index = LineIndex::new(content);

        // Check UTF-16 line starts are tracked correctly
        assert_eq!(line_index.line_starts_utf16, vec![0]);
        assert_eq!(line_index.length_utf16, 4); // 'a' (1) + 👋 (2) + 'b' (1) = 4 UTF-16 units
        assert_eq!(line_index.length, 6); // 'a' (1) + 👋 (4) + 'b' (1) = 6 UTF-8 bytes
    }

    #[test]
    fn test_edge_case_changes_at_boundaries() {
        let content = "abc";
        let line_index = LineIndex::new(content);

        // Insert at beginning
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 0), Position::new(0, 0))),
            range_length: None,
            text: "start".to_string(),
        }];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "startabc");

        // Insert at end
        let line_index = LineIndex::new(content);
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 3), Position::new(0, 3))),
            range_length: None,
            text: "end".to_string(),
        }];

        let result = apply_text_changes(content, &changes, &line_index).unwrap();
        assert_eq!(result, "abcend");
    }
}
