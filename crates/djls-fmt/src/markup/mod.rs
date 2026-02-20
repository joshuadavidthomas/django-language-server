// Vendored from markup_fmt v0.26.0
// https://github.com/g-plane/markup_fmt
// Copyright (c) 2023-present Pig Fang — MIT License (see LICENSE)
//
// Stripped to HTML + Jinja/Django + XML only.
// Django-specific parser fixes applied (see parser.rs).

mod ast;
pub mod config;
mod ctx;
mod error;
mod helpers;
mod parser;
mod printer;
mod state;

use std::borrow::Cow;

use tiny_pretty::IndentKind;
use tiny_pretty::PrintOptions;

use crate::markup::config::FormatOptions;
use crate::markup::ctx::Ctx;
pub use crate::markup::ctx::Hints;
pub use crate::markup::error::*;
pub use crate::markup::parser::Language;
use crate::markup::parser::Parser;
use crate::markup::printer::DocGen;
use crate::markup::state::State;

/// Format the given source code.
///
/// An external formatter is required for formatting code
/// inside `<script>` or `<style>` tag.
pub fn format_text<E, F>(
    code: &str,
    language: Language,
    options: &FormatOptions,
    external_formatter: F,
) -> Result<String, FormatError<E>>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
{
    let mut parser = Parser::new(code, language);
    let ast = parser.parse_root().map_err(FormatError::Syntax)?;

    if ast.children.first().is_some_and(|child| {
        if let ast::Node {
            kind: ast::NodeKind::Comment(ast::Comment { raw, .. }),
            ..
        } = child
        {
            raw.trim_start()
                .strip_prefix(&options.language.ignore_file_comment_directive)
                .is_some_and(|rest| {
                    rest.starts_with(|c: char| c.is_ascii_whitespace()) || rest.is_empty()
                })
        } else {
            false
        }
    }) {
        return Ok(code.into());
    }

    let mut ctx = Ctx {
        source: code,
        language,
        indent_width: options.layout.indent_width,
        print_width: options.layout.print_width,
        options: &options.language,
        external_formatter,
        external_formatter_errors: Default::default(),
    };

    let doc = ast.doc(
        &mut ctx,
        &State {
            current_tag_name: None,
            is_root: true,
            in_svg: false,
            indent_level: 0,
        },
    );
    if !ctx.external_formatter_errors.is_empty() {
        return Err(FormatError::External(ctx.external_formatter_errors));
    }

    Ok(tiny_pretty::print(
        &doc,
        &PrintOptions {
            indent_kind: if options.layout.use_tabs {
                IndentKind::Tab
            } else {
                IndentKind::Space
            },
            line_break: options.layout.line_break.into(),
            width: options.layout.print_width,
            tab_size: options.layout.indent_width,
        },
    ))
}
