use djls_source::FileKind;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::TagDelimiter;
use djls_templates::Token;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompletionOffsetContext<'source> {
    Template(TemplateCompletionContext<'source>),
    None,
}

impl<'source> CompletionOffsetContext<'source> {
    pub(crate) fn new(
        kind: FileKind,
        source: &'source str,
        tokens: &[Token],
        offset: Offset,
    ) -> Self {
        match kind {
            FileKind::Template => Self::Template(TemplateCompletionContext::from_tokens(
                source, tokens, offset,
            )),
            FileKind::Python | FileKind::Other => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplateCompletionContext<'source> {
    Text,
    TagName {
        prefix: OffsetPrefix<'source>,
        needs_leading_space: bool,
        close: TagClose,
    },
    TagArgument {
        tag: &'source str,
        position: usize,
        prefix: OffsetPrefix<'source>,
        close: TagClose,
    },
    QuotedArgument {
        tag: &'source str,
        position: usize,
        quote: char,
        prefix: OffsetPrefix<'source>,
        suffix: OffsetSuffix<'source>,
        closed: bool,
        close: TagClose,
    },
    LibraryName {
        prefix: OffsetPrefix<'source>,
        suffix: OffsetSuffix<'source>,
        close: TagClose,
    },
    LoadSymbol {
        prefix: OffsetPrefix<'source>,
        suffix: OffsetSuffix<'source>,
        library: Option<&'source str>,
        needs_trailing_space: bool,
    },
    Filter {
        prefix: OffsetPrefix<'source>,
    },
}

impl<'source> TemplateCompletionContext<'source> {
    fn from_tokens(source: &'source str, tokens: &[Token], offset: Offset) -> Self {
        let Some(token) = token_at_offset(tokens, offset) else {
            return Self::Text;
        };

        match token {
            Token::Block { span, .. }
            | Token::Error {
                span,
                delimiter: TagDelimiter::Block,
                ..
            } => Self::from_tag(source, *span, offset),
            Token::Variable { span, .. }
            | Token::Error {
                span,
                delimiter: TagDelimiter::Variable,
                ..
            } => Self::from_variable(source, *span, offset),
            Token::Comment { .. }
            | Token::Eof
            | Token::Error {
                delimiter: TagDelimiter::Comment,
                ..
            }
            | Token::Newline { .. }
            | Token::Text { .. }
            | Token::Whitespace { .. } => Self::Text,
        }
    }

    fn from_tag(source: &'source str, content_span: Span, offset: Offset) -> Self {
        let content = content_before_offset(source, content_span, offset);
        let close = TagClose::after_offset(source, offset, content_span);
        let needs_leading_space = content.is_empty() || !content.starts_with(' ');
        let trimmed = content.trim_start();
        let tokens = split_tag_words(trimmed);

        if tokens.is_empty() {
            return Self::TagName {
                prefix: OffsetPrefix::new("", offset),
                needs_leading_space,
                close,
            };
        }

        if tokens.len() == 1 && !ends_in_unquoted_whitespace(trimmed) {
            return Self::TagName {
                prefix: OffsetPrefix::new(tokens[0], offset),
                needs_leading_space,
                close,
            };
        }

        let tag = tokens[0];
        if tag == "load" {
            return Self::from_load_tag(source, content_span, offset, trimmed, &tokens);
        }

        let args = &tokens[1..];
        let (position, prefix) = if ends_in_unquoted_whitespace(trimmed) {
            (args.len(), "")
        } else if let Some((partial, complete_args)) = args.split_last() {
            (complete_args.len(), *partial)
        } else {
            (0, "")
        };

        if let Some((quote, quoted_prefix)) = unclosed_quote_prefix(prefix) {
            let (suffix, closed, close) =
                OffsetSuffix::quoted_at_offset(source, offset, content_span, quote);
            return Self::QuotedArgument {
                tag,
                position,
                quote,
                prefix: OffsetPrefix::new(quoted_prefix, offset),
                suffix,
                closed,
                close,
            };
        }

        Self::TagArgument {
            tag,
            position,
            prefix: OffsetPrefix::new(prefix, offset),
            close,
        }
    }

    fn from_load_tag(
        source: &'source str,
        content_span: Span,
        offset: Offset,
        trimmed_before_offset: &'source str,
        tokens_before_offset: &[&'source str],
    ) -> Self {
        let prefix = if ends_in_unquoted_whitespace(trimmed_before_offset) {
            ""
        } else {
            tokens_before_offset.last().copied().unwrap_or_default()
        };

        let start = content_span.start_usize().min(source.len());
        let end = content_span.end_usize().min(source.len());
        let full_content = if start <= end {
            &source[start..end]
        } else {
            ""
        };
        let full_tokens = split_tag_words(full_content.trim_start());
        let ends_in_separator = ends_in_unquoted_whitespace(trimmed_before_offset);
        let current_index = if ends_in_separator {
            tokens_before_offset.len()
        } else {
            tokens_before_offset.len().saturating_sub(1)
        };

        let Some(from_index) = full_tokens.iter().position(|token| *token == "from") else {
            let suffix = OffsetSuffix::word_at_offset(source, offset, content_span);
            let close = TagClose::after_offset(source, suffix.span.end_offset(), content_span);
            return Self::LibraryName {
                prefix: OffsetPrefix::new(prefix, offset),
                suffix,
                close,
            };
        };

        if current_index > from_index {
            let suffix = OffsetSuffix::word_at_offset(source, offset, content_span);
            let close = TagClose::after_offset(source, suffix.span.end_offset(), content_span);
            Self::LibraryName {
                prefix: OffsetPrefix::new(prefix, offset),
                suffix,
                close,
            }
        } else if current_index < from_index {
            Self::LoadSymbol {
                prefix: OffsetPrefix::new(prefix, offset),
                suffix: OffsetSuffix::word_at_offset(source, offset, content_span),
                library: full_tokens.get(from_index + 1).copied(),
                needs_trailing_space: false,
            }
        } else if ends_in_separator {
            Self::LoadSymbol {
                prefix: OffsetPrefix::new("", offset),
                suffix: OffsetSuffix::new("", offset),
                library: full_tokens.get(from_index + 1).copied(),
                needs_trailing_space: true,
            }
        } else {
            Self::Text
        }
    }

    fn from_variable(source: &'source str, content_span: Span, offset: Offset) -> Self {
        let content = content_before_offset(source, content_span, offset);
        let Some(pipe) = find_last_unquoted_pipe(content) else {
            return Self::Text;
        };
        let after_pipe = &content[pipe + 1..];
        let prefix = after_pipe.trim_start();

        Self::Filter {
            prefix: OffsetPrefix::new(prefix, offset),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagClose {
    None,
    Partial { replacement_suffix_len: usize },
    Full { replacement_suffix_len: usize },
}

impl TagClose {
    fn after_offset(source: &str, offset: Offset, content_span: Span) -> Self {
        Self::after_position(source, offset.get() as usize, content_span)
    }

    fn after_position(source: &str, offset: usize, content_span: Span) -> Self {
        let offset = offset.min(source.len());
        let content_end = content_span.end_usize().min(source.len());
        if offset > content_end {
            return Self::None;
        }

        let content_suffix = &source[offset..content_end];
        if content_suffix.chars().all(char::is_whitespace)
            && source[content_end..].starts_with("%}")
        {
            return Self::Full {
                replacement_suffix_len: content_suffix.len() + "%}".len(),
            };
        }

        let Some(close_offset) = content_suffix.find('}') else {
            return Self::None;
        };
        if content_suffix[..close_offset]
            .chars()
            .all(char::is_whitespace)
        {
            Self::Partial {
                replacement_suffix_len: close_offset + 1,
            }
        } else {
            Self::None
        }
    }

    pub(crate) fn partial_replacement_suffix_len(self) -> usize {
        match self {
            Self::Partial {
                replacement_suffix_len,
            } => replacement_suffix_len,
            Self::None | Self::Full { .. } => 0,
        }
    }

    pub(crate) fn existing_close_replacement_suffix_len(self) -> usize {
        match self {
            Self::Partial {
                replacement_suffix_len,
            }
            | Self::Full {
                replacement_suffix_len,
            } => replacement_suffix_len,
            Self::None => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OffsetPrefix<'source> {
    pub(crate) text: &'source str,
    pub(crate) span: Span,
}

impl<'source> OffsetPrefix<'source> {
    fn new(text: &'source str, offset: Offset) -> Self {
        Self {
            text,
            span: Span::before_offset(offset, text.len()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OffsetSuffix<'source> {
    pub(crate) text: &'source str,
    pub(crate) span: Span,
}

impl<'source> OffsetSuffix<'source> {
    fn new(text: &'source str, offset: Offset) -> Self {
        Self {
            text,
            span: Span::saturating_from_parts_usize(offset.get() as usize, text.len()),
        }
    }

    fn word_at_offset(source: &'source str, offset: Offset, content_span: Span) -> Self {
        let start = (offset.get() as usize).min(source.len());
        let content_end = content_span.end_usize().min(source.len());
        if start >= content_end {
            return Self::new("", offset);
        }

        let text = &source[start..content_end];
        let suffix_len = text
            .char_indices()
            .find_map(|(index, character)| {
                (character.is_whitespace() || matches!(character, '%' | '}')).then_some(index)
            })
            .unwrap_or(text.len());

        Self::new(&text[..suffix_len], offset)
    }

    fn quoted_at_offset(
        source: &'source str,
        offset: Offset,
        content_span: Span,
        quote: char,
    ) -> (Self, bool, TagClose) {
        let start = (offset.get() as usize).min(source.len());
        let content_end = content_span.end_usize().min(source.len());
        if start >= content_end {
            return (
                Self::new("", offset),
                false,
                TagClose::after_offset(source, offset, content_span),
            );
        }

        let text = &source[start..content_end];
        if let Some((closing_index, _)) = text.char_indices().find(|(index, character)| {
            *character == quote && !is_escaped(source.as_bytes(), start + index)
        }) {
            let after_quote = start + closing_index + quote.len_utf8();
            return (
                Self::new(&text[..closing_index], offset),
                true,
                TagClose::after_position(source, after_quote, content_span),
            );
        }

        let close = TagClose::after_offset(source, offset, content_span);
        let suffix =
            if matches!(close, TagClose::Full { .. }) && text.chars().all(char::is_whitespace) {
                ""
            } else {
                text
            };

        (Self::new(suffix, offset), false, close)
    }
}

fn token_at_offset(tokens: &[Token], offset: Offset) -> Option<&Token> {
    tokens.iter().find(|token| {
        let Some(full_span) = token.full_span() else {
            return false;
        };

        full_span.contains(offset)
            || matches!(token, Token::Error { .. }) && full_span.end_offset() == offset
    })
}

fn content_before_offset(source: &str, content_span: Span, offset: Offset) -> &str {
    let start = content_span.start_usize().min(source.len());
    let end = content_span.end_usize().min(source.len());
    if start > end {
        return "";
    }

    let content = &source[start..end];
    let mut content_offset = (offset.get() as usize)
        .saturating_sub(content_span.start_usize())
        .min(content.len());
    while content_offset > 0 && !content.is_char_boundary(content_offset) {
        content_offset -= 1;
    }

    &content[..content_offset]
}

fn ends_in_unquoted_whitespace(text: &str) -> bool {
    let mut quote = None;
    let mut last = None;
    let bytes = text.as_bytes();

    for (index, character) in text.char_indices() {
        match character {
            '\'' | '"'
                if (quote.is_none() || quote == Some(character)) && !is_escaped(bytes, index) =>
            {
                quote = if quote == Some(character) {
                    None
                } else {
                    Some(character)
                };
            }
            _ => {}
        }
        last = Some(character);
    }

    quote.is_none() && last.is_some_and(char::is_whitespace)
}

fn split_tag_words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut word_start = None;
    let mut quote = None;
    let bytes = text.as_bytes();

    for (index, character) in text.char_indices() {
        if word_start.is_none() {
            if character.is_whitespace() {
                continue;
            }
            word_start = Some(index);
        }

        match character {
            '\'' | '"'
                if (quote.is_none() || quote == Some(character)) && !is_escaped(bytes, index) =>
            {
                quote = if quote == Some(character) {
                    None
                } else {
                    Some(character)
                };
            }
            character if character.is_whitespace() && quote.is_none() => {
                if let Some(start) = word_start.take() {
                    words.push(&text[start..index]);
                }
            }
            _ => {}
        }
    }

    if let Some(start) = word_start {
        words.push(&text[start..]);
    }

    words
}

fn unclosed_quote_prefix(text: &str) -> Option<(char, &str)> {
    let mut quote = None;
    let mut quote_start = 0;
    let bytes = text.as_bytes();

    for (index, character) in text.char_indices() {
        match character {
            '\'' | '"'
                if (quote.is_none() || quote == Some(character)) && !is_escaped(bytes, index) =>
            {
                if quote == Some(character) {
                    quote = None;
                } else {
                    quote = Some(character);
                    quote_start = index;
                }
            }
            _ => {}
        }
    }

    quote.map(|quote| (quote, &text[quote_start + quote.len_utf8()..]))
}

fn is_escaped(bytes: &[u8], index: usize) -> bool {
    bytes[..index]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 1
}

fn find_last_unquoted_pipe(text: &str) -> Option<usize> {
    let mut last_pipe = None;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let bytes = text.as_bytes();

    for (i, ch) in text.char_indices() {
        match ch {
            '\'' if !in_double_quote => {
                let backslashes = bytes[..i].iter().rev().take_while(|&&b| b == b'\\').count();
                if backslashes % 2 == 0 {
                    in_single_quote = !in_single_quote;
                }
            }
            '"' if !in_single_quote => {
                let backslashes = bytes[..i].iter().rev().take_while(|&&b| b == b'\\').count();
                if backslashes % 2 == 0 {
                    in_double_quote = !in_double_quote;
                }
            }
            '|' if !in_single_quote && !in_double_quote => last_pipe = Some(i),
            _ => {}
        }
    }

    last_pipe
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURSOR: &str = "▮";

    fn source_with_offset_marker(input: &str) -> (String, Offset) {
        let offset = input
            .find(CURSOR)
            .expect("test source should contain the expected text");
        let mut source = input.to_string();
        source.replace_range(offset..offset + CURSOR.len(), "");
        (
            source,
            Offset::new(u32::try_from(offset).expect("test source offset should fit in u32")),
        )
    }

    fn with_syntax_context<R>(
        input: &str,
        assert: impl for<'source> FnOnce(&'source str, CompletionOffsetContext<'source>) -> R,
    ) -> R {
        let (source, offset) = source_with_offset_marker(input);
        let tokens = djls_templates::lex_template_impl(source.as_str());
        let context =
            CompletionOffsetContext::new(FileKind::Template, source.as_str(), &tokens, offset);
        assert(source.as_str(), context)
    }

    #[test]
    fn unsupported_files_have_no_syntax_context() {
        let source = "print('hello')";

        assert_eq!(
            CompletionOffsetContext::new(FileKind::Python, source, &[], Offset::new(0),),
            CompletionOffsetContext::None,
        );
    }

    #[test]
    fn open_tag_starts_tag_name_syntax_context() {
        with_syntax_context("{%▮", |_, context| {
            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::TagName {
                    prefix: OffsetPrefix {
                        text: "",
                        span: Span::new(2, 0),
                    },
                    needs_leading_space: true,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn tag_name_syntax_context_tracks_prefix_and_span() {
        with_syntax_context("{% sta▮", |_, context| {
            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::TagName {
                    prefix: OffsetPrefix {
                        text: "sta",
                        span: Span::new(3, 3),
                    },
                    needs_leading_space: false,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn tag_argument_syntax_context_tracks_tag_position_and_prefix() {
        with_syntax_context("{% include \"base.html\" wi▮", |source, context| {
            let prefix_start = source
                .find("wi")
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::TagArgument {
                    tag: "include",
                    position: 1,
                    prefix: OffsetPrefix {
                        text: "wi",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            2
                        ),
                    },
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn tag_argument_syntax_context_counts_quoted_whitespace_as_one_argument() {
        with_syntax_context(
            "{% include \"base layout.html\" wi▮",
            |source, context| {
                let prefix_start = source
                    .find("wi")
                    .expect("test source should contain the expected text");

                assert_eq!(
                    context,
                    CompletionOffsetContext::Template(TemplateCompletionContext::TagArgument {
                        tag: "include",
                        position: 1,
                        prefix: OffsetPrefix {
                            text: "wi",
                            span: Span::new(
                                u32::try_from(prefix_start)
                                    .expect("test source offset should fit in u32"),
                                2
                            ),
                        },
                        close: TagClose::None,
                    })
                );
            },
        );
    }

    #[test]
    fn quoted_argument_syntax_context_keeps_trailing_whitespace_inside_open_quote() {
        with_syntax_context("{% include \"base layout ▮", |source, context| {
            let prefix = "base layout ";
            let prefix_start = source
                .find(prefix)
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "include",
                    position: 0,
                    quote: '"',
                    prefix: OffsetPrefix {
                        text: prefix,
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            u32::try_from(prefix.len())
                                .expect("test source offset should fit in u32"),
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + prefix.len())
                                .expect("test source offset should fit in u32"),
                            0,
                        ),
                    },
                    closed: false,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn quoted_argument_syntax_context_tracks_prefix() {
        with_syntax_context("{% extends \"ba▮", |source, context| {
            let prefix_start = source
                .find("ba")
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "extends",
                    position: 0,
                    quote: '"',
                    prefix: OffsetPrefix {
                        text: "ba",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            2
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + 2)
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    closed: false,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn quoted_argument_syntax_context_tracks_slash_prefix() {
        with_syntax_context("{% include \"partials/na▮", |source, context| {
            let prefix = "partials/na";
            let prefix_start = source
                .find(prefix)
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "include",
                    position: 0,
                    quote: '"',
                    prefix: OffsetPrefix {
                        text: prefix,
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            u32::try_from(prefix.len())
                                .expect("test source offset should fit in u32"),
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + prefix.len())
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    closed: false,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn quoted_argument_syntax_context_preserves_existing_full_close() {
        with_syntax_context("{% extends \"ba▮ %}", |source, context| {
            let prefix_start = source
                .find("ba")
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "extends",
                    position: 0,
                    quote: '"',
                    prefix: OffsetPrefix {
                        text: "ba",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            2
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + 2)
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    closed: false,
                    close: TagClose::Full {
                        replacement_suffix_len: 3,
                    },
                })
            );
        });
    }

    #[test]
    fn quoted_argument_syntax_context_tracks_closed_suffix() {
        with_syntax_context("{% extends \"ba▮se.html\" %}", |source, context| {
            let prefix_start = source
                .find("ba")
                .expect("test source should contain the expected text");
            let suffix_start = prefix_start + 2;

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "extends",
                    position: 0,
                    quote: '"',
                    prefix: OffsetPrefix {
                        text: "ba",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            2
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "se.html",
                        span: Span::new(
                            u32::try_from(suffix_start)
                                .expect("test source offset should fit in u32"),
                            7
                        ),
                    },
                    closed: true,
                    close: TagClose::Full {
                        replacement_suffix_len: 3,
                    },
                })
            );
        });
    }

    #[test]
    fn quoted_argument_syntax_context_supports_single_quotes() {
        with_syntax_context("{% extends 'ba▮", |source, context| {
            let prefix_start = source
                .find("ba")
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "extends",
                    position: 0,
                    quote: '\'',
                    prefix: OffsetPrefix {
                        text: "ba",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            2
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + 2)
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    closed: false,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn quoted_argument_syntax_context_tracks_later_quoted_argument_positions() {
        with_syntax_context("{% include \"a.html\" with x=\"▮", |source, context| {
            let prefix_start = source
                .find("x=\"")
                .expect("test source should contain the expected text")
                + "x=\"".len();

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "include",
                    position: 2,
                    quote: '"',
                    prefix: OffsetPrefix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    closed: false,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn quoted_argument_syntax_context_ignores_escaped_quotes() {
        with_syntax_context(r#"{% extends "ba\"se▮"#, |source, context| {
            let prefix = r#"ba\"se"#;
            let prefix_start = source
                .find(prefix)
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
                    tag: "extends",
                    position: 0,
                    quote: '"',
                    prefix: OffsetPrefix {
                        text: prefix,
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            u32::try_from(prefix.len())
                                .expect("test source offset should fit in u32"),
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + prefix.len())
                                .expect("test source offset should fit in u32"),
                            0,
                        ),
                    },
                    closed: false,
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn closed_quoted_argument_stays_tag_argument_after_quote() {
        with_syntax_context("{% extends \"base.html\"▮", |source, context| {
            let prefix = "\"base.html\"";
            let prefix_start = source
                .find(prefix)
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::TagArgument {
                    tag: "extends",
                    position: 0,
                    prefix: OffsetPrefix {
                        text: prefix,
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            u32::try_from(prefix.len())
                                .expect("test source offset should fit in u32"),
                        ),
                    },
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn cursor_before_quote_stays_tag_argument() {
        with_syntax_context("{% extends ▮\"base.html\"", |_, context| {
            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::TagArgument {
                    tag: "extends",
                    position: 0,
                    prefix: OffsetPrefix {
                        text: "",
                        span: Span::new(11, 0),
                    },
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn load_tag_uses_library_name_syntax_context() {
        with_syntax_context("{% load stat▮", |source, context| {
            let prefix_start = source
                .find("stat")
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
                    prefix: OffsetPrefix {
                        text: "stat",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            4
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + 4)
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn selective_load_symbol_uses_load_symbol_syntax_context() {
        with_syntax_context("{% load trans▮ from i18n %}", |source, context| {
            let prefix_start = source
                .find("trans")
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LoadSymbol {
                    prefix: OffsetPrefix {
                        text: "trans",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            5
                        ),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(
                            u32::try_from(prefix_start + 5)
                                .expect("test source offset should fit in u32"),
                            0
                        ),
                    },
                    library: Some("i18n"),
                    needs_trailing_space: false,
                })
            );
        });
    }

    #[test]
    fn selective_load_from_keyword_stays_text_syntax_context() {
        with_syntax_context("{% load trans f▮rom i18n %}", |_, context| {
            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::Text)
            );
        });
    }

    #[test]
    fn selective_load_from_library_uses_library_name_syntax_context() {
        with_syntax_context("{% load trans from i▮ %}", |source, context| {
            let prefix_start = source
                .find('i')
                .expect("test source should contain the expected text");

            assert!(matches!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
                    prefix: OffsetPrefix {
                        text: "i",
                        span,
                    },
                    suffix: OffsetSuffix { text: "", .. },
                    close: TagClose::Full { .. },
                }) if span == Span::new(u32::try_from(prefix_start).expect("test source offset should fit in u32"), 1)
            ));
        });
    }

    #[test]
    fn selective_load_from_library_tracks_source_suffix_at_token_start() {
        with_syntax_context("{% load trans from ▮i18n %}", |source, context| {
            let suffix_start = source
                .find("i18n")
                .expect("test source should contain the expected text");

            assert!(matches!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
                    prefix: OffsetPrefix { text: "", .. },
                    suffix: OffsetSuffix {
                        text: "i18n",
                        span,
                    },
                    close: TagClose::Full { .. },
                }) if span == Span::new(u32::try_from(suffix_start).expect("test source offset should fit in u32"), 4)
            ));
        });
    }

    #[test]
    fn tag_syntax_context_reports_partial_and_full_closes_after_offset() {
        with_syntax_context("{% load stat▮}", |_, context| {
            assert!(matches!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
                    close: TagClose::Partial { .. },
                    ..
                })
            ));
        });
        with_syntax_context("{% load stat▮ %}", |_, context| {
            assert!(matches!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
                    close: TagClose::Full { .. },
                    ..
                })
            ));
        });
        with_syntax_context("{% load stat▮ }", |_, context| {
            assert!(matches!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
                    close: TagClose::Partial {
                        replacement_suffix_len: 2,
                    },
                    ..
                })
            ));
        });
    }

    #[test]
    fn tag_syntax_context_does_not_treat_later_brace_as_partial_close() {
        with_syntax_context("{% load stat▮\n} plain", |_, context| {
            assert!(matches!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
                    close: TagClose::None,
                    ..
                })
            ));
        });
    }

    #[test]
    fn variable_pipe_uses_filter_syntax_context() {
        with_syntax_context("{{ value|def▮", |source, context| {
            let prefix_start = source
                .find("def")
                .expect("test source should contain the expected text");

            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::Filter {
                    prefix: OffsetPrefix {
                        text: "def",
                        span: Span::new(
                            u32::try_from(prefix_start)
                                .expect("test source offset should fit in u32"),
                            3
                        ),
                    },
                })
            );
        });
    }

    #[test]
    fn quoted_pipe_stays_template_text_syntax_context() {
        with_syntax_context("{{ value:'a|b'▮", |_, context| {
            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::Text),
            );
        });
    }

    #[test]
    fn closed_tag_stays_template_text_syntax_context() {
        with_syntax_context("{% load static %}▮", |_, context| {
            assert_eq!(
                context,
                CompletionOffsetContext::Template(TemplateCompletionContext::Text),
            );
        });
    }
}
