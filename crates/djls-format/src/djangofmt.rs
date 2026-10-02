use ::djangofmt::args::Profile;
use ::djangofmt::commands::format::FormatterConfig;
use ::djangofmt::commands::format::format_text;
use ::djangofmt::line_width::IndentWidth as DjangofmtIndentWidth;
use ::djangofmt::pyproject;
use camino::Utf8Path;
use markup_fmt::SyntaxErrorKind;

use crate::FormatError;
use crate::FormatOptions;
use crate::IndentStyle;

pub(super) fn format(
    source: &str,
    path: &Utf8Path,
    format_options: FormatOptions,
) -> Result<Option<String>, FormatError> {
    let (options, _root) = pyproject::load_options(path.as_std_path())
        .map_err(|error| FormatError::Config(format!("{error}")))?;
    let profile = options
        .profile
        .or_else(|| Profile::from_path(path.as_std_path()))
        .unwrap_or_default();
    let mut config = FormatterConfig::new(
        options.line_length.unwrap_or_default(),
        format_options
            .indent_width
            .and_then(|width| DjangofmtIndentWidth::try_from(width.value()).ok())
            .or(options.indent_width)
            .unwrap_or_default(),
        options.custom_blocks,
        options.html_void_self_closing.unwrap_or_default(),
        options.preserve_unquoted_attrs.unwrap_or_default(),
    );
    if let Some(use_tabs) = format_options
        .indent_style
        .map(|style| style == IndentStyle::Tabs)
    {
        config.markup.layout.use_tabs = use_tabs;
        config.malva.layout.use_tabs = use_tabs;
        config.json.use_tabs = use_tabs;
    }

    format_text(source, &config, profile, Some(path.as_std_path())).map_err(|error| match error {
        markup_fmt::FormatError::Syntax(syntax) => {
            // An unclosed tag is reported where parsing gave up, often the end of
            // the file; its opening tag is the place to fix.
            let pos = if let SyntaxErrorKind::ExpectCloseTag { pos, .. } = &syntax.kind {
                *pos
            } else if let SyntaxErrorKind::ExpectJinjaBlockEnd { pos, .. } = &syntax.kind {
                // This position is just past the `{%`.
                source
                    .get(..*pos)
                    .and_then(|before| before.rfind("{%"))
                    .unwrap_or(*pos)
            } else {
                syntax.pos
            };
            let (line, column) = line_column(source, pos);
            FormatError::Syntax {
                line,
                column,
                detail: format!("{:?}", syntax.kind),
            }
        }
        markup_fmt::FormatError::External(_) => FormatError::Template(format!("{error:?}")),
    })
}

// markup_fmt's own line/column are off by one after the first line and zero on
// the last, so derive 1-based values from the byte offset.
fn line_column(source: &str, pos: usize) -> (usize, usize) {
    let before = source.get(..pos).unwrap_or(source);
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let line = before.matches('\n').count() + 1;
    let column = before[line_start..].chars().count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::line_column;

    #[test]
    fn line_column_is_one_based_on_every_line() {
        let source = "ab\ncd\né";
        assert_eq!(line_column(source, 0), (1, 1));
        assert_eq!(line_column(source, 1), (1, 2));
        assert_eq!(line_column(source, 3), (2, 1));
        assert_eq!(line_column(source, 6), (3, 1));
        assert_eq!(line_column(source, source.len()), (3, 2));
    }
}
