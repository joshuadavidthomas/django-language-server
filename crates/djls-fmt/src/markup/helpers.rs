// Vendored from markup_fmt v0.26.0
// Stripped to HTML + Jinja/Django + XML only

use std::sync::LazyLock;

use aho_corasick::AhoCorasick;

use crate::markup::Language;

pub(crate) fn is_component(name: &str) -> bool {
    name.contains('-') || name.contains(|c: char| c.is_ascii_uppercase())
}

static NON_WS_SENSITIVE_TAGS: [&str; 76] = [
    "address",
    "blockquote",
    "button",
    "caption",
    "center",
    "colgroup",
    "dialog",
    "div",
    "figure",
    "figcaption",
    "footer",
    "form",
    "select",
    "option",
    "optgroup",
    "header",
    "hr",
    "legend",
    "listing",
    "main",
    "p",
    "plaintext",
    "pre",
    "progress",
    "search",
    "object",
    "details",
    "summary",
    "xmp",
    "area",
    "base",
    "basefont",
    "datalist",
    "head",
    "link",
    "meta",
    "meter",
    "noembed",
    "noframes",
    "param",
    "rp",
    "title",
    "html",
    "body",
    "article",
    "aside",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hgroup",
    "nav",
    "section",
    "table",
    "tr",
    "thead",
    "th",
    "tbody",
    "td",
    "tfoot",
    "dir",
    "dd",
    "dl",
    "dt",
    "menu",
    "ol",
    "ul",
    "li",
    "fieldset",
    "video",
    "audio",
    "picture",
    "source",
    "track",
];

pub(crate) fn is_whitespace_sensitive_tag(name: &str, language: Language) -> bool {
    match language {
        Language::Html | Language::Jinja => {
            name.eq_ignore_ascii_case("a")
                || !NON_WS_SENSITIVE_TAGS
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case(name))
                    && !css_dataset::tags::SVG_TAGS
                        .iter()
                        .any(|tag| tag.eq_ignore_ascii_case(name))
        }
        Language::Xml => false,
    }
}

static VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr", "param",
];

pub(crate) fn is_void_element(name: &str, language: Language) -> bool {
    match language {
        Language::Html | Language::Jinja => VOID_ELEMENTS
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(name)),
        Language::Xml => false,
    }
}

pub(crate) fn is_html_tag(name: &str, language: Language) -> bool {
    match language {
        Language::Html | Language::Jinja => {
            css_dataset::tags::STANDARD_HTML_TAGS
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case(name))
                || css_dataset::tags::NON_STANDARD_HTML_TAGS
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case(name))
        }
        Language::Xml => false,
    }
}

pub(crate) fn is_svg_tag(name: &str, language: Language) -> bool {
    match language {
        Language::Html | Language::Jinja => css_dataset::tags::SVG_TAGS
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(name)),
        Language::Xml => false,
    }
}

pub(crate) fn is_mathml_tag(name: &str, language: Language) -> bool {
    match language {
        Language::Html | Language::Jinja => css_dataset::tags::MATH_ML_TAGS
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(name)),
        Language::Xml => false,
    }
}

pub(crate) static UNESCAPING_AC: LazyLock<AhoCorasick> =
    LazyLock::new(|| AhoCorasick::new(["&quot;", "&#x22;", "&#x27;"]).unwrap());

pub(crate) fn detect_indent(s: &str) -> usize {
    s.lines()
        .skip(if s.starts_with([' ', '\t']) { 0 } else { 1 })
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.as_bytes()
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count()
        })
        .min()
        .unwrap_or_default()
}

pub(crate) fn has_template_interpolation(s: &str, language: Language) -> bool {
    match language {
        Language::Html | Language::Xml => false,
        Language::Jinja => s.contains("{{") || s.contains("{%"),
    }
}
