use super::fs::FileSystem;
use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::anyhow;
use anyhow::Result;
use djls_project::TemplateTags;
use salsa::Database;
use tower_lsp_server::lsp_types::CompletionItem;
use tower_lsp_server::lsp_types::CompletionItemKind;
use tower_lsp_server::lsp_types::CompletionResponse;
use tower_lsp_server::lsp_types::DidChangeTextDocumentParams;
use tower_lsp_server::lsp_types::DidCloseTextDocumentParams;
use tower_lsp_server::lsp_types::DidOpenTextDocumentParams;
use tower_lsp_server::lsp_types::DidSaveTextDocumentParams;
use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;
use tower_lsp_server::lsp_types::Documentation;
use tower_lsp_server::lsp_types::InsertTextFormat;
use tower_lsp_server::lsp_types::MarkupContent;
use tower_lsp_server::lsp_types::MarkupKind;
use tower_lsp_server::lsp_types::Position;

use super::document::ClosingBrace;
use super::document::LanguageId;
use super::document::LineIndex;
use super::document::TextDocument;
use super::utils::uri_to_pathbuf;

#[derive(Debug)]
pub struct Store {
    documents: HashMap<String, TextDocument>,
    versions: HashMap<String, i32>,
    vfs: FileSystem,
    root_path: PathBuf,
}

impl Store {
    pub fn new<P: AsRef<std::path::Path>>(root_path: P) -> anyhow::Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        let vfs = FileSystem::new(&root_path);

        Ok(Store {
            documents: HashMap::new(),
            versions: HashMap::new(),
            vfs,
            root_path,
        })
    }
    
    /// Check if a URI represents a file within the workspace
    fn is_workspace_file(&self, uri: &tower_lsp_server::lsp_types::Uri) -> bool {
        if let Some(path) = uri_to_pathbuf(uri) {
            // Check if the path is under the workspace root
            path.starts_with(&self.root_path)
        } else {
            // Not a file URI, ignore
            false
        }
    }
    pub fn handle_did_open(&mut self, db: &dyn Database, params: &DidOpenTextDocumentParams) {
        // Only process files within the workspace
        if !self.is_workspace_file(&params.text_document.uri) {
            // Silently ignore files outside workspace
            return;
        }
        
        let uri = params.text_document.uri.to_string();
        let version = params.text_document.version;
        let content = &params.text_document.text;

        // Convert URI to relative path for VFS
        if let Some(absolute_path) = uri_to_pathbuf(&params.text_document.uri) {
            // Make path relative to workspace root
            if let Ok(relative_path) = absolute_path.strip_prefix(&self.root_path) {
                // Write content to FileSystem (memory layer for opened file)
                if let Err(e) = self.vfs.write_string(
                    &relative_path.to_string_lossy(),
                    content
                ) {
                    eprintln!("Warning: Failed to write file to VFS: {}", e);
                    // Continue with normal processing despite VFS error
                }
            }
        }

        let document = TextDocument::from_did_open_params(db, params);

        self.add_document(document, uri.clone());
        self.versions.insert(uri, version);
    }

    pub fn handle_did_change(
        &mut self,
        db: &dyn Database,
        params: &DidChangeTextDocumentParams,
    ) -> Result<()> {
        // Only process files within the workspace
        if !self.is_workspace_file(&params.text_document.uri) {
            // Return Ok to avoid errors for files outside workspace
            return Ok(());
        }
        
        let uri = params.text_document.uri.as_str().to_string();
        let version = params.text_document.version;

        // Convert URI to relative path for VFS
        if let Some(absolute_path) = uri_to_pathbuf(&params.text_document.uri) {
            if let Ok(relative_path) = absolute_path.strip_prefix(&self.root_path) {
                let relative_path_str = relative_path.to_string_lossy();
                
                // Read current content from VFS
                let current_content = match self.vfs.read_to_string(&relative_path_str) {
                    Ok(content) => content,
                    Err(e) => {
                        eprintln!("Warning: Failed to read from VFS, falling back to TextDocument: {}", e);
                        // Fallback to existing TextDocument approach
                        let document = self
                            .get_document(&uri)
                            .ok_or_else(|| anyhow!("Document not found: {}", uri))?;
                        let new_document = document.with_changes(db, &params.content_changes, version);
                        self.documents.insert(uri.clone(), new_document);
                        self.versions.insert(uri, version);
                        return Ok(());
                    }
                };

                // Apply text changes to VFS content
                let updated_content = self.apply_changes_to_content(current_content, &params.content_changes)?;

                // Write updated content back to VFS
                if let Err(e) = self.vfs.write_string(&relative_path_str, &updated_content) {
                    eprintln!("Warning: Failed to write to VFS: {}", e);
                }

                // Create new TextDocument with updated content for backward compatibility
                let index = LineIndex::new(&updated_content);
                let language_id = self.get_document(&uri)
                    .map(|doc| doc.language_id(db))
                    .unwrap_or(LanguageId::HtmlDjango);
                
                let new_document = TextDocument::new(db, uri.clone(), updated_content.clone(), index, version, language_id);
                self.documents.insert(uri.clone(), new_document);
                self.versions.insert(uri, version);

                return Ok(());
            }
        }

        // Fallback to original implementation if path conversion fails
        let document = self
            .get_document(&uri)
            .ok_or_else(|| anyhow!("Document not found: {}", uri))?;

        let new_document = document.with_changes(db, &params.content_changes, version);

        self.documents.insert(uri.clone(), new_document);
        self.versions.insert(uri, version);

        Ok(())
    }

    pub fn handle_did_close(&mut self, params: &DidCloseTextDocumentParams) {
        // Only process files within the workspace for VFS cleanup
        if self.is_workspace_file(&params.text_document.uri) {
            // Convert URI to relative path for VFS
            if let Some(absolute_path) = uri_to_pathbuf(&params.text_document.uri) {
                if let Ok(relative_path) = absolute_path.strip_prefix(&self.root_path) {
                    let relative_path_str = relative_path.to_string_lossy();
                    
                    // Discard any unsaved changes in VFS (clean up memory layer)
                    if let Err(e) = self.vfs.discard_changes(&relative_path_str) {
                        eprintln!("Warning: Failed to discard VFS changes on close: {}", e);
                        // Continue with document removal despite VFS error
                    }
                }
            }
        }

        // Remove document from Store tracking (always do this regardless of VFS status)
        self.remove_document(params.text_document.uri.as_str());
    }

    pub fn handle_did_save(&mut self, params: &DidSaveTextDocumentParams) -> Result<()> {
        // Only process files within the workspace
        if !self.is_workspace_file(&params.text_document.uri) {
            // Return Ok to avoid errors for files outside workspace
            return Ok(());
        }

        // Convert URI to relative path for VFS
        if let Some(absolute_path) = uri_to_pathbuf(&params.text_document.uri) {
            if let Ok(relative_path) = absolute_path.strip_prefix(&self.root_path) {
                let relative_path_str = relative_path.to_string_lossy();
                
                // Discard changes in VFS (clear memory layer so reads return disk content)
                if let Err(e) = self.vfs.discard_changes(&relative_path_str) {
                    eprintln!("Warning: Failed to discard VFS changes on save: {}", e);
                    // Continue normally - this is not a critical error
                }
            }
        }

        Ok(())
    }

    /// Apply text changes to content (similar to TextDocument::with_changes but for strings)
    fn apply_changes_to_content(
        &self,
        mut content: String,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<String> {
        for change in changes {
            if let Some(range) = change.range {
                // Incremental change with range
                let index = LineIndex::new(&content);
                
                if let (Some(start_offset), Some(end_offset)) = (
                    index.offset(range.start).map(|o| o as usize),
                    index.offset(range.end).map(|o| o as usize),
                ) {
                    let mut updated_content = String::with_capacity(
                        content.len() - (end_offset - start_offset) + change.text.len(),
                    );

                    updated_content.push_str(&content[..start_offset]);
                    updated_content.push_str(&change.text);
                    updated_content.push_str(&content[end_offset..]);

                    content = updated_content;
                } else {
                    return Err(anyhow!("Invalid range in text change"));
                }
            } else {
                // Full document replacement
                content.clone_from(&change.text);
            }
        }
        Ok(content)
    }

    fn add_document(&mut self, document: TextDocument, uri: String) {
        self.documents.insert(uri, document);
    }

    fn remove_document(&mut self, uri: &str) {
        self.documents.remove(uri);
        self.versions.remove(uri);
    }

    fn get_document(&self, uri: &str) -> Option<&TextDocument> {
        self.documents.get(uri)
    }

    #[allow(dead_code)]
    fn get_document_mut(&mut self, uri: &str) -> Option<&mut TextDocument> {
        self.documents.get_mut(uri)
    }

    #[allow(dead_code)]
    pub fn get_all_documents(&self) -> impl Iterator<Item = &TextDocument> {
        self.documents.values()
    }

    #[allow(dead_code)]
    pub fn get_documents_by_language<'db>(
        &'db self,
        db: &'db dyn Database,
        language_id: LanguageId,
    ) -> impl Iterator<Item = &'db TextDocument> + 'db {
        self.documents
            .values()
            .filter(move |doc| doc.language_id(db) == language_id)
    }

    #[allow(dead_code)]
    pub fn get_version(&self, uri: &str) -> Option<i32> {
        self.versions.get(uri).copied()
    }

    #[allow(dead_code)]
    pub fn is_version_valid(&self, uri: &str, version: i32) -> bool {
        self.get_version(uri) == Some(version)
    }

    pub fn get_completions(
        &self,
        db: &dyn Database,
        uri: &str,
        position: Position,
        tags: &TemplateTags,
    ) -> Option<CompletionResponse> {
        let document = self.get_document(uri)?;

        if document.language_id(db) != LanguageId::HtmlDjango {
            return None;
        }

        let context = document.get_template_tag_context(db, position)?;

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
