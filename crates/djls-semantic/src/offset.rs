use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::TagBit;
use djls_templates::parse_template;

use crate::db::Db as SemanticDb;
use crate::resolution::LiteralTemplateReference;
use crate::resolution::TemplateName;
use crate::scoping::LoadKind;
use crate::tags::TagSpecs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticOffsetContext<'db> {
    TemplateReference { name: TemplateName<'db>, span: Span },
    LoadLibrary { name: String, span: Span },
    LoadSymbol { name: String, span: Span },
    Tag { name: String, span: Span },
    Filter { name: String, span: Span },
    Variable { name: String, span: Span },
    None,
}

impl<'db> SemanticOffsetContext<'db> {
    pub fn from_offset(db: &'db dyn SemanticDb, file: File, offset: Offset) -> Self {
        let Some(nodelist) = parse_template(db, file) else {
            return Self::None;
        };

        let Some(node) = nodelist.node_at(db, offset) else {
            return Self::None;
        };

        let tag_specs = db.tag_specs();

        match node {
            Node::Tag {
                name,
                name_span,
                bits,
                ..
            } => Self::from_tag(db, tag_specs, name, *name_span, bits, offset),
            Node::Variable {
                var,
                var_span,
                filters,
                ..
            } => {
                for filter in filters {
                    let span = filter.span.with_length_usize_saturating(filter.name.len());
                    if span.contains(offset) {
                        return Self::Filter {
                            name: filter.name.clone(),
                            span,
                        };
                    }
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

    fn from_tag(
        db: &'db dyn SemanticDb,
        tag_specs: &TagSpecs,
        name: &str,
        name_span: Span,
        bits: &[TagBit],
        offset: Offset,
    ) -> Self {
        if name_span.contains(offset) {
            return Self::Tag {
                name: name.to_string(),
                span: name_span,
            };
        }

        match name {
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

            _ => LiteralTemplateReference::from_tag(tag_specs, name, bits)
                .filter(|reference| reference.span.contains(offset))
                .map_or(Self::None, |reference| Self::TemplateReference {
                    name: TemplateName::new(db, reference.template_name.to_string()),
                    span: reference.span,
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::*;
    use crate::testing::TestDatabase;

    fn offset_of(source: &str, needle: &str) -> Offset {
        Offset::new(u32::try_from(source.find(needle).unwrap()).unwrap())
    }

    fn context_for_source<'db>(
        db: &'db TestDatabase,
        source: &str,
        offset: Offset,
    ) -> SemanticOffsetContext<'db> {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.get_or_create_file(Utf8Path::new(path));
        SemanticOffsetContext::from_offset(db, file, offset)
    }

    #[test]
    fn identifies_template_reference_context() {
        let db = TestDatabase::new();
        let source = r#"{% extends "base.html" %}"#;

        let context = context_for_source(&db, source, offset_of(source, "base"));

        assert_eq!(
            context,
            SemanticOffsetContext::TemplateReference {
                name: TemplateName::new(&db, "base.html".to_string()),
                span: Span::saturating_from_parts_usize(11, 11),
            }
        );
    }

    #[test]
    fn ignores_dynamic_template_reference_context() {
        let db = TestDatabase::new();
        let source = "{% include partial_name %}";

        let context = context_for_source(&db, source, offset_of(source, "partial"));

        assert_eq!(context, SemanticOffsetContext::None);
    }

    #[test]
    fn identifies_load_library_context() {
        let db = TestDatabase::new();
        let source = "{% load static i18n %}";

        let context = context_for_source(&db, source, offset_of(source, "static"));

        assert_eq!(
            context,
            SemanticOffsetContext::LoadLibrary {
                name: "static".to_string(),
                span: Span::saturating_from_parts_usize(8, 6),
            }
        );
    }

    #[test]
    fn identifies_selective_load_symbol_context() {
        let db = TestDatabase::new();
        let source = "{% load trans blocktrans from i18n %}";

        let context = context_for_source(&db, source, offset_of(source, "blocktrans"));

        assert_eq!(
            context,
            SemanticOffsetContext::LoadSymbol {
                name: "blocktrans".to_string(),
                span: Span::saturating_from_parts_usize(14, 10),
            }
        );
    }

    #[test]
    fn identifies_selective_load_library_context() {
        let db = TestDatabase::new();
        let source = "{% load trans from i18n %}";

        let context = context_for_source(&db, source, offset_of(source, "i18n"));

        assert_eq!(
            context,
            SemanticOffsetContext::LoadLibrary {
                name: "i18n".to_string(),
                span: Span::saturating_from_parts_usize(19, 4),
            }
        );
    }

    #[test]
    fn identifies_tag_name_context() {
        let db = TestDatabase::new();
        let source = "{% if user %}";

        let context = context_for_source(&db, source, offset_of(source, "if"));

        assert_eq!(
            context,
            SemanticOffsetContext::Tag {
                name: "if".to_string(),
                span: Span::saturating_from_parts_usize(3, 2),
            }
        );
    }

    #[test]
    fn ignores_unrecognized_tag_arguments() {
        let db = TestDatabase::new();
        let source = "{% if user %}";

        let context = context_for_source(&db, source, offset_of(source, "user"));

        assert_eq!(context, SemanticOffsetContext::None);
    }

    #[test]
    fn identifies_filter_context() {
        let db = TestDatabase::new();
        let source = "{{ user.name|title }}";

        let context = context_for_source(&db, source, offset_of(source, "title"));

        assert_eq!(
            context,
            SemanticOffsetContext::Filter {
                name: "title".to_string(),
                span: Span::new(13, 5),
            }
        );
    }

    #[test]
    fn identifies_variable_context() {
        let db = TestDatabase::new();
        let source = "{{ user.name|title }}";

        let context = context_for_source(&db, source, offset_of(source, "user"));

        assert_eq!(
            context,
            SemanticOffsetContext::Variable {
                name: "user.name".to_string(),
                span: Span::new(3, 9),
            }
        );
    }
}
