use anyhow::{anyhow, Result};
use std::collections::HashMap;
use tower_lsp::lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Position,
    Range,
};
use tracing::{debug, error, info, instrument, warn};

#[derive(Debug)]
pub struct Store {
    documents: HashMap<String, TextDocument>,
    versions: HashMap<String, i32>,
}

impl Store {
    pub fn new() -> Self {
        debug!("Creating new document store");
        Self {
            documents: HashMap::new(),
            versions: HashMap::new(),
        }
    }

    #[instrument(skip(self, params), fields(uri = %params.text_document.uri))]
    pub fn handle_did_open(&mut self, params: DidOpenTextDocumentParams) -> Result<()> {
        let document = TextDocument::new(
            String::from(params.text_document.uri.clone()),
            params.text_document.text.clone(),
            params.text_document.version,
            params.text_document.language_id.clone(),
        );

        info!(
            version = params.text_document.version,
            language = %params.text_document.language_id,
            "Opening document"
        );

        self.add_document(document);
        debug!("Document added to store");

        Ok(())
    }

    #[instrument(skip(self, params), fields(uri = %params.text_document.uri))]
    pub fn handle_did_change(&mut self, params: DidChangeTextDocumentParams) -> Result<()> {
        let uri = params.text_document.uri.as_str().to_string();
        let version = params.text_document.version;
        let changes = params.content_changes.len();

        debug!(changes, version, "Processing document changes");

        let document = self.get_document_mut(&uri).ok_or_else(|| {
            error!(uri, "Document not found");
            anyhow!("Document not found: {}", uri)
        })?;

        for (i, change) in params.content_changes.iter().enumerate() {
            if let Some(range) = change.range {
                debug!(
                    change_index = i,
                    start_line = range.start.line,
                    start_char = range.start.character,
                    end_line = range.end.line,
                    end_char = range.end.character,
                    "Applying incremental change"
                );
                document.apply_change(range, &change.text)?;
            } else {
                debug!(change_index = i, "Applying full document update");
                document.set_content(change.text.clone());
            }
        }

        document.version = version;
        self.versions.insert(uri, version);
        debug!(version, "Document version updated");

        Ok(())
    }

    #[instrument(skip(self, params), fields(uri = %params.text_document.uri))]
    pub fn handle_did_close(&mut self, params: DidCloseTextDocumentParams) -> Result<()> {
        info!("Closing document");
        self.remove_document(&String::from(params.text_document.uri));
        debug!("Document removed from store");
        Ok(())
    }

    #[instrument(skip(self))]
    fn add_document(&mut self, document: TextDocument) {
        let uri = document.uri.clone();
        let version = document.version;
        let language = &document.language_id;

        debug!(%uri, version, ?language, "Adding document to store");
        self.documents.insert(uri.clone(), document);
        self.versions.insert(uri, version);
    }

    #[instrument(skip(self))]
    fn remove_document(&mut self, uri: &str) {
        debug!(%uri, "Removing document from store");
        self.documents.remove(uri);
        self.versions.remove(uri);
    }

    fn get_document(&self, uri: &str) -> Option<&TextDocument> {
        self.documents.get(uri)
    }

    fn get_document_mut(&mut self, uri: &str) -> Option<&mut TextDocument> {
        self.documents.get_mut(uri)
    }

    pub fn get_all_documents(&self) -> impl Iterator<Item = &TextDocument> {
        self.documents.values()
    }

    pub fn get_documents_by_language(
        &self,
        language_id: LanguageId,
    ) -> impl Iterator<Item = &TextDocument> {
        self.documents
            .values()
            .filter(move |doc| doc.language_id == language_id)
    }

    pub fn get_version(&self, uri: &str) -> Option<i32> {
        self.versions.get(uri).copied()
    }

    pub fn is_version_valid(&self, uri: &str, version: i32) -> bool {
        self.get_version(uri).map_or(false, |v| v == version)
    }
}

#[derive(Clone, Debug)]
pub struct TextDocument {
    uri: String,
    contents: String,
    index: LineIndex,
    version: i32,
    language_id: LanguageId,
}

impl TextDocument {
    #[instrument(skip(contents))]
    fn new(uri: String, contents: String, version: i32, language_id: String) -> Self {
        debug!(%uri, version, %language_id, content_length = contents.len(), "Creating new text document");
        let index = LineIndex::new(&contents);
        Self {
            uri,
            contents,
            index,
            version,
            language_id: LanguageId::from(language_id),
        }
    }

    #[instrument(skip(self, new_text), fields(uri = %self.uri))]
    pub fn apply_change(&mut self, range: Range, new_text: &str) -> Result<()> {
        debug!(
            start_line = range.start.line,
            start_char = range.start.character,
            end_line = range.end.line,
            end_char = range.end.character,
            new_text_length = new_text.len(),
            "Applying change to document"
        );

        let start_offset = self.index.offset(range.start).ok_or_else(|| {
            let e = anyhow!("Invalid start position: {:?}", range.start);
            error!(?range.start, "Invalid start position");
            e
        })? as usize;

        let end_offset = self.index.offset(range.end).ok_or_else(|| {
            let e = anyhow!("Invalid end position: {:?}", range.end);
            error!(?range.end, "Invalid end position");
            e
        })? as usize;

        let mut new_content = String::with_capacity(
            self.contents.len() - (end_offset - start_offset) + new_text.len(),
        );

        new_content.push_str(&self.contents[..start_offset]);
        new_content.push_str(new_text);
        new_content.push_str(&self.contents[end_offset..]);

        debug!(
            old_length = self.contents.len(),
            new_length = new_content.len(),
            "Updating document content"
        );

        self.set_content(new_content);
        Ok(())
    }

    pub fn set_content(&mut self, new_content: String) {
        self.contents = new_content;
        self.index = LineIndex::new(&self.contents);
    }

    pub fn get_text(&self) -> &str {
        &self.contents
    }

    pub fn get_text_range(&self, range: Range) -> Option<&str> {
        let start = self.index.offset(range.start)? as usize;
        let end = self.index.offset(range.end)? as usize;

        Some(&self.contents[start..end])
    }

    pub fn get_line(&self, line: u32) -> Option<&str> {
        let start = self.index.line_starts.get(line as usize)?;
        let end = self
            .index
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.index.length);

        Some(&self.contents[*start as usize..end as usize])
    }

    pub fn line_count(&self) -> usize {
        self.index.line_starts.len()
    }
}

#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    length: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        let mut pos = 0;

        for c in text.chars() {
            pos += c.len_utf8() as u32;
            if c == '\n' {
                line_starts.push(pos);
            }
        }

        Self {
            line_starts,
            length: pos,
        }
    }

    pub fn offset(&self, position: Position) -> Option<u32> {
        let line_start = self.line_starts.get(position.line as usize)?;

        Some(line_start + position.character)
    }

    pub fn position(&self, offset: u32) -> Position {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(line) => line - 1,
        };

        let line_start = self.line_starts[line];
        let character = offset - line_start;

        Position::new(line as u32, character)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LanguageId {
    HtmlDjango,
    Other,
    Python,
}

impl From<&str> for LanguageId {
    fn from(language_id: &str) -> Self {
        match language_id {
            "htmldjango" => Self::HtmlDjango,
            "python" => Self::Python,
            _ => Self::Other,
        }
    }
}

impl From<String> for LanguageId {
    fn from(language_id: String) -> Self {
        Self::from(language_id.as_str())
    }
}
