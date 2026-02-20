// Vendored from markup_fmt v0.26.0
// Stripped to HTML + Jinja/Django + XML only

use std::num::NonZeroUsize;

#[derive(Clone, Debug, Default)]
pub struct FormatOptions {
    pub layout: LayoutOptions,
    pub language: LanguageOptions,
}

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    pub print_width: usize,
    pub use_tabs: bool,
    pub indent_width: usize,
    pub line_break: LineBreak,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            print_width: 80,
            use_tabs: false,
            indent_width: 2,
            line_break: LineBreak::Lf,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum LineBreak {
    #[default]
    Lf,
    Crlf,
}

impl From<LineBreak> for tiny_pretty::LineBreak {
    fn from(value: LineBreak) -> Self {
        match value {
            LineBreak::Lf => tiny_pretty::LineBreak::Lf,
            LineBreak::Crlf => tiny_pretty::LineBreak::Crlf,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LanguageOptions {
    pub quotes: Quotes,
    pub format_comments: bool,
    pub script_indent: bool,
    pub html_script_indent: Option<bool>,
    pub style_indent: bool,
    pub html_style_indent: Option<bool>,
    pub closing_bracket_same_line: bool,
    pub closing_tag_line_break_for_empty: ClosingTagLineBreakForEmpty,
    pub max_attrs_per_line: Option<NonZeroUsize>,
    pub prefer_attrs_single_line: bool,
    pub single_attr_same_line: bool,
    pub html_normal_self_closing: Option<bool>,
    pub html_void_self_closing: Option<bool>,
    pub component_self_closing: Option<bool>,
    pub svg_self_closing: Option<bool>,
    pub mathml_self_closing: Option<bool>,
    pub whitespace_sensitivity: WhitespaceSensitivity,
    pub doctype_keyword_case: DoctypeKeywordCase,
    pub script_formatter: Option<ScriptFormatter>,
    pub ignore_comment_directive: String,
    pub ignore_file_comment_directive: String,
}

impl Default for LanguageOptions {
    fn default() -> Self {
        LanguageOptions {
            quotes: Quotes::default(),
            format_comments: false,
            script_indent: false,
            html_script_indent: None,
            style_indent: false,
            html_style_indent: None,
            closing_bracket_same_line: false,
            closing_tag_line_break_for_empty: ClosingTagLineBreakForEmpty::default(),
            max_attrs_per_line: None,
            prefer_attrs_single_line: false,
            single_attr_same_line: true,
            html_normal_self_closing: None,
            html_void_self_closing: None,
            component_self_closing: None,
            svg_self_closing: None,
            mathml_self_closing: None,
            whitespace_sensitivity: WhitespaceSensitivity::default(),
            doctype_keyword_case: DoctypeKeywordCase::default(),
            script_formatter: None,
            ignore_comment_directive: "markup-fmt-ignore".into(),
            ignore_file_comment_directive: "markup-fmt-ignore-file".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Quotes {
    #[default]
    Double,
    Single,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum ClosingTagLineBreakForEmpty {
    Always,
    #[default]
    Fit,
    Never,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum WhitespaceSensitivity {
    #[default]
    Css,
    Strict,
    Ignore,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum DoctypeKeywordCase {
    Ignore,
    #[default]
    Upper,
    Lower,
}

#[derive(Clone, Copy, Debug)]
pub enum ScriptFormatter {
    Dprint,
    Biome,
}
