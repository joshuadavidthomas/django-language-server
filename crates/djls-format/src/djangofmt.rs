use ::djangofmt::args::Profile;
use ::djangofmt::commands::format::FormatterConfig;
use ::djangofmt::commands::format::format_text;
use ::djangofmt::line_width::IndentWidth as DjangofmtIndentWidth;
use ::djangofmt::pyproject;
use camino::Utf8Path;

use crate::FormatError;
use crate::FormatOptions;
use crate::IndentStyle;

pub(super) fn format(
    source: &str,
    path: &Utf8Path,
    format_options: FormatOptions,
) -> Result<Option<String>, FormatError> {
    let options = pyproject::load_options(path.as_std_path());
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

    format_text(source, &config, profile)
        .map_err(|error| FormatError::Djangofmt(format!("{error:?}")))
}
