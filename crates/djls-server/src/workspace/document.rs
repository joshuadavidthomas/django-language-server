use tower_lsp_server::lsp_types::DidOpenTextDocumentParams;
use tower_lsp_server::lsp_types::Position;

#[derive(Debug, Clone)]
pub struct TextDocument {
    #[allow(dead_code)]
    pub uri: String,
    pub version: i32,
    pub language_id: LanguageId,
}

impl TextDocument {
    pub fn new(uri: String, version: i32, language_id: LanguageId) -> Self {
        Self {
            uri,
            version,
            language_id,
        }
    }

    pub fn from_did_open_params(params: &DidOpenTextDocumentParams) -> Self {
        let uri = params.text_document.uri.to_string();
        let version = params.text_document.version;
        let language_id = LanguageId::from(params.text_document.language_id.as_str());

        TextDocument::new(uri, version, language_id)
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
