use std::sync::Arc;
use tower_lsp_server::lsp_types::{Position, Range};
use djls_workspace::{FileId, VfsSnapshot};

/// Document metadata container - no longer a Salsa input, just plain data
#[derive(Clone, Debug)]
pub struct TextDocument {
    pub uri: String,
    pub version: i32,
    pub language_id: LanguageId,
    file_id: FileId,
}

impl TextDocument {
    pub fn new(uri: String, version: i32, language_id: LanguageId, file_id: FileId) -> Self {
        Self {
            uri,
            version,
            language_id,
            file_id,
        }
    }
    
    pub fn file_id(&self) -> FileId {
        self.file_id
    }
    
    pub fn get_content(&self, vfs: &VfsSnapshot) -> Option<Arc<str>> {
        vfs.get_text(self.file_id)
    }
    
    pub fn get_line(&self, vfs: &VfsSnapshot, line_index: &LineIndex, line: u32) -> Option<String> {
        let content = self.get_content(vfs)?;
        
        let line_start = *line_index.line_starts.get(line as usize)?;
        let line_end = line_index.line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(line_index.length);
        
        Some(content[line_start as usize..line_end as usize].to_string())
    }
    
    pub fn get_text_range(&self, vfs: &VfsSnapshot, line_index: &LineIndex, range: Range) -> Option<String> {
        let content = self.get_content(vfs)?;
        
        let start_offset = line_index.offset(range.start)? as usize;
        let end_offset = line_index.offset(range.end)? as usize;
        
        Some(content[start_offset..end_offset].to_string())
    }
    
    pub fn get_template_tag_context(&self, vfs: &VfsSnapshot, line_index: &LineIndex, position: Position) -> Option<TemplateTagContext> {
        let content = self.get_content(vfs)?;
        
        let start = line_index.line_starts.get(position.line as usize)?;
        let end = line_index
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(line_index.length);

        let line = &content[*start as usize..end as usize];
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
    pub line_starts: Vec<u32>,
    pub length: u32,
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

impl From<LanguageId> for djls_workspace::FileKind {
    fn from(language_id: LanguageId) -> Self {
        match language_id {
            LanguageId::Python => Self::Python,
            LanguageId::HtmlDjango => Self::Template,
            LanguageId::Other => Self::Other,
        }
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

