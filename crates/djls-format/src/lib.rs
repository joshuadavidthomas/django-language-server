mod djangofmt;

use std::num::NonZeroU8;

use camino::Utf8Path;
use djls_conf::FormatBackend;
use djls_source::LineEnding;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatOutcome {
    Changed(String),
    Unchanged,
    Ignored,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatOptions {
    indent_width: Option<IndentWidth>,
    indent_style: Option<IndentStyle>,
    trim_trailing_whitespace: bool,
    insert_final_newline: bool,
    trim_final_newlines: bool,
}

impl FormatOptions {
    #[must_use]
    pub fn new(indent_width: Option<IndentWidth>, indent_style: Option<IndentStyle>) -> Self {
        Self {
            indent_width,
            indent_style,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn trim_trailing_whitespace(mut self, enabled: bool) -> Self {
        self.trim_trailing_whitespace = enabled;
        self
    }

    #[must_use]
    pub fn insert_final_newline(mut self, enabled: bool) -> Self {
        self.insert_final_newline = enabled;
        self
    }

    #[must_use]
    pub fn trim_final_newlines(mut self, enabled: bool) -> Self {
        self.trim_final_newlines = enabled;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentWidth(NonZeroU8);

impl IndentWidth {
    const MAX: u8 = 16;

    #[must_use]
    pub(crate) const fn value(self) -> u8 {
        self.0.get()
    }
}

impl TryFrom<u8> for IndentWidth {
    type Error = String;

    fn try_from(width: u8) -> Result<Self, Self::Error> {
        match NonZeroU8::new(width) {
            Some(width) if width.get() <= Self::MAX => Ok(Self(width)),
            _ => Err(format!(
                "indent-width must be between 1 and {} (got {width})",
                Self::MAX,
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    Spaces,
    Tabs,
}

fn normalize_formatted_text(formatted: String, format_options: FormatOptions) -> String {
    let mut formatted = formatted;

    if format_options.trim_trailing_whitespace {
        formatted = trim_trailing_line_whitespace(formatted);
    }

    let final_line_suffix = if format_options.trim_final_newlines {
        LineEnding::strip_suffix(&formatted)
    } else {
        None
    };

    if let Some((mut prefix, ending)) = final_line_suffix {
        while let Some((next_prefix, _)) = LineEnding::strip_suffix(prefix) {
            prefix = next_prefix;
        }

        let prefix_len = prefix.len();
        formatted.replace_range(prefix_len.., ending.as_str());
    }

    if format_options.insert_final_newline && LineEnding::strip_suffix(&formatted).is_none() {
        let preferred_final_line_ending = LineEnding::last_in(&formatted).unwrap_or_default();
        formatted.push_str(preferred_final_line_ending.as_str());
    }

    formatted
}

fn trim_trailing_line_whitespace(text: String) -> String {
    let mut bytes = text.into_bytes();
    let mut read = 0;
    let mut write = 0;
    let mut line_write_end = 0;

    while read < bytes.len() {
        if let Some(ending) = LineEnding::match_at(&bytes, read) {
            write = line_write_end;

            for offset in 0..ending.byte_len() {
                bytes[write] = bytes[read + offset];
                write += 1;
            }

            read += ending.byte_len();
            line_write_end = write;
        } else {
            let byte = bytes[read];
            bytes[write] = byte;
            read += 1;
            write += 1;

            if !matches!(byte, b' ' | b'\t') {
                line_write_end = write;
            }
        }
    }

    bytes.truncate(line_write_end);
    String::from_utf8(bytes).expect("trimming ASCII whitespace preserves UTF-8")
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("djangofmt failed: {0}")]
    Djangofmt(String),
}

pub fn format_template(
    source: &str,
    path: &Utf8Path,
    backend: FormatBackend,
    format_options: FormatOptions,
) -> Result<FormatOutcome, FormatError> {
    let formatted = match backend {
        FormatBackend::Djangofmt => djangofmt::format(source, path, format_options),
    }?;
    let Some(formatted) = formatted else {
        return Ok(FormatOutcome::Ignored);
    };
    let formatted = normalize_formatted_text(formatted, format_options);

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
    fn invalid_indent_width_is_rejected() {
        assert!(IndentWidth::try_from(0).is_err());
        assert!(IndentWidth::try_from(17).is_err());
        assert_eq!(
            IndentWidth::try_from(4).ok().map(IndentWidth::value),
            Some(4)
        );
    }

    #[test]
    fn text_options_trim_trailing_whitespace() {
        assert_eq!(
            normalize_formatted_text(
                "alpha  \r\nβeta\t\ngamma  ".to_string(),
                FormatOptions::default().trim_trailing_whitespace(true),
            ),
            "alpha\r\nβeta\ngamma",
        );
    }

    #[test]
    fn text_options_insert_final_newline() {
        assert_eq!(
            normalize_formatted_text(
                "alpha".to_string(),
                FormatOptions::default().insert_final_newline(true),
            ),
            "alpha\n",
        );
        assert_eq!(
            normalize_formatted_text(
                "alpha\r\nbeta".to_string(),
                FormatOptions::default().insert_final_newline(true),
            ),
            "alpha\r\nbeta\r\n",
        );
    }

    #[test]
    fn text_options_trim_extra_final_newlines() {
        assert_eq!(
            normalize_formatted_text(
                "alpha\n\n\n".to_string(),
                FormatOptions::default().trim_final_newlines(true),
            ),
            "alpha\n",
        );
        assert_eq!(
            normalize_formatted_text(
                "alpha\r\n\r\n".to_string(),
                FormatOptions::default().trim_final_newlines(true),
            ),
            "alpha\r\n",
        );
        assert_eq!(
            normalize_formatted_text(
                "alpha\r\n\n".to_string(),
                FormatOptions::default().trim_final_newlines(true),
            ),
            "alpha\n",
        );
    }

    #[test]
    fn text_options_combine_final_newline_options() {
        assert_eq!(
            normalize_formatted_text(
                "alpha".to_string(),
                FormatOptions::default()
                    .insert_final_newline(true)
                    .trim_final_newlines(true),
            ),
            "alpha\n",
        );
        assert_eq!(
            normalize_formatted_text(
                "alpha\n\n".to_string(),
                FormatOptions::default()
                    .insert_final_newline(true)
                    .trim_final_newlines(true),
            ),
            "alpha\n",
        );
    }

    #[test]
    fn formats_template_text() {
        let source = "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}');\">\n    Content\n</div>\n";

        assert_eq!(
            format_template(
                source,
                Utf8Path::new("template.html"),
                FormatBackend::Djangofmt,
                FormatOptions::default(),
            )
            .unwrap(),
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
            format_template(
                source,
                Utf8Path::new("template.html"),
                FormatBackend::Djangofmt,
                FormatOptions::default(),
            )
            .unwrap(),
            FormatOutcome::Unchanged,
        );
    }

    #[test]
    fn reports_ignored_template_text() {
        let source = "<!-- djangofmt:ignore -->\n<div>Content</div>\n";

        assert_eq!(
            format_template(
                source,
                Utf8Path::new("template.html"),
                FormatBackend::Djangofmt,
                FormatOptions::default(),
            )
            .unwrap(),
            FormatOutcome::Ignored,
        );
    }
}
