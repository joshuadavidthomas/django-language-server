pub mod diff;
mod django;

pub use diff::compute_text_edits;
pub use diff::is_changed;
pub use diff::unified_diff;
pub use diff::Edit;
pub use diff::InvalidEditRange;
pub use django::format_django_syntax;
pub use djls_conf::ContentType;
pub use djls_conf::FormatConfig;
pub use djls_conf::IndentStyle;

/// Format a Django template source string according to the given configuration.
///
/// Routes to the appropriate formatting backend based on content type:
/// - `Text` / `Auto`: token-level Django syntax formatting (normalizes tags,
///   variables, comments while preserving non-Django text)
/// - `Html`: currently falls through to Django syntax formatting; HTML-aware
///   formatting via `markup_fmt` will be added in a later phase
#[must_use]
pub fn format_source(source: &str, config: &FormatConfig) -> String {
    // HTML-aware formatting via `markup_fmt` will be added in a later phase;
    // for now all content types go through Django syntax formatting.
    match config.content_type() {
        ContentType::Auto | ContentType::Text | ContentType::Html => {
            format_django_syntax(source, config)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_source_applies_django_formatting() {
        let source = "{%if user%}{{user.name}}{%endif%}";
        let formatted = format_source(source, &FormatConfig::default());
        assert_eq!(formatted, "{% if user %}{{ user.name }}{% endif %}");
    }

    #[test]
    fn format_source_text_content_type() {
        let source = "{%if user%}";
        let config = FormatConfig::default();
        // Default is Auto, which should also format
        let formatted = format_source(source, &config);
        assert_eq!(formatted, "{% if user %}");
    }
}
