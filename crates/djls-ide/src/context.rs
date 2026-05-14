use djls_semantic::LoadKind;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::parse_template;
use djls_templates::Filter;
use djls_templates::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OffsetContext {
    TemplateReference {
        name: String,
        span: Span,
    },
    LoadLibrary {
        name: String,
        span: Span,
    },
    LoadSymbol {
        name: String,
        span: Span,
    },
    BlockDefinition {
        name: String,
        span: Span,
    },
    BlockReference {
        name: String,
        span: Span,
    },
    Tag {
        name: String,
        span: Span,
    },
    Filter {
        name: String,
        span: Span,
    },
    Variable {
        name: String,
        filters: Vec<Filter>,
        span: Span,
    },
    Comment {
        content: String,
        span: Span,
    },
    Text {
        span: Span,
    },
    None,
}

impl OffsetContext {
    pub(crate) fn from_offset(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Self {
        let Some(nodelist) = parse_template(db, file) else {
            return Self::None;
        };

        let source = file.source(db);
        let Some(node) = nodelist.node_at(db, offset) else {
            return Self::None;
        };

        Self::from_node(node, source.as_str(), offset)
    }

    pub(crate) fn from_node(node: &Node, source: &str, offset: Offset) -> Self {
        match node {
            Node::Tag { name, bits, span } => Self::from_tag(
                name,
                bits,
                *span,
                node.identifier_span().unwrap_or(*span),
                source,
                offset,
            ),
            Node::Variable { var, filters, span } => {
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

                if node
                    .identifier_span()
                    .is_some_and(|span| span.contains(offset))
                {
                    Self::Variable {
                        name: var.clone(),
                        filters: filters.clone(),
                        span: *span,
                    }
                } else {
                    Self::None
                }
            }
            Node::Comment { content, span } => Self::Comment {
                content: content.clone(),
                span: *span,
            },
            Node::Text { span } => Self::Text { span: *span },
            Node::Error { .. } => Self::None,
        }
    }

    fn from_tag(
        name: &str,
        bits: &[String],
        span: Span,
        name_span: Span,
        source: &str,
        offset: Offset,
    ) -> Self {
        if name_span.contains(offset) {
            return Self::Tag {
                name: name.to_string(),
                span: name_span,
            };
        }

        let first_bit = bits
            .first()
            .and_then(|bit| bit_span(source, span, bit, 0).map(|(_, span)| (bit, span)))
            .filter(|(_, span)| span.contains(offset));

        match name {
            "extends" | "include" => {
                first_bit.map_or(Self::None, |(bit, span)| Self::TemplateReference {
                    name: strip_template_reference_quotes(bit).to_string(),
                    span,
                })
            }

            "load" => {
                let Some(load_kind) = djls_semantic::parse_load_bits(bits) else {
                    return Self::None;
                };

                let mut search_start = 0;
                for bit in bits {
                    let Some((relative_start, span)) = bit_span(source, span, bit, search_start)
                    else {
                        continue;
                    };

                    match &load_kind {
                        LoadKind::FullLoad { libraries }
                            if libraries.iter().any(|library| library == bit)
                                && span.contains(offset) =>
                        {
                            return Self::LoadLibrary {
                                name: bit.clone(),
                                span,
                            };
                        }
                        LoadKind::SelectiveImport { library, .. }
                            if library == bit && span.contains(offset) =>
                        {
                            return Self::LoadLibrary {
                                name: bit.clone(),
                                span,
                            };
                        }
                        LoadKind::SelectiveImport { symbols, .. }
                            if symbols.iter().any(|symbol| symbol == bit)
                                && span.contains(offset) =>
                        {
                            return Self::LoadSymbol {
                                name: bit.clone(),
                                span,
                            };
                        }
                        LoadKind::FullLoad { .. } | LoadKind::SelectiveImport { .. } => {}
                    }

                    search_start = relative_start + bit.len();
                }

                Self::None
            }

            "block" => first_bit.map_or(Self::None, |(bit, span)| Self::BlockDefinition {
                name: bit.clone(),
                span,
            }),

            "endblock" => first_bit.map_or(Self::None, |(bit, span)| Self::BlockReference {
                name: bit.clone(),
                span,
            }),

            _ => Self::None,
        }
    }
}

fn bit_span(source: &str, tag_span: Span, bit: &str, search_start: usize) -> Option<(usize, Span)> {
    let content_start = tag_span.start_usize();
    let content_end = tag_span.end_usize();
    let content = source.get(content_start..content_end)?;
    let relative_start = search_start + content[search_start..].find(bit)?;
    let span = Span::saturating_from_parts_usize(content_start + relative_start, bit.len());
    Some((relative_start, span))
}

pub(crate) fn strip_template_reference_quotes(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag_span(source: &str) -> Span {
        Span::saturating_from_parts_usize(3, source.len() - 6)
    }

    fn offset_of(source: &str, needle: &str) -> Offset {
        Offset::new(u32::try_from(source.find(needle).unwrap()).unwrap())
    }

    #[test]
    fn strip_template_reference_quotes_strips_double_quotes() {
        assert_eq!(
            strip_template_reference_quotes("\"base.html\""),
            "base.html"
        );
    }

    #[test]
    fn strip_template_reference_quotes_strips_single_quotes() {
        assert_eq!(strip_template_reference_quotes("'base.html'"), "base.html");
    }

    #[test]
    fn strip_template_reference_quotes_strips_quotes_and_whitespace() {
        assert_eq!(
            strip_template_reference_quotes("  \"base.html\"  "),
            "base.html"
        );
    }

    #[test]
    fn strip_template_reference_quotes_handles_unquoted() {
        assert_eq!(strip_template_reference_quotes("base.html"), "base.html");
    }

    #[test]
    fn tag_name_context_wins_on_tag_name() {
        let source = "{% extends \"base.html\" %}";
        let result = OffsetContext::from_tag(
            "extends",
            &["\"base.html\"".to_string()],
            tag_span(source),
            Span::new(3, 7),
            source,
            offset_of(source, "extends"),
        );

        assert!(matches!(
            result,
            OffsetContext::Tag { name, .. } if name == "extends"
        ));
    }

    #[test]
    fn extends_template_reference_context_on_template_name() {
        let source = "{% extends \"base.html\" %}";
        let result = OffsetContext::from_tag(
            "extends",
            &["\"base.html\"".to_string()],
            tag_span(source),
            Span::new(3, 7),
            source,
            offset_of(source, "base.html"),
        );

        assert!(matches!(
            result,
            OffsetContext::TemplateReference { name, .. } if name == "base.html"
        ));
    }

    #[test]
    fn include_template_reference_context() {
        let source = "{% include \"partial.html\" %}";
        let result = OffsetContext::from_tag(
            "include",
            &["\"partial.html\"".to_string()],
            tag_span(source),
            Span::new(3, 7),
            source,
            offset_of(source, "partial.html"),
        );

        assert!(matches!(
            result,
            OffsetContext::TemplateReference { name, .. } if name == "partial.html"
        ));
    }

    #[test]
    fn load_library_context() {
        let source = "{% load static i18n %}";
        let result = OffsetContext::from_tag(
            "load",
            &["static".to_string(), "i18n".to_string()],
            tag_span(source),
            Span::new(3, 4),
            source,
            offset_of(source, "static"),
        );

        assert!(matches!(
            result,
            OffsetContext::LoadLibrary { name, .. } if name == "static"
        ));
    }

    #[test]
    fn selective_load_symbol_and_library_contexts() {
        let source = "{% load trans from i18n %}";
        let bits = ["trans".to_string(), "from".to_string(), "i18n".to_string()];

        let symbol = OffsetContext::from_tag(
            "load",
            &bits,
            tag_span(source),
            Span::new(3, 4),
            source,
            offset_of(source, "trans"),
        );
        let library = OffsetContext::from_tag(
            "load",
            &bits,
            tag_span(source),
            Span::new(3, 4),
            source,
            offset_of(source, "i18n"),
        );

        assert!(matches!(
            symbol,
            OffsetContext::LoadSymbol { name, .. } if name == "trans"
        ));
        assert!(matches!(
            library,
            OffsetContext::LoadLibrary { name, .. } if name == "i18n"
        ));
    }

    #[test]
    fn block_contexts_apply_to_block_name() {
        let source = "{% block content %}";
        let result = OffsetContext::from_tag(
            "block",
            &["content".to_string()],
            tag_span(source),
            Span::new(3, 5),
            source,
            offset_of(source, "content"),
        );

        assert!(matches!(
            result,
            OffsetContext::BlockDefinition { name, .. } if name == "content"
        ));
    }

    #[test]
    fn endblock_contexts_apply_to_block_name() {
        let source = "{% endblock content %}";
        let result = OffsetContext::from_tag(
            "endblock",
            &["content".to_string()],
            tag_span(source),
            Span::new(3, 8),
            source,
            offset_of(source, "content"),
        );

        assert!(matches!(
            result,
            OffsetContext::BlockReference { name, .. } if name == "content"
        ));
    }

    #[test]
    fn generic_tag_argument_is_not_a_context() {
        let source = "{% if user.is_authenticated %}";
        let result = OffsetContext::from_tag(
            "if",
            &["user.is_authenticated".to_string()],
            tag_span(source),
            Span::new(3, 2),
            source,
            offset_of(source, "user"),
        );

        assert_eq!(result, OffsetContext::None);
    }

    #[test]
    fn variable_filter_context() {
        let node = Node::Variable {
            var: "value".to_string(),
            filters: vec![Filter::new("title".to_string(), None, Span::new(9, 5))],
            span: Span::new(3, 11),
        };

        let result = OffsetContext::from_node(&node, "{{ value|title }}", Offset::new(10));

        assert!(matches!(
            result,
            OffsetContext::Filter { name, .. } if name == "title"
        ));
    }
}
