use tower_lsp_server::lsp_types::Position;

#[derive(Clone, Debug)]
pub struct LineIndex {
    pub line_starts: Vec<u32>,
    pub line_starts_utf16: Vec<u32>,
    pub length: u32,
    pub length_utf16: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        let mut line_starts_utf16 = vec![0];
        let mut pos_utf8 = 0;
        let mut pos_utf16 = 0;

        for c in text.chars() {
            pos_utf8 += u32::try_from(c.len_utf8()).unwrap_or(0);
            pos_utf16 += u32::try_from(c.len_utf16()).unwrap_or(0);
            if c == '\n' {
                line_starts.push(pos_utf8);
                line_starts_utf16.push(pos_utf16);
            }
        }

        Self {
            line_starts,
            line_starts_utf16,
            length: pos_utf8,
            length_utf16: pos_utf16,
        }
    }

    pub fn offset(&self, position: Position) -> Option<u32> {
        let line_start = self.line_starts.get(position.line as usize)?;

        Some(line_start + position.character)
    }

    /// Convert UTF-16 LSP position to UTF-8 byte offset
    pub fn offset_utf16(&self, position: Position, text: &str) -> Option<u32> {
        let line_start_utf8 = self.line_starts.get(position.line as usize)?;
        let _line_start_utf16 = self.line_starts_utf16.get(position.line as usize)?;

        // If position is at start of line, return UTF-8 line start
        if position.character == 0 {
            return Some(*line_start_utf8);
        }

        // Find the line text
        let next_line_start = self
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.length);

        let line_text = text.get(*line_start_utf8 as usize..next_line_start as usize)?;

        // Convert UTF-16 character offset to UTF-8 byte offset within the line
        let mut utf16_pos = 0;
        let mut utf8_pos = 0;

        for c in line_text.chars() {
            if utf16_pos >= position.character {
                break;
            }
            utf16_pos += u32::try_from(c.len_utf16()).unwrap_or(0);
            utf8_pos += u32::try_from(c.len_utf8()).unwrap_or(0);
        }

        Some(line_start_utf8 + utf8_pos)
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
