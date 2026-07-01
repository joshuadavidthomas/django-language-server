use djls_project::TemplateName;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::TagBit;
use djls_templates::parse_template;

use crate::db::Db as SemanticDb;
use crate::references::LiteralTemplateReference;
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
