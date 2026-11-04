use djls_project::TagRole;
use djls_project::TemplateName;
use djls_project::TemplateReferenceKind;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::TemplateParseResult;
use djls_templates::parse_template;

use crate::TagSpec;
use crate::db::Db as SemanticDb;
use crate::layout::MapSpans;
use crate::references::LiteralTemplateReference;
use crate::scoping::LoadKind;
use crate::scoping::LoadedLibraries;
use crate::scoping::ScopedTagFacts;
use crate::scoping::template_analysis_projection_for_file;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::ActiveTemplateVariable;
use crate::structure::StructuralOccurrenceMeaning;
use crate::structure::active_template_nodes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticOffsetContext<'db> {
    TemplateReference {
        name: TemplateName<'db>,
        kind: TemplateReferenceKind,
        span: Span,
    },
    LoadLibrary {
        name: String,
        span: Span,
    },
    LoadSymbol {
        name: String,
        library: String,
        span: Span,
    },
    TemplateBlock {
        name: String,
        span: Span,
    },
    Tag {
        name: String,
        loaded_libraries: Vec<String>,
        span: Span,
    },
    Filter {
        name: String,
        loaded_libraries: Vec<String>,
        span: Span,
    },
    Variable {
        name: String,
        span: Span,
    },
    None,
}

impl MapSpans for SemanticOffsetContext<'_> {
    fn map_spans(&mut self, map: &mut impl FnMut(Span) -> Span) {
        match self {
            Self::TemplateReference { span, .. }
            | Self::LoadLibrary { span, .. }
            | Self::LoadSymbol { span, .. }
            | Self::TemplateBlock { span, .. }
            | Self::Tag { span, .. }
            | Self::Filter { span, .. }
            | Self::Variable { span, .. } => span.map_spans(map),
            Self::None => {}
        }
    }
}

impl<'db> SemanticOffsetContext<'db> {
    pub fn from_offset(db: &'db dyn SemanticDb, file: File, offset: Offset) -> Self {
        let TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
            return Self::None;
        };

        let projection = template_analysis_projection_for_file(db, file, nodelist);
        projection.read_at(offset.get(), |analysis, offset| {
            let tree = analysis.tree(db);
            let loaded = analysis.loaded_libraries(db);
            let tag_facts = analysis.scoped_tag_facts(db);
            active_template_nodes(tree.regions(db), tree.root(db))
                .map(|node| match node {
                    ActiveTemplateNode::Tag(tag) if tag.full_span.contains(offset) => {
                        Self::from_tag(db, loaded, tag_facts, tag, offset)
                    }
                    ActiveTemplateNode::Variable(variable) if variable.span.contains(offset) => {
                        Self::from_variable(loaded, variable, offset)
                    }
                    ActiveTemplateNode::Tag(_) | ActiveTemplateNode::Variable(_) => Self::None,
                })
                .find(|context| *context != Self::None)
                .unwrap_or(Self::None)
        })
    }

    fn from_variable(
        loaded: &LoadedLibraries,
        variable: ActiveTemplateVariable<'_>,
        offset: Offset,
    ) -> Self {
        for filter in variable.filters {
            let span = filter.span.with_length_usize_saturating(filter.name.len());
            if span.contains(offset) {
                let loaded_libraries = loaded
                    .available_at(span.start())
                    .libraries_loading_symbol(&filter.name)
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                return Self::Filter {
                    name: filter.name.to_string(),
                    loaded_libraries,
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
        loaded: &LoadedLibraries,
        tag_facts: &ScopedTagFacts,
        tag: ActiveTemplateTag<'_>,
        offset: Offset,
    ) -> Self {
        let load_state = loaded.available_at(tag.span.start());
        if tag.name_span.contains(offset) {
            return match tag.structural_meaning {
                StructuralOccurrenceMeaning::Definition => {
                    let loaded_libraries = load_state
                        .libraries_loading_symbol(tag.tag)
                        .into_iter()
                        .map(str::to_string)
                        .collect();
                    Self::Tag {
                        name: tag.tag.to_string(),
                        loaded_libraries,
                        span: tag.name_span,
                    }
                }
                StructuralOccurrenceMeaning::CapturedIntermediate
                | StructuralOccurrenceMeaning::CapturedCloser => Self::None,
            };
        }

        let spec = tag_facts
            .for_tag(tag)
            .and_then(|facts| facts.spec.as_deref());

        if spec.and_then(TagSpec::role) == Some(TagRole::TemplateLibraryLoader) {
            let Some(load_kind) = LoadKind::from_loader_bits(tag.bits) else {
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
                            library: library.as_str().to_string(),
                            span: symbol.span(),
                        })
                }
            }
        } else if spec.and_then(TagSpec::role) == Some(TagRole::TemplateBlock) {
            tag.bits
                .first()
                .filter(|name| name.span.contains(offset))
                .map_or(Self::None, |name| Self::TemplateBlock {
                    name: name.as_str().to_string(),
                    span: name.span,
                })
        } else {
            spec.and_then(|spec| LiteralTemplateReference::from_spec(spec, tag.bits))
                .filter(|reference| reference.bit_span.contains(offset))
                .map_or(Self::None, |reference| Self::TemplateReference {
                    name: TemplateName::new(db, reference.template_name.to_string()),
                    kind: reference.kind,
                    span: reference.span,
                })
        }
    }
}
