pub mod diff;
mod django;
// Vendored markup_fmt — suppress warnings for vendored code that we keep
// close to upstream for maintainability.
#[allow(
    dead_code,
    clippy::bool_to_int_with_if,
    clippy::collapsible_else_if,
    clippy::enum_variant_names,
    clippy::fn_params_excessive_bools,
    clippy::items_after_statements,
    clippy::map_unwrap_or,
    clippy::module_name_repetitions,
    clippy::needless_continue,
    clippy::redundant_closure,
    clippy::redundant_else,
    clippy::semicolon_if_nothing_returned,
    clippy::struct_excessive_bools,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::default_trait_access,
    clippy::needless_borrow,
    clippy::redundant_closure_for_method_calls,
    clippy::unnecessary_semicolon,
    clippy::unnecessary_wraps,
    clippy::used_underscore_binding,
    clippy::wildcard_imports
)]
mod markup;

use std::borrow::Cow;

use camino::Utf8Path;
pub use diff::compute_text_edits;
pub use diff::is_changed;
pub use diff::unified_diff;
pub use diff::Edit;
pub use diff::InvalidEditRange;
pub use django::format_django_syntax;
pub use djls_conf::ContentType;
pub use djls_conf::FormatConfig;
pub use djls_conf::IndentStyle;
pub use markup::FormatError;
pub use markup::Language;
pub use markup::SyntaxError;

/// Detect the content type for a given file path, respecting configuration overrides.
#[must_use]
pub fn detect_content_type(path: &Utf8Path, config: &FormatConfig) -> ContentType {
    if config.content_type() != ContentType::Auto {
        return config.content_type();
    }
    match path.extension() {
        Some("html" | "htm" | "djhtml") => ContentType::Html,
        _ => ContentType::Text,
    }
}

/// Format a Django template source string according to the given configuration.
///
/// Routes to the appropriate formatting backend based on content type:
/// - `Text` / `Auto`: token-level Django syntax formatting (normalizes tags,
///   variables, comments while preserving non-Django text)
/// - `Html`: HTML-aware formatting via vendored `markup_fmt` with Django parser
///   fixes, plus embedded CSS formatting via `malva`
#[must_use]
pub fn format_source(source: &str, config: &FormatConfig) -> String {
    match config.content_type() {
        ContentType::Auto | ContentType::Text => format_django_syntax(source, config),
        ContentType::Html => format_html(source, config).unwrap_or_else(|_| {
            // Fall back to Django syntax formatting if HTML parsing fails
            format_django_syntax(source, config)
        }),
    }
}

/// Format a Django template source string with content-type detection from the file path.
#[must_use]
pub fn format_source_with_path(source: &str, path: &Utf8Path, config: &FormatConfig) -> String {
    let content_type = detect_content_type(path, config);
    let effective_config = config.clone().with_content_type(content_type);
    format_source(source, &effective_config)
}

/// Format HTML source with Django template support via vendored `markup_fmt`.
fn format_html(source: &str, config: &FormatConfig) -> Result<String, String> {
    let options = build_markup_options(config);
    let css_options = build_css_options(config);

    markup::format_text(
        source,
        markup::Language::Jinja,
        &options,
        |code, hints| match hints.ext {
            "css" | "scss" | "less" => {
                malva::format_text(code, malva_syntax(hints.ext), &css_options)
                    .map(Cow::Owned)
                    .map_err(|e| e.to_string())
            }
            _ => Ok(Cow::Borrowed(code)),
        },
    )
    .map_err(|e| e.to_string())
}

fn malva_syntax(ext: &str) -> malva::Syntax {
    match ext {
        "scss" => malva::Syntax::Scss,
        "less" => malva::Syntax::Less,
        _ => malva::Syntax::Css,
    }
}

fn build_markup_options(config: &FormatConfig) -> markup::config::FormatOptions {
    markup::config::FormatOptions {
        layout: markup::config::LayoutOptions {
            print_width: config.print_width() as usize,
            use_tabs: matches!(config.indent_style(), IndentStyle::Tabs),
            indent_width: config.indent_width() as usize,
            line_break: markup::config::LineBreak::Lf,
        },
        language: markup::config::LanguageOptions {
            ..Default::default()
        },
    }
}

fn build_css_options(config: &FormatConfig) -> malva::config::FormatOptions {
    malva::config::FormatOptions {
        layout: malva::config::LayoutOptions {
            print_width: config.print_width() as usize,
            use_tabs: matches!(config.indent_style(), IndentStyle::Tabs),
            indent_width: config.indent_width() as usize,
            ..Default::default()
        },
        language: malva::config::LanguageOptions::default(),
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
        let formatted = format_source(source, &config);
        assert_eq!(formatted, "{% if user %}");
    }

    #[test]
    fn format_html_basic() {
        let source = "<div><p>hello</p></div>";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        assert!(formatted.contains("<div>"));
        assert!(formatted.contains("<p>"));
    }

    #[test]
    fn format_html_with_django_tags() {
        let source = "<div>{% if user %}<p>{{ user.name }}</p>{% endif %}</div>";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        assert!(formatted.contains("{% if user %}"));
        assert!(formatted.contains("{{ user.name }}"));
    }

    #[test]
    fn format_html_trans_self_closing() {
        // {% trans %} must be treated as self-closing in Django (not a block tag)
        let source = "<p>{% trans \"Hello\" %}</p>";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        assert!(formatted.contains("{% trans"));
        // Should NOT be waiting for {% endtrans %}
        assert!(!formatted.contains("endtrans"));
    }

    #[test]
    fn format_html_blocktrans_block() {
        let source = "<div>{% blocktrans %}Hello{% endblocktrans %}</div>";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        assert!(formatted.contains("blocktrans"));
        assert!(formatted.contains("endblocktrans"));
    }

    #[test]
    fn format_html_for_empty() {
        // {% empty %} is an intermediate tag for {% for %} in Django
        let source = "<ul>{% for item in items %}<li>{{ item }}</li>{% empty %}<li>No items</li>{% endfor %}</ul>";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        assert!(formatted.contains("{% empty %}"));
    }

    #[test]
    fn format_html_verbatim_block() {
        let source = "<div>{% verbatim %}{{ not_rendered }}{% endverbatim %}</div>";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        assert!(formatted.contains("verbatim"));
        assert!(formatted.contains("endverbatim"));
    }

    #[test]
    fn format_html_comment_block() {
        let source = "<div>{% comment %}This is hidden{% endcomment %}</div>";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        assert!(formatted.contains("comment"));
        assert!(formatted.contains("endcomment"));
    }

    #[test]
    fn detect_content_type_html_extension() {
        let config = FormatConfig::default();
        assert_eq!(
            detect_content_type(Utf8Path::new("template.html"), &config),
            ContentType::Html
        );
        assert_eq!(
            detect_content_type(Utf8Path::new("template.htm"), &config),
            ContentType::Html
        );
        assert_eq!(
            detect_content_type(Utf8Path::new("template.djhtml"), &config),
            ContentType::Html
        );
    }

    #[test]
    fn detect_content_type_text_extension() {
        let config = FormatConfig::default();
        assert_eq!(
            detect_content_type(Utf8Path::new("email.txt"), &config),
            ContentType::Text
        );
        assert_eq!(
            detect_content_type(Utf8Path::new("query.sql"), &config),
            ContentType::Text
        );
    }

    #[test]
    fn detect_content_type_config_override() {
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        assert_eq!(
            detect_content_type(Utf8Path::new("email.txt"), &config),
            ContentType::Html
        );
    }

    #[test]
    fn format_html_fallback_on_parse_error() {
        // Broken HTML should fall back to Django syntax formatting
        let source = "{%if x%}unclosed tags everywhere{%endif%}";
        let config = FormatConfig::default().with_content_type(ContentType::Html);
        let formatted = format_source(source, &config);
        // Should still produce valid output via fallback
        assert!(formatted.contains("if x"));
    }
}
