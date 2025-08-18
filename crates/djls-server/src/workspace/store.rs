use std::collections::HashMap;

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
use tower_lsp_server::lsp_types::Documentation;
use tower_lsp_server::lsp_types::InsertTextFormat;
use tower_lsp_server::lsp_types::MarkupContent;
use tower_lsp_server::lsp_types::MarkupKind;
use tower_lsp_server::lsp_types::Position;

use super::document::ClosingBrace;
use super::document::LanguageId;
use super::document::TextDocument;

#[derive(Debug, Default)]
pub struct Store {
    documents: HashMap<String, TextDocument>,
    versions: HashMap<String, i32>,
}

impl Store {
    pub fn handle_did_open(&mut self, db: &dyn Database, params: &DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let version = params.text_document.version;

        let document = TextDocument::from_did_open_params(db, params);

        self.add_document(document, uri.clone());
        self.versions.insert(uri, version);
    }

    pub fn handle_did_change(
        &mut self,
        db: &dyn Database,
        params: &DidChangeTextDocumentParams,
    ) -> Result<()> {
        let uri = params.text_document.uri.as_str().to_string();
        let version = params.text_document.version;

        let document = self
            .get_document(&uri)
            .ok_or_else(|| anyhow!("Document not found: {}", uri))?;

        let new_document = document.with_changes(db, &params.content_changes, version);

        self.documents.insert(uri.clone(), new_document);
        self.versions.insert(uri, version);

        Ok(())
    }

    pub fn handle_did_close(&mut self, params: &DidCloseTextDocumentParams) {
        self.remove_document(params.text_document.uri.as_str());
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
