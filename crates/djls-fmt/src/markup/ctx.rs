// Vendored from markup_fmt v0.26.0
// Stripped to HTML + Jinja/Django + XML only

use std::borrow::Cow;

use memchr::memchr;

use crate::markup::config::LanguageOptions;
use crate::markup::config::Quotes;
use crate::markup::config::WhitespaceSensitivity;
use crate::markup::helpers;
use crate::markup::state::State;
use crate::markup::Language;

const QUOTES: [&str; 3] = ["\"", "\"", "'"];

pub(crate) struct Ctx<'b, E, F>
where
    F: for<'a> FnMut(&'a str, Hints<'b>) -> Result<Cow<'a, str>, E>,
{
    pub(crate) source: &'b str,
    pub(crate) language: Language,
    pub(crate) indent_width: usize,
    pub(crate) print_width: usize,
    pub(crate) options: &'b LanguageOptions,
    pub(crate) external_formatter: F,
    pub(crate) external_formatter_errors: Vec<E>,
}

impl<'b, E, F> Ctx<'b, E, F>
where
    F: for<'a> FnMut(&'a str, Hints<'b>) -> Result<Cow<'a, str>, E>,
{
    pub(crate) fn script_indent(&self) -> bool {
        match self.language {
            Language::Html | Language::Jinja => self
                .options
                .html_script_indent
                .unwrap_or(self.options.script_indent),
            Language::Xml => false,
        }
    }

    pub(crate) fn style_indent(&self) -> bool {
        match self.language {
            Language::Html | Language::Jinja => self
                .options
                .html_style_indent
                .unwrap_or(self.options.style_indent),
            Language::Xml => false,
        }
    }

    pub(crate) fn is_whitespace_sensitive(&self, tag_name: &str) -> bool {
        match self.language {
            Language::Xml => false,
            Language::Html | Language::Jinja => match self.options.whitespace_sensitivity {
                WhitespaceSensitivity::Css => {
                    helpers::is_whitespace_sensitive_tag(tag_name, self.language)
                }
                WhitespaceSensitivity::Strict => true,
                WhitespaceSensitivity::Ignore => false,
            },
        }
    }

    pub(crate) fn with_escaping_quotes(
        &mut self,
        s: &str,
        mut processer: impl FnMut(String, &mut Self) -> String,
    ) -> String {
        let escaped = helpers::UNESCAPING_AC.replace_all(s, &QUOTES);
        let proceeded = processer(escaped, self);
        if memchr(b'\'', proceeded.as_bytes()).is_some()
            && memchr(b'"', proceeded.as_bytes()).is_some()
        {
            match self.options.quotes {
                Quotes::Double => proceeded.replace('"', "&quot;"),
                Quotes::Single => proceeded.replace('\'', "&#x27;"),
            }
        } else {
            proceeded
        }
    }

    pub(crate) fn format_script<'a>(
        &mut self,
        code: &'a str,
        lang: &'b str,
        start: usize,
        state: &State,
    ) -> Cow<'a, str> {
        self.format_with_external_formatter(
            self.source
                .get(0..start)
                .unwrap_or_default()
                .replace(|c: char| !c.is_ascii_whitespace(), " ")
                + code,
            Hints {
                print_width: self.print_width,
                indent_level: state.indent_level,
                attr: false,
                ext: lang,
            },
        )
    }

    pub(crate) fn format_style<'a>(
        &mut self,
        code: &'a str,
        lang: &'b str,
        start: usize,
        state: &State,
    ) -> Cow<'a, str> {
        self.format_with_external_formatter(
            "\n".repeat(
                self.source
                    .get(0..start)
                    .unwrap_or_default()
                    .lines()
                    .count()
                    .saturating_sub(1),
            ) + code,
            Hints {
                print_width: self
                    .print_width
                    .saturating_sub((state.indent_level as usize) * self.indent_width)
                    .saturating_sub(if self.style_indent() {
                        self.indent_width
                    } else {
                        0
                    }),
                indent_level: state.indent_level,
                attr: false,
                ext: if lang == "postcss" { "css" } else { lang },
            },
        )
    }

    pub(crate) fn format_style_attr(&mut self, code: &str, start: usize, state: &State) -> String {
        self.format_with_external_formatter(
            self.source
                .get(0..start)
                .unwrap_or_default()
                .replace(|c: char| !c.is_ascii_whitespace(), " ")
                + code,
            Hints {
                print_width: u16::MAX as usize,
                indent_level: state.indent_level,
                attr: true,
                ext: "css",
            },
        )
        .trim()
        .to_owned()
    }

    pub(crate) fn format_json<'a>(
        &mut self,
        code: &'a str,
        start: usize,
        state: &State,
    ) -> Cow<'a, str> {
        self.format_with_external_formatter(
            self.source
                .get(0..start)
                .unwrap_or_default()
                .replace(|c: char| !c.is_ascii_whitespace(), " ")
                + code,
            Hints {
                print_width: self
                    .print_width
                    .saturating_sub((state.indent_level as usize) * self.indent_width)
                    .saturating_sub(if self.script_indent() {
                        self.indent_width
                    } else {
                        0
                    }),
                indent_level: state.indent_level,
                attr: false,
                ext: "json",
            },
        )
    }

    pub(crate) fn format_jinja(
        &mut self,
        code: &str,
        start: usize,
        expr: bool,
        state: &State,
    ) -> String {
        self.format_with_external_formatter(
            self.source
                .get(0..start)
                .unwrap_or_default()
                .replace(|c: char| !c.is_ascii_whitespace(), " ")
                + code,
            Hints {
                print_width: self
                    .print_width
                    .saturating_sub((state.indent_level as usize) * self.indent_width),
                indent_level: state.indent_level,
                attr: false,
                ext: if expr {
                    "markup-fmt-jinja-expr"
                } else {
                    "markup-fmt-jinja-stmt"
                },
            },
        )
        .trim_ascii()
        .to_owned()
    }

    fn format_with_external_formatter<'a>(
        &mut self,
        code: String,
        hints: Hints<'b>,
    ) -> Cow<'a, str> {
        match (self.external_formatter)(&code, hints) {
            Ok(Cow::Owned(formatted)) => Cow::from(formatted),
            Ok(Cow::Borrowed(..)) => Cow::from(code),
            Err(e) => {
                self.external_formatter_errors.push(e);
                code.into()
            }
        }
    }
}

pub struct Hints<'s> {
    pub print_width: usize,
    pub indent_level: u16,
    pub attr: bool,
    pub ext: &'s str,
}
