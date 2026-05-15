use djls_semantic::LoadKind;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::parse_template;
use djls_templates::Node;
use djls_templates::TagBit;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OffsetContext {
    TemplateReference { name: String, span: Span },
    LoadLibrary { name: String, span: Span },
    LoadSymbol { name: String, span: Span },
    Tag { name: String, span: Span },
    Filter { name: String, span: Span },
    Variable { name: String, span: Span },
    None,
}

impl OffsetContext {
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
                let Some(load_kind) = djls_semantic::parse_load_bits(bits) else {
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

#[cfg(test)]
mod tests {
    use djls_templates::Filter;

    use super::*;

    fn tag_span(source: &str) -> Span {
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

    #[test]
    fn identifies_template_reference_context() {
        let source = r#"{% extends "base.html" %}"#;
        let node = parsed_tag(source);

        let context = OffsetContext::from_node(&node, offset_of(source, "base"));

        assert_eq!(
            context,
            OffsetContext::TemplateReference {
                name: "base.html".to_string(),
                span: Span::saturating_from_parts_usize(11, 11),
            }
        );
    }

    #[test]
    fn identifies_load_library_context() {
        let source = "{% load static i18n %}";
        let node = parsed_tag(source);

        let context = OffsetContext::from_node(&node, offset_of(source, "static"));

        assert_eq!(
            context,
            OffsetContext::LoadLibrary {
                name: "static".to_string(),
                span: Span::saturating_from_parts_usize(8, 6),
            }
        );
    }

    #[test]
    fn identifies_selective_load_symbol_context() {
        let source = "{% load trans blocktrans from i18n %}";
        let node = parsed_tag(source);

        let context = OffsetContext::from_node(&node, offset_of(source, "blocktrans"));

        assert_eq!(
            context,
            OffsetContext::LoadSymbol {
                name: "blocktrans".to_string(),
                span: Span::saturating_from_parts_usize(14, 10),
            }
        );
    }

    #[test]
    fn identifies_selective_load_library_context() {
        let source = "{% load trans from i18n %}";
        let node = parsed_tag(source);

        let context = OffsetContext::from_node(&node, offset_of(source, "i18n"));

        assert_eq!(
            context,
            OffsetContext::LoadLibrary {
                name: "i18n".to_string(),
                span: Span::saturating_from_parts_usize(19, 4),
            }
        );
    }

    #[test]
    fn identifies_tag_name_context() {
        let source = "{% if user %}";
        let node = parsed_tag(source);

        let context = OffsetContext::from_node(&node, offset_of(source, "if"));

        assert_eq!(
            context,
            OffsetContext::Tag {
                name: "if".to_string(),
                span: Span::saturating_from_parts_usize(3, 2),
            }
        );
    }

    #[test]
    fn ignores_unrecognized_tag_arguments() {
        let source = "{% if user %}";
        let node = parsed_tag(source);

        let context = OffsetContext::from_node(&node, offset_of(source, "user"));

        assert_eq!(context, OffsetContext::None);
    }

    #[test]
    fn identifies_filter_context() {
        let node = Node::Variable {
            var: "user.name".to_string(),
            var_span: Span::new(3, 9),
            filters: vec![Filter::new("title".to_string(), None, Span::new(13, 5))],
            span: tag_span("{{ user.name|title }}"),
        };

        let context = OffsetContext::from_node(&node, Offset::new(14));

        assert_eq!(
            context,
            OffsetContext::Filter {
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
            span: tag_span("{{ user.name|title }}"),
        };

        let context = OffsetContext::from_node(&node, Offset::new(5));

        assert_eq!(
            context,
            OffsetContext::Variable {
                name: "user.name".to_string(),
                span: Span::new(3, 9),
            }
        );
    }
}
