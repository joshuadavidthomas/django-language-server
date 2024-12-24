use anyhow::{anyhow, Result};
use djls_project::TemplateTags;
use std::collections::HashMap;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, Documentation, InsertTextFormat,
    MarkupContent, MarkupKind, Position, Range,
};

#[derive(Debug)]
pub struct Store {
    documents: HashMap<String, TextDocument>,
    versions: HashMap<String, i32>,
}

impl Store {
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
            versions: HashMap::new(),
        }
    }

    pub fn handle_did_open(&mut self, params: DidOpenTextDocumentParams) -> Result<()> {
        let document = TextDocument::new(
            String::from(params.text_document.uri),
            params.text_document.text,
            params.text_document.version,
            params.text_document.language_id,
        );

        self.add_document(document);

        Ok(())
    }

    pub fn handle_did_change(&mut self, params: DidChangeTextDocumentParams) -> Result<()> {
        let uri = params.text_document.uri.as_str().to_string();
        let version = params.text_document.version;

        let document = self
            .get_document_mut(&uri)
            .ok_or_else(|| anyhow!("Document not found: {}", uri))?;

        for change in params.content_changes {
            if let Some(range) = change.range {
                document.apply_change(range, &change.text)?;
            } else {
                // Full document update
                document.set_content(change.text);
            }
        }

        document.version = version;
        self.versions.insert(uri, version);

        Ok(())
    }

    pub fn handle_did_close(&mut self, params: DidCloseTextDocumentParams) -> Result<()> {
        self.remove_document(&String::from(params.text_document.uri));

        Ok(())
    }

    fn add_document(&mut self, document: TextDocument) {
        let uri = document.uri.clone();
        let version = document.version;

        self.documents.insert(uri.clone(), document);
        self.versions.insert(uri, version);
    }

    fn remove_document(&mut self, uri: &str) {
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

    pub fn get_completions(
        &self,
        uri: &str,
        position: Position,
        tags: &TemplateTags,
    ) -> Option<CompletionResponse> {
        let document = self.get_document(uri)?;

        if document.language_id != LanguageId::HtmlDjango {
            return None;
        }

        let context = document.get_template_tag_context(position)?;

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
                            value: doc.to_string(),
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

#[derive(Clone, Debug)]
pub struct TextDocument {
    uri: String,
    contents: String,
    index: LineIndex,
    version: i32,
    language_id: LanguageId,
}

impl TextDocument {
    fn new(uri: String, contents: String, version: i32, language_id: String) -> Self {
        let index = LineIndex::new(&contents);
        Self {
            uri,
            contents,
            index,
            version,
            language_id: LanguageId::from(language_id),
        }
    }

    pub fn apply_change(&mut self, range: Range, new_text: &str) -> Result<()> {
        let start_offset = self
            .index
            .offset(range.start)
            .ok_or_else(|| anyhow!("Invalid start position: {:?}", range.start))?
            as usize;
        let end_offset = self
            .index
            .offset(range.end)
            .ok_or_else(|| anyhow!("Invalid end position: {:?}", range.end))?
            as usize;

        let mut new_content = String::with_capacity(
            self.contents.len() - (end_offset - start_offset) + new_text.len(),
        );

        new_content.push_str(&self.contents[..start_offset]);
        new_content.push_str(new_text);
        new_content.push_str(&self.contents[end_offset..]);

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

    pub fn get_template_tag_context(&self, position: Position) -> Option<TemplateTagContext> {
        let line = self.get_line(position.line.try_into().ok()?)?;
        let prefix = &line[..position.character.try_into().ok()?];
        let rest_of_line = &line[position.character.try_into().ok()?..];
        let rest_trimmed = rest_of_line.trim_start();

        prefix.rfind("{%").map(|tag_start| {
            // Check if we're immediately after {% with no space
            let needs_leading_space = prefix.ends_with("{%");

            let closing_brace = if rest_trimmed.starts_with("%}") {
                ClosingBrace::FullClose
            } else if rest_trimmed.starts_with("}") {
                ClosingBrace::PartialClose
            } else {
                ClosingBrace::None
            };

            TemplateTagContext {
                partial_tag: prefix[tag_start + 2..].trim().to_string(),
                closing_brace,
                needs_leading_space,
            }
        })
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

#[derive(Debug)]
pub enum ClosingBrace {
    None,
    PartialClose, // just }
    FullClose,    // %}
}

#[derive(Debug)]
pub struct TemplateTagContext {
    pub partial_tag: String,
    pub closing_brace: ClosingBrace,
    pub needs_leading_space: bool,
}
