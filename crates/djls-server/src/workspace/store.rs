use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use anyhow::Result;
use camino::Utf8PathBuf;
use djls_project::TemplateTags;
use djls_workspace::{FileId, FileKind, TextSource, Vfs};
use tower_lsp_server::lsp_types::CompletionItem;
use tower_lsp_server::lsp_types::CompletionItemKind;
use tower_lsp_server::lsp_types::CompletionResponse;
use tower_lsp_server::lsp_types::DidChangeTextDocumentParams;
use tower_lsp_server::lsp_types::DidCloseTextDocumentParams;
use tower_lsp_server::lsp_types::DidOpenTextDocumentParams;
use tower_lsp_server::lsp_types::Documentation;
use tower_lsp_server::lsp_types::InsertTextFormat;
use tower_lsp_server::lsp_types::MarkupContent;
use tower_lsp_server::lsp_types::MarkupKind;
use tower_lsp_server::lsp_types::Position;

use super::document::{ClosingBrace, LanguageId, LineIndex, TextDocument};

pub struct Store {
    vfs: Arc<Vfs>,
    file_ids: HashMap<String, FileId>,
    line_indices: HashMap<FileId, LineIndex>,
    versions: HashMap<String, i32>,
    documents: HashMap<String, TextDocument>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            vfs: Arc::new(Vfs::default()),
            file_ids: HashMap::new(),
            line_indices: HashMap::new(),
            versions: HashMap::new(),
            documents: HashMap::new(),
        }
    }
}

impl Store {
    pub fn handle_did_open(&mut self, params: &DidOpenTextDocumentParams) -> Result<()> {
        let uri_str = params.text_document.uri.to_string();
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let content = params.text_document.text.clone();
        let language_id = LanguageId::from(params.text_document.language_id.as_str());
        let kind = FileKind::from(language_id.clone());

        // Convert URI to Url for VFS
        let vfs_url =
            url::Url::parse(&uri.to_string()).map_err(|e| anyhow!("Invalid URI: {}", e))?;

        // Convert to path - simplified for now, just use URI string
        let path = Utf8PathBuf::from(uri.as_str());

        // Store content in VFS
        let text_source = TextSource::Overlay(Arc::from(content.as_str()));
        let file_id = self.vfs.intern_file(vfs_url, path, kind, text_source);

        // Set overlay content in VFS
        self.vfs.set_overlay(file_id, Arc::from(content.as_str()))?;

        // Create TextDocument metadata
        let document = TextDocument::new(uri_str.clone(), version, language_id.clone(), file_id);
        self.documents.insert(uri_str.clone(), document);

        // Cache mappings and indices
        self.file_ids.insert(uri_str.clone(), file_id);
        self.line_indices.insert(file_id, LineIndex::new(&content));
        self.versions.insert(uri_str, version);

        Ok(())
    }

    pub fn handle_did_change(&mut self, params: &DidChangeTextDocumentParams) -> Result<()> {
        let uri_str = params.text_document.uri.as_str().to_string();
        let version = params.text_document.version;

        // Look up FileId
        let file_id = self
            .file_ids
            .get(&uri_str)
            .copied()
            .ok_or_else(|| anyhow!("Document not found: {}", uri_str))?;

        // Get current content from VFS
        let snapshot = self.vfs.snapshot();
        let current_content = snapshot
            .get_text(file_id)
            .ok_or_else(|| anyhow!("File content not found: {}", uri_str))?;

        // Apply text changes
        let mut new_content = current_content.to_string();
        for change in &params.content_changes {
            if let Some(range) = change.range {
                // Get current line index for position calculations
                let line_index = self
                    .line_indices
                    .get(&file_id)
                    .ok_or_else(|| anyhow!("Line index not found for: {}", uri_str))?;

                if let (Some(start_offset), Some(end_offset)) = (
                    line_index.offset(range.start).map(|o| o as usize),
                    line_index.offset(range.end).map(|o| o as usize),
                ) {
                    let mut updated_content = String::with_capacity(
                        new_content.len() - (end_offset - start_offset) + change.text.len(),
                    );

                    updated_content.push_str(&new_content[..start_offset]);
                    updated_content.push_str(&change.text);
                    updated_content.push_str(&new_content[end_offset..]);

                    new_content = updated_content;
                }
            } else {
                // Full document update
                new_content.clone_from(&change.text);
            }
        }

        // Update TextDocument version
        if let Some(document) = self.documents.get_mut(&uri_str) {
            document.version = version;
        }

        // Update VFS with new content
        self.vfs
            .set_overlay(file_id, Arc::from(new_content.as_str()))?;

        // Update cached line index and version
        self.line_indices
            .insert(file_id, LineIndex::new(&new_content));
        self.versions.insert(uri_str, version);

        Ok(())
    }

    pub fn handle_did_close(&mut self, params: &DidCloseTextDocumentParams) {
        let uri_str = params.text_document.uri.as_str();

        // Remove TextDocument metadata
        self.documents.remove(uri_str);

        // Look up FileId and remove mappings
        if let Some(file_id) = self.file_ids.remove(uri_str) {
            self.line_indices.remove(&file_id);
        }
        self.versions.remove(uri_str);

        // Note: We don't remove from VFS as it might be useful for caching
        // The VFS will handle cleanup internally
    }

    pub fn get_file_id(&self, uri: &str) -> Option<FileId> {
        self.file_ids.get(uri).copied()
    }

    pub fn get_line_index(&self, file_id: FileId) -> Option<&LineIndex> {
        self.line_indices.get(&file_id)
    }

    #[allow(dead_code)]
    pub fn get_version(&self, uri: &str) -> Option<i32> {
        self.versions.get(uri).copied()
    }

    #[allow(dead_code)]
    pub fn is_version_valid(&self, uri: &str, version: i32) -> bool {
        self.get_version(uri) == Some(version)
    }

    // TextDocument helper methods
    pub fn get_document(&self, uri: &str) -> Option<&TextDocument> {
        self.documents.get(uri)
    }

    pub fn get_document_mut(&mut self, uri: &str) -> Option<&mut TextDocument> {
        self.documents.get_mut(uri)
    }

    pub fn get_completions(
        &self,
        uri: &str,
        position: Position,
        tags: &TemplateTags,
    ) -> Option<CompletionResponse> {
        // Check if this is a Django template using TextDocument metadata
        let document = self.get_document(uri)?;
        if document.language_id != LanguageId::HtmlDjango {
            return None;
        }

        // Get template tag context from document
        let vfs_snapshot = self.vfs.snapshot();
        let line_index = self.get_line_index(document.file_id())?;
        let context = document.get_template_tag_context(&vfs_snapshot, line_index, position)?;

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
}
