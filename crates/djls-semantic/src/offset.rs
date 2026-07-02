use djls_project::TemplateName;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::parse_template;

use crate::db::Db as SemanticDb;
use crate::references::LiteralTemplateReference;
use crate::scoping::LoadKind;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::ActiveTemplateVariable;
use crate::structure::active_template_nodes;
use crate::structure::build_template_tree;
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

        let tree = build_template_tree(db, nodelist);
        let tag_specs = db.tag_specs();

        for node in active_template_nodes(tree.regions(db), tree.root(db)) {
            let context = match node {
                ActiveTemplateNode::Tag(tag) if tag.full_span.contains(offset) => {
                    Self::from_tag(db, tag_specs, tag, offset)
                }
                ActiveTemplateNode::Variable(variable) if variable.span.contains(offset) => {
                    Self::from_variable(variable, offset)
                }
                ActiveTemplateNode::Tag(_) | ActiveTemplateNode::Variable(_) => Self::None,
            };

            if context != Self::None {
                return context;
            }
        }

        Self::None
    }

    fn from_variable(variable: ActiveTemplateVariable<'_>, offset: Offset) -> Self {
        for filter in variable.filters {
            let span = filter.span.with_length_usize_saturating(filter.name.len());
            if span.contains(offset) {
                return Self::Filter {
                    name: filter.name.clone(),
                    span,
                };
            }
        }

        if variable.var_span.contains(offset) {
            return Self::Variable {
                name: variable.var.to_string(),
                span: variable.var_span,
            };
        }

        Self::None
    }

    fn from_tag(
        db: &'db dyn SemanticDb,
        tag_specs: &TagSpecs,
        tag: ActiveTemplateTag<'_>,
        offset: Offset,
    ) -> Self {
        if tag.name_span.contains(offset) {
            return Self::Tag {
                name: tag.tag.to_string(),
                span: tag.name_span,
            };
        }

        match tag.tag {
            "load" => {
                let Some(load_kind) = LoadKind::from_tag(tag.tag, tag.bits) else {
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

            _ => LiteralTemplateReference::from_tag(tag_specs, tag.tag, tag.bits)
                .filter(|reference| reference.bit_span.contains(offset))
                .map_or(Self::None, |reference| Self::TemplateReference {
                    name: TemplateName::new(db, reference.template_name.to_string()),
                    span: reference.span,
                }),
        }
    }
}
