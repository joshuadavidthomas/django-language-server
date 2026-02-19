pub mod diff;

pub use diff::compute_text_edits;
pub use diff::is_changed;
pub use diff::unified_diff;
pub use diff::Edit;
pub use djls_conf::ContentType;
pub use djls_conf::FormatConfig;
pub use djls_conf::IndentStyle;

#[must_use]
pub fn format_source(source: &str, config: &FormatConfig) -> String {
    match config.content_type() {
        ContentType::Auto | ContentType::Html | ContentType::Text => source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_source_is_passthrough_for_now() {
        let source = "{%if user%}{{user.name}}{%endif%}";
        let formatted = format_source(source, &FormatConfig::default());
        assert_eq!(formatted, source);
    }
}
