use camino::Utf8Path;
use djangofmt::args::Profile;
use djangofmt::commands::format::format_text;
use djangofmt::commands::format::FormatterConfig;
use djangofmt::pyproject;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatOutcome {
    Changed(String),
    Unchanged,
    Ignored,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("djangofmt failed: {0}")]
    Djangofmt(String),
}

pub fn format_template(source: &str, path: &Utf8Path) -> Result<FormatOutcome, FormatError> {
    let options = pyproject::load_options(path.as_std_path());
    let profile = options
        .profile
        .or_else(|| Profile::from_path(path.as_std_path()))
        .unwrap_or_default();
    let config = FormatterConfig::new(
        options.line_length.unwrap_or_default(),
        options.indent_width.unwrap_or_default(),
        options.custom_blocks,
        options.html_void_self_closing.unwrap_or_default(),
        options.preserve_unquoted_attrs.unwrap_or_default(),
    );

    let Some(formatted) = format_text(source, &config, profile)
        .map_err(|error| FormatError::Djangofmt(format!("{error:?}")))?
    else {
        return Ok(FormatOutcome::Ignored);
    };

    if formatted == source {
        Ok(FormatOutcome::Unchanged)
    } else {
        Ok(FormatOutcome::Changed(formatted))
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::*;

    #[test]
    fn formats_template_text() {
        let source = "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}');\">\n    Content\n</div>\n";

        assert_eq!(
            format_template(source, Utf8Path::new("template.html")).unwrap(),
            FormatOutcome::Changed(
                "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}')\">\n    Content\n</div>\n"
                    .to_string(),
            ),
        );
    }

    #[test]
    fn reports_unchanged_template_text() {
        let source = "<div>Content</div>\n";

        assert_eq!(
            format_template(source, Utf8Path::new("template.html")).unwrap(),
            FormatOutcome::Unchanged,
        );
    }

    #[test]
    fn reports_ignored_template_text() {
        let source = "<!-- djangofmt:ignore -->\n<div>Content</div>\n";

        assert_eq!(
            format_template(source, Utf8Path::new("template.html")).unwrap(),
            FormatOutcome::Ignored,
        );
    }
}
