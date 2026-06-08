use djls_semantic::LoadKind;
use djls_source::File;
use djls_source::FileKind;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;
use djls_templates::Token;
use djls_templates::parse_template;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResolvedOffsetContext {
    TemplateReference { name: String, span: Span },
    LoadLibrary { name: String, span: Span },
    LoadSymbol { name: String, span: Span },
    Tag { name: String, span: Span },
    Filter { name: String, span: Span },
    Variable { name: String, span: Span },
    None,
}

impl ResolvedOffsetContext {
    pub(crate) fn from_offset(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Self {
        let Some(nodelist) = parse_template(db, file) else {
            return Self::None;
        };

        let Some(node) = nodelist.node_at(db, offset) else {
            return Self::None;
        };

        Self::from_node(node, offset)
    }

    pub(crate) fn from_node(node: &Node, offset: Offset) -> Self {
        match node {
            Node::Tag {
                name,
                name_span,
                bits,
                ..
            } => Self::from_tag(name, *name_span, bits, offset),
            Node::Variable {
                var,
                var_span,
                filters,
                ..
            } => {
                if let Some(filter) = filters.iter().find(|filter| {
                    filter
                        .span
                        .with_length_usize_saturating(filter.name.len())
                        .contains(offset)
                }) {
                    let span = filter.span.with_length_usize_saturating(filter.name.len());
                    return Self::Filter {
                        name: filter.name.clone(),
                        span,
                    };
                }

                if var_span.contains(offset) {
                    return Self::Variable {
                        name: var.clone(),
                        span: *var_span,
                    };
                }

                Self::None
            }
            Node::Comment { .. } | Node::Text { .. } | Node::Error { .. } => Self::None,
        }
    }

    fn from_tag(name: &str, name_span: Span, bits: &[TagBit], offset: Offset) -> Self {
        if name_span.contains(offset) {
            return Self::Tag {
                name: name.to_string(),
                span: name_span,
            };
        }

        match name {
            "extends" | "include" => {
                bits.first()
                    .filter(|bit| bit.span.contains(offset))
                    .map_or(Self::None, |bit| Self::TemplateReference {
                        name: bit.template_string().value().to_string(),
                        span: bit.span,
                    })
            }

            "load" => {
                let Some(load_kind) = LoadKind::from_tag(name, bits) else {
                    return Self::None;
                };

                match load_kind {
                    LoadKind::FullLoad { libraries } => libraries
                        .into_iter()
                        .find(|library| library.span().contains(offset))
                        .map_or(Self::None, |library| Self::LoadLibrary {
                            name: library.as_str().to_string(),
                            span: library.span(),
                        }),
                    LoadKind::SelectiveImport { symbols, library } => {
                        if library.span().contains(offset) {
                            return Self::LoadLibrary {
                                name: library.as_str().to_string(),
                                span: library.span(),
                            };
                        }

                        symbols
                            .into_iter()
                            .find(|symbol| symbol.span().contains(offset))
                            .map_or(Self::None, |symbol| Self::LoadSymbol {
                                name: symbol.as_str().to_string(),
                                span: symbol.span(),
                            })
                    }
                }
            }

            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OffsetSyntaxContext<'source> {
    Template(TemplateOffsetSyntaxContext<'source>),
    None,
}

impl<'source> OffsetSyntaxContext<'source> {
    pub(crate) fn new(
        kind: FileKind,
        source: &'source str,
        tokens: &[Token],
        offset: Offset,
    ) -> Self {
        match kind {
            FileKind::Template => Self::Template(TemplateOffsetSyntaxContext::from_tokens(
                source, tokens, offset,
            )),
            FileKind::Python | FileKind::Other => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplateOffsetSyntaxContext<'source> {
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

impl<'source> TemplateOffsetSyntaxContext<'source> {
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
        let offset = (offset.get() as usize).min(source.len());
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
    use djls_templates::Filter;

    use super::*;

    const CURSOR: &str = "▮";

    fn content_span(source: &str) -> Span {
        Span::saturating_from_parts_usize(3, source.len() - 6)
    }

    fn offset_of(source: &str, needle: &str) -> Offset {
        Offset::new(u32::try_from(source.find(needle).unwrap()).unwrap())
    }

    fn parsed_tag(source: &str) -> Node {
        let (nodes, errors) = djls_templates::parse_template_impl(source);
        assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");
        nodes.into_iter().next().expect("expected one node")
    }

    fn source_with_offset_marker(input: &str) -> (String, Offset) {
        let offset = input.find(CURSOR).unwrap();
        let mut source = input.to_string();
        source.replace_range(offset..offset + CURSOR.len(), "");
        (source, Offset::new(u32::try_from(offset).unwrap()))
    }

    fn with_syntax_context<R>(
        input: &str,
        assert: impl for<'source> FnOnce(&'source str, OffsetSyntaxContext<'source>) -> R,
    ) -> R {
        let (source, offset) = source_with_offset_marker(input);
        let tokens = djls_templates::lex_template_impl(source.as_str());
        let context =
            OffsetSyntaxContext::new(FileKind::Template, source.as_str(), &tokens, offset);
        assert(source.as_str(), context)
    }

    #[test]
    fn unsupported_files_have_no_syntax_context() {
        let source = "print('hello')";

        assert_eq!(
            OffsetSyntaxContext::new(FileKind::Python, source, &[], Offset::new(0),),
            OffsetSyntaxContext::None,
        );
    }

    #[test]
    fn identifies_template_reference_context() {
        let source = r#"{% extends "base.html" %}"#;
        let node = parsed_tag(source);

        let context = ResolvedOffsetContext::from_node(&node, offset_of(source, "base"));

        assert_eq!(
            context,
            ResolvedOffsetContext::TemplateReference {
                name: "base.html".to_string(),
                span: Span::saturating_from_parts_usize(11, 11),
            }
        );
    }

    #[test]
    fn identifies_load_library_context() {
        let source = "{% load static i18n %}";
        let node = parsed_tag(source);

        let context = ResolvedOffsetContext::from_node(&node, offset_of(source, "static"));

        assert_eq!(
            context,
            ResolvedOffsetContext::LoadLibrary {
                name: "static".to_string(),
                span: Span::saturating_from_parts_usize(8, 6),
            }
        );
    }

    #[test]
    fn identifies_selective_load_symbol_context() {
        let source = "{% load trans blocktrans from i18n %}";
        let node = parsed_tag(source);

        let context = ResolvedOffsetContext::from_node(&node, offset_of(source, "blocktrans"));

        assert_eq!(
            context,
            ResolvedOffsetContext::LoadSymbol {
                name: "blocktrans".to_string(),
                span: Span::saturating_from_parts_usize(14, 10),
            }
        );
    }

    #[test]
    fn identifies_selective_load_library_context() {
        let source = "{% load trans from i18n %}";
        let node = parsed_tag(source);

        let context = ResolvedOffsetContext::from_node(&node, offset_of(source, "i18n"));

        assert_eq!(
            context,
            ResolvedOffsetContext::LoadLibrary {
                name: "i18n".to_string(),
                span: Span::saturating_from_parts_usize(19, 4),
            }
        );
    }

    #[test]
    fn identifies_tag_name_context() {
        let source = "{% if user %}";
        let node = parsed_tag(source);

        let context = ResolvedOffsetContext::from_node(&node, offset_of(source, "if"));

        assert_eq!(
            context,
            ResolvedOffsetContext::Tag {
                name: "if".to_string(),
                span: Span::saturating_from_parts_usize(3, 2),
            }
        );
    }

    #[test]
    fn ignores_unrecognized_tag_arguments() {
        let source = "{% if user %}";
        let node = parsed_tag(source);

        let context = ResolvedOffsetContext::from_node(&node, offset_of(source, "user"));

        assert_eq!(context, ResolvedOffsetContext::None);
    }

    #[test]
    fn identifies_filter_context() {
        let node = Node::Variable {
            var: "user.name".to_string(),
            var_span: Span::new(3, 9),
            filters: vec![Filter::new("title".to_string(), None, Span::new(13, 5))],
            span: content_span("{{ user.name|title }}"),
        };

        let context = ResolvedOffsetContext::from_node(&node, Offset::new(14));

        assert_eq!(
            context,
            ResolvedOffsetContext::Filter {
                name: "title".to_string(),
                span: Span::new(13, 5),
            }
        );
    }

    #[test]
    fn identifies_variable_context() {
        let node = Node::Variable {
            var: "user.name".to_string(),
            var_span: Span::new(3, 9),
            filters: vec![Filter::new("title".to_string(), None, Span::new(13, 5))],
            span: content_span("{{ user.name|title }}"),
        };

        let context = ResolvedOffsetContext::from_node(&node, Offset::new(5));

        assert_eq!(
            context,
            ResolvedOffsetContext::Variable {
                name: "user.name".to_string(),
                span: Span::new(3, 9),
            }
        );
    }

    #[test]
    fn open_tag_starts_tag_name_syntax_context() {
        with_syntax_context("{%▮", |_, context| {
            assert_eq!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::TagName {
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
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::TagName {
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
            let prefix_start = source.find("wi").unwrap();

            assert_eq!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::TagArgument {
                    tag: "include",
                    position: 1,
                    prefix: OffsetPrefix {
                        text: "wi",
                        span: Span::new(u32::try_from(prefix_start).unwrap(), 2),
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
                let prefix_start = source.find("wi").unwrap();

                assert_eq!(
                    context,
                    OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::TagArgument {
                        tag: "include",
                        position: 1,
                        prefix: OffsetPrefix {
                            text: "wi",
                            span: Span::new(u32::try_from(prefix_start).unwrap(), 2),
                        },
                        close: TagClose::None,
                    })
                );
            },
        );
    }

    #[test]
    fn tag_argument_syntax_context_keeps_trailing_whitespace_inside_open_quote() {
        with_syntax_context("{% include \"base layout ▮", |source, context| {
            let prefix = "\"base layout ";
            let prefix_start = source.find(prefix).unwrap();

            assert_eq!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::TagArgument {
                    tag: "include",
                    position: 0,
                    prefix: OffsetPrefix {
                        text: prefix,
                        span: Span::new(
                            u32::try_from(prefix_start).unwrap(),
                            u32::try_from(prefix.len()).unwrap(),
                        ),
                    },
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn load_tag_uses_library_name_syntax_context() {
        with_syntax_context("{% load stat▮", |source, context| {
            let prefix_start = source.find("stat").unwrap();

            assert_eq!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LibraryName {
                    prefix: OffsetPrefix {
                        text: "stat",
                        span: Span::new(u32::try_from(prefix_start).unwrap(), 4),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(u32::try_from(prefix_start + 4).unwrap(), 0),
                    },
                    close: TagClose::None,
                })
            );
        });
    }

    #[test]
    fn selective_load_symbol_uses_load_symbol_syntax_context() {
        with_syntax_context("{% load trans▮ from i18n %}", |source, context| {
            let prefix_start = source.find("trans").unwrap();

            assert_eq!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LoadSymbol {
                    prefix: OffsetPrefix {
                        text: "trans",
                        span: Span::new(u32::try_from(prefix_start).unwrap(), 5),
                    },
                    suffix: OffsetSuffix {
                        text: "",
                        span: Span::new(u32::try_from(prefix_start + 5).unwrap(), 0),
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
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::Text)
            );
        });
    }

    #[test]
    fn selective_load_from_library_uses_library_name_syntax_context() {
        with_syntax_context("{% load trans from i▮ %}", |source, context| {
            let prefix_start = source.find('i').unwrap();

            assert!(matches!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LibraryName {
                    prefix: OffsetPrefix {
                        text: "i",
                        span,
                    },
                    suffix: OffsetSuffix { text: "", .. },
                    close: TagClose::Full { .. },
                }) if span == Span::new(u32::try_from(prefix_start).unwrap(), 1)
            ));
        });
    }

    #[test]
    fn selective_load_from_library_tracks_source_suffix_at_token_start() {
        with_syntax_context("{% load trans from ▮i18n %}", |source, context| {
            let suffix_start = source.find("i18n").unwrap();

            assert!(matches!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LibraryName {
                    prefix: OffsetPrefix { text: "", .. },
                    suffix: OffsetSuffix {
                        text: "i18n",
                        span,
                    },
                    close: TagClose::Full { .. },
                }) if span == Span::new(u32::try_from(suffix_start).unwrap(), 4)
            ));
        });
    }

    #[test]
    fn tag_syntax_context_reports_partial_and_full_closes_after_offset() {
        with_syntax_context("{% load stat▮}", |_, context| {
            assert!(matches!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LibraryName {
                    close: TagClose::Partial { .. },
                    ..
                })
            ));
        });
        with_syntax_context("{% load stat▮ %}", |_, context| {
            assert!(matches!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LibraryName {
                    close: TagClose::Full { .. },
                    ..
                })
            ));
        });
        with_syntax_context("{% load stat▮ }", |_, context| {
            assert!(matches!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LibraryName {
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
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::LibraryName {
                    close: TagClose::None,
                    ..
                })
            ));
        });
    }

    #[test]
    fn variable_pipe_uses_filter_syntax_context() {
        with_syntax_context("{{ value|def▮", |source, context| {
            let prefix_start = source.find("def").unwrap();

            assert_eq!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::Filter {
                    prefix: OffsetPrefix {
                        text: "def",
                        span: Span::new(u32::try_from(prefix_start).unwrap(), 3),
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
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::Text),
            );
        });
    }

    #[test]
    fn closed_tag_stays_template_text_syntax_context() {
        with_syntax_context("{% load static %}▮", |_, context| {
            assert_eq!(
                context,
                OffsetSyntaxContext::Template(TemplateOffsetSyntaxContext::Text),
            );
        });
    }
}
