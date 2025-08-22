use salsa::Database;
use tower_lsp_server::lsp_types::DidOpenTextDocumentParams;
use tower_lsp_server::lsp_types::Position;
use tower_lsp_server::lsp_types::Range;
use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;

#[salsa::input(debug)]
pub struct TextDocument {
    #[returns(ref)]
    pub uri: String,
    #[returns(ref)]
    pub contents: String,
    #[returns(ref)]
    pub index: LineIndex,
    pub version: i32,
    pub language_id: LanguageId,
}

impl TextDocument {
    pub fn from_did_open_params(db: &dyn Database, params: &DidOpenTextDocumentParams) -> Self {
        let uri = params.text_document.uri.to_string();
        let contents = params.text_document.text.clone();
        let version = params.text_document.version;
        let language_id = LanguageId::from(params.text_document.language_id.as_str());

        let index = LineIndex::new(&contents);
        TextDocument::new(db, uri, contents, index, version, language_id)
    }

    pub fn with_changes(
        self,
        db: &dyn Database,
        changes: &[TextDocumentContentChangeEvent],
        new_version: i32,
    ) -> Self {
        let mut new_contents = self.contents(db).to_string();

        for change in changes {
            if let Some(range) = change.range {
                let index = LineIndex::new(&new_contents);

                if let (Some(start_offset), Some(end_offset)) = (
                    index.offset(range.start).map(|o| o as usize),
                    index.offset(range.end).map(|o| o as usize),
                ) {
                    let mut updated_content = String::with_capacity(
                        new_contents.len() - (end_offset - start_offset) + change.text.len(),
                    );

                    updated_content.push_str(&new_contents[..start_offset]);
                    updated_content.push_str(&change.text);
                    updated_content.push_str(&new_contents[end_offset..]);

                    new_contents = updated_content;
                }
            } else {
                // Full document update
                new_contents.clone_from(&change.text);
            }
        }

        let index = LineIndex::new(&new_contents);
        TextDocument::new(
            db,
            self.uri(db).to_string(),
            new_contents,
            index,
            new_version,
            self.language_id(db),
        )
    }

    #[allow(dead_code)]
    pub fn get_text(self, db: &dyn Database) -> String {
        self.contents(db).to_string()
    }

    #[allow(dead_code)]
    pub fn get_text_range(self, db: &dyn Database, range: Range) -> Option<String> {
        let index = self.index(db);
        let start = index.offset(range.start)? as usize;
        let end = index.offset(range.end)? as usize;
        let contents = self.contents(db);
        Some(contents[start..end].to_string())
    }

    pub fn get_line(self, db: &dyn Database, line: u32) -> Option<String> {
        let index = self.index(db);
        let start = index.line_starts.get(line as usize)?;
        let end = index
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(index.length);

        let contents = self.contents(db);
        Some(contents[*start as usize..end as usize].to_string())
    }

    #[allow(dead_code)]
    pub fn line_count(self, db: &dyn Database) -> usize {
        self.index(db).line_starts.len()
    }

    pub fn get_template_tag_context(
        self,
        db: &dyn Database,
        position: Position,
    ) -> Option<TemplateTagContext> {
        let line = self.get_line(db, position.line)?;
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
            pos += u32::try_from(c.len_utf8()).unwrap_or(0);
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

    #[allow(dead_code)]
    pub fn position(&self, offset: u32) -> Position {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(line) => line - 1,
        };

        let line_start = self.line_starts[line];
        let character = offset - line_start;

        Position::new(u32::try_from(line).unwrap_or(0), character)
    }

    /// Get line start offset for a given line number
    pub fn line_start(&self, line: usize) -> Option<u32> {
        self.line_starts.get(line).copied()
    }

    /// Get total content length
    pub fn length(&self) -> u32 {
        self.length
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
            "django-html" | "htmldjango" => Self::HtmlDjango,
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
