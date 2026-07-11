use djls_project::TemplateName;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::parse_template;

use crate::db::Db as SemanticDb;
use crate::references::LiteralTemplateReference;
use crate::references::TemplateReferenceKind;
use crate::scoping::LoadKind;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::ActiveTemplateVariable;
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

impl<'db> SemanticOffsetContext<'db> {
    pub fn from_offset(db: &'db dyn SemanticDb, file: File, offset: Offset) -> Self {
        let djls_templates::TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
            return Self::None;
        };

        let tree = crate::structure::build_template_tree_for_file(db, file, nodelist);
        let loaded = crate::scoping::compute_loaded_libraries_for_file(db, file, nodelist);

        for node in active_template_nodes(tree.regions(db), tree.root(db)) {
            let context = match node {
                ActiveTemplateNode::Tag(tag) if tag.full_span.contains(offset) => {
                    Self::from_tag(db, file, loaded, tag, offset)
                }
                ActiveTemplateNode::Variable(variable) if variable.span.contains(offset) => {
                    Self::from_variable(loaded, variable, offset)
                }
                ActiveTemplateNode::Tag(_) | ActiveTemplateNode::Variable(_) => Self::None,
            };

            if context != Self::None {
                return context;
            }
        }

        Self::None
    }

    fn from_variable(
        loaded: &crate::scoping::LoadedLibraries,
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
                    name: filter.name.clone(),
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
        file: File,
        loaded: &crate::scoping::LoadedLibraries,
        tag: ActiveTemplateTag<'_>,
        offset: Offset,
    ) -> Self {
        let load_state = loaded.available_at(tag.span.start());
        if tag.name_span.contains(offset) {
            let loaded_libraries = load_state
                .libraries_loading_symbol(tag.tag)
                .into_iter()
                .map(str::to_string)
                .collect();
            return Self::Tag {
                name: tag.tag.to_string(),
                loaded_libraries,
                span: tag.name_span,
            };
        }

        let spec = crate::tags::effective_tag_spec(db, file, tag.tag, &load_state);

        if spec.as_ref().and_then(crate::TagSpec::role)
            == Some(crate::tags::TagRole::TemplateLibraryLoader)
        {
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
                            library: library.as_str().to_string(),
                            span: symbol.span(),
                        })
                }
            }
        } else {
            spec.as_ref()
                .and_then(|spec| LiteralTemplateReference::from_spec(spec, tag.bits))
                .filter(|reference| reference.bit_span.contains(offset))
                .map_or(Self::None, |reference| Self::TemplateReference {
                    name: TemplateName::new(db, reference.template_name.to_string()),
                    kind: reference.kind,
                    span: reference.span,
                })
        }
    }
}
