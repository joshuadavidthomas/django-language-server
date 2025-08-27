mod language;
mod line_index;
mod template;

pub use language::LanguageId;
pub use line_index::LineIndex;
pub use template::ClosingBrace;
pub use template::TemplateTagContext;
use tower_lsp_server::lsp_types::Position;
use tower_lsp_server::lsp_types::Range;

use crate::FileId;

#[derive(Clone, Debug)]
pub struct TextDocument {
    pub uri: String,
    pub version: i32,
    pub language_id: LanguageId,
    pub(crate) file_id: FileId,
    line_index: LineIndex,
}

impl TextDocument {
    pub(crate) fn new(
        uri: String,
        version: i32,
        language_id: LanguageId,
        file_id: FileId,
        content: &str,
    ) -> Self {
        let line_index = LineIndex::new(content);
        Self {
            uri,
            version,
            language_id,
            file_id,
            line_index,
        }
    }

    pub(crate) fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    pub fn get_content<'a>(&self, content: &'a str) -> &'a str {
        content
    }

    pub fn get_line(&self, content: &str, line: u32) -> Option<String> {
        let line_start = *self.line_index.line_starts.get(line as usize)?;
        let line_end = self
            .line_index
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.line_index.length);

        Some(content[line_start as usize..line_end as usize].to_string())
    }

    pub fn get_text_range(&self, content: &str, range: Range) -> Option<String> {
        let start_offset = self.line_index.offset(range.start)? as usize;
        let end_offset = self.line_index.offset(range.end)? as usize;

        Some(content[start_offset..end_offset].to_string())
    }

    pub fn get_template_tag_context(
        &self,
        content: &str,
        position: Position,
    ) -> Option<TemplateTagContext> {
        let start = self.line_index.line_starts.get(position.line as usize)?;
        let end = self
            .line_index
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.line_index.length);

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

    pub fn position_to_offset(&self, position: Position) -> Option<u32> {
        self.line_index.offset(position)
    }

    pub fn offset_to_position(&self, offset: u32) -> Position {
        self.line_index.position(offset)
    }

    pub fn update_content(&mut self, content: &str) {
        self.line_index = LineIndex::new(content);
    }

    pub fn version(&self) -> i32 {
        self.version
    }

    pub fn language_id(&self) -> LanguageId {
        self.language_id.clone()
    }
}
