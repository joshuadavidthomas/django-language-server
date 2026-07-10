use djls_conf::FormatBackend;
use djls_format::FormatOptions;
use djls_format::FormatOutcome;
use djls_format::IndentStyle;
use djls_format::IndentWidth;
use djls_source::Db;
use djls_source::File;
use djls_source::PositionEncoding;
use tower_lsp_server::ls_types;

#[must_use]
pub fn format_document(
    db: &dyn Db,
    file: File,
    encoding: PositionEncoding,
    backend: FormatBackend,
    formatting_options: &ls_types::FormattingOptions,
) -> Vec<ls_types::TextEdit> {
    let Ok(source) = file.try_source(db) else {
        return Vec::new();
    };
    let path = file.path(db);

    let indent_width = u8::try_from(formatting_options.tab_size)
        .ok()
        .and_then(|width| IndentWidth::try_from(width).ok());
    let indent_style = if formatting_options.insert_spaces {
        IndentStyle::Spaces
    } else {
        IndentStyle::Tabs
    };
    let format_options = FormatOptions::new(indent_width, Some(indent_style))
        .trim_trailing_whitespace(formatting_options.trim_trailing_whitespace.unwrap_or(false))
        .insert_final_newline(formatting_options.insert_final_newline.unwrap_or(false))
        .trim_final_newlines(formatting_options.trim_final_newlines.unwrap_or(false));

    let formatted =
        match djls_format::format_template(source.as_str(), path, backend, format_options) {
            Ok(FormatOutcome::Changed(formatted)) => formatted,
            Ok(FormatOutcome::Unchanged | FormatOutcome::Ignored) => return Vec::new(),
            Err(error) => {
                tracing::debug!("Formatting failed for {path}: {error}");
                return Vec::new();
            }
        };

    let (line, character) = file.end_line_col(db, encoding).into();
    vec![ls_types::TextEdit::new(
        ls_types::Range::new(
            ls_types::Position::new(0, 0),
            ls_types::Position::new(line, character),
        ),
        formatted,
    )]
}
