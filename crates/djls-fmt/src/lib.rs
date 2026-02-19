pub use djls_conf::ContentType;
pub use djls_conf::FormatConfig;
pub use djls_conf::IndentStyle;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("formatting backend for content type `{0:?}` is unavailable")]
    UnsupportedContentType(ContentType),
}

pub type Result<T> = std::result::Result<T, FormatError>;

pub fn format_source(source: &str, _config: &FormatConfig) -> Result<String> {
    Ok(source.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_source_is_passthrough_for_now() {
        let source = "{%if user%}{{user.name}}{%endif%}";
        let formatted = format_source(source, &FormatConfig::default()).unwrap();
        assert_eq!(formatted, source);
    }
}
