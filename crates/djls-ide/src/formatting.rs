use camino::Utf8Path;
use djls_conf::FormatBackend;
use djls_conf::FormatConfig;
use djls_format::FormatOutcome;
use djls_source::LineIndex;
use djls_source::PositionEncoding;
use tower_lsp_server::ls_types;

#[must_use]
pub fn format_document(
    source: &str,
    path: &Utf8Path,
    line_index: &LineIndex,
    encoding: PositionEncoding,
    config: &FormatConfig,
) -> Vec<ls_types::TextEdit> {
    if !config.enabled() {
        return Vec::new();
    }

    let result = match config.backend() {
        FormatBackend::Djangofmt => djls_format::format_template(source, path),
    };

    match result {
        Ok(FormatOutcome::Changed(formatted)) => {
            vec![ls_types::TextEdit::new(
                full_document_range(source, line_index, encoding),
                formatted,
            )]
        }
        Ok(FormatOutcome::Unchanged | FormatOutcome::Ignored) => Vec::new(),
        Err(error) => {
            tracing::debug!("Formatting failed for {path}: {error}");
            Vec::new()
        }
    }
}

fn full_document_range(
    source: &str,
    line_index: &LineIndex,
    encoding: PositionEncoding,
) -> ls_types::Range {
    ls_types::Range {
        start: ls_types::Position::new(0, 0),
        end: document_end_position(source, line_index, encoding),
    }
}

fn document_end_position(
    source: &str,
    line_index: &LineIndex,
    encoding: PositionEncoding,
) -> ls_types::Position {
    let line = u32::try_from(line_index.lines().len().saturating_sub(1)).unwrap_or_default();
    let line_start = line_index.lines().last().copied().unwrap_or_default() as usize;
    let line_text = &source[line_start.min(source.len())..];
    let character = match encoding {
        PositionEncoding::Utf8 => u32::try_from(line_text.len()).unwrap_or(u32::MAX),
        PositionEncoding::Utf16 => line_text
            .chars()
            .map(|character| u32::try_from(character.len_utf16()).unwrap_or_default())
            .sum(),
        PositionEncoding::Utf32 => u32::try_from(line_text.chars().count()).unwrap_or(u32::MAX),
    };

    ls_types::Position::new(line, character)
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_conf::FormatConfig;
    use djls_source::LineIndex;
    use djls_source::PositionEncoding;
    use tower_lsp_server::ls_types;

    use super::document_end_position;
    use super::format_document;

    #[test]
    fn format_document_returns_full_document_edit() {
        let source = "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}');\">\n    Content\n</div>\n";
        let line_index = LineIndex::from(source);

        let edits = format_document(
            source,
            Utf8Path::new("template.html"),
            &line_index,
            PositionEncoding::Utf16,
            &FormatConfig::default(),
        );

        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].range,
            ls_types::Range::new(ls_types::Position::new(0, 0), ls_types::Position::new(3, 0)),
        );
        assert_eq!(
            edits[0].new_text,
            "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}')\">\n    Content\n</div>\n",
        );
    }

    #[test]
    fn document_end_position_handles_trailing_newline() {
        let source = "first\nsecond\n";
        let line_index = LineIndex::from(source);

        assert_eq!(
            document_end_position(source, &line_index, PositionEncoding::Utf16),
            ls_types::Position::new(2, 0),
        );
    }

    #[test]
    fn document_end_position_uses_client_encoding() {
        let source = "emoji: 🐍";
        let line_index = LineIndex::from(source);

        assert_eq!(
            document_end_position(source, &line_index, PositionEncoding::Utf8),
            ls_types::Position::new(0, 11),
        );
        assert_eq!(
            document_end_position(source, &line_index, PositionEncoding::Utf16),
            ls_types::Position::new(0, 9),
        );
        assert_eq!(
            document_end_position(source, &line_index, PositionEncoding::Utf32),
            ls_types::Position::new(0, 8),
        );
    }
}
