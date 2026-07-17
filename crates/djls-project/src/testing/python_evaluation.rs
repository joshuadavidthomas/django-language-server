use camino::Utf8PathBuf;
use djls_source::File;

use crate::db::Db;
use crate::project::Project;
use crate::python::InvalidModuleName;
use crate::python::PythonImportError;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonSyntaxError;
use crate::python::evaluation;
use crate::python::file_to_module;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonModuleEvaluationView {
    bindings: Vec<PythonBindingView>,
    pub namespace_unknowns: Vec<PythonUnknownView>,
    syntax_errors: Vec<PythonSyntaxError>,
    pub mutations: Vec<PythonMutationView>,
    read_error: Option<PythonFileReadErrorView>,
    pub dependency_files: Vec<File>,
    pub imports: Vec<PythonImportOutcomeView>,
}

impl PythonModuleEvaluationView {
    #[must_use]
    pub fn binding(&self, name: &str) -> Option<&PythonBindingView> {
        self.bindings.iter().find(|binding| binding.name == name)
    }

    #[must_use]
    pub fn namespace_open(&self) -> bool {
        !self.namespace_unknowns.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonBindingView {
    name: String,
    pub alternatives: Vec<PythonBindingAlternativeView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonBindingAlternativeView {
    Bound(PythonBoundValueView),
    Unbound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonBoundValueView {
    pub value: PythonValueView,
    pub binding_origins: Vec<djls_source::Origin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonValueView {
    pub kind: PythonValueKindView,
    pub origins: Vec<djls_source::Origin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonValueKindView {
    Str(String),
    Bool(bool),
    Path(Utf8PathBuf),
    List(Vec<PythonSequenceItemView>),
    Tuple(Vec<PythonSequenceItemView>),
    Dict(Vec<PythonDictItemView>),
    Unknown(PythonUnknownView),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonSequenceItemView {
    Value(PythonValueView),
    UnknownElement(PythonUnknownView),
    UnknownUnpack(PythonUnknownView),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonDictItemView {
    Entry {
        key: PythonValueView,
        value: PythonValueView,
    },
    UnknownUnpack(PythonUnknownView),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonUnknownView {
    pub cause: PythonUnknownCauseView,
    pub origins: Vec<djls_source::Origin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonUnknownCauseView {
    UnsupportedExpression,
    UnsupportedMutation,
    InvalidImport(PythonImportErrorView),
    ImportNotFound(PythonModuleName),
    SkippedExternal(PythonModuleName),
    Unreadable(PythonFileReadErrorView),
    SyntaxErrors(Vec<PythonSyntaxError>),
    Cycle,
    AlternativeLimitExceeded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonFileReadErrorView {
    pub path: Utf8PathBuf,
    pub kind: std::io::ErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonImportErrorView {
    InvalidModuleName(InvalidModuleName),
    EmptyAbsoluteImport,
    TooManyDots,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonImportOutcomeView {
    Resolved {
        origin: djls_source::Origin,
        file: File,
        importer_module: PythonModuleName,
        imported_module: PythonModuleName,
    },
    InvalidImport {
        origin: djls_source::Origin,
        reason: PythonImportErrorView,
    },
    NotFound {
        origin: djls_source::Origin,
        module: PythonModuleName,
    },
    SkippedExternal {
        origin: djls_source::Origin,
        module: PythonModuleName,
    },
    Unreadable {
        origin: djls_source::Origin,
        file: File,
        importer_module: PythonModuleName,
        imported_module: PythonModuleName,
        error: PythonFileReadErrorView,
    },
    SyntaxErrors {
        origin: djls_source::Origin,
        file: File,
        importer_module: PythonModuleName,
        imported_module: PythonModuleName,
        errors: Vec<PythonSyntaxError>,
    },
    Cycle {
        origin: djls_source::Origin,
        file: File,
        importer_module: PythonModuleName,
        imported_module: PythonModuleName,
        syntax_errors: Vec<PythonSyntaxError>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonMutationView {
    pub binding: String,
    pub path: Vec<PythonMutationPathSegmentView>,
    pub operation: PythonMutationOperationView,
    pub origin: djls_source::Origin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonMutationPathSegmentView {
    Index(usize),
    Key(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonMutationOperationView {
    Append,
    Extend,
    Insert,
    Remove,
}

pub fn python_module_evaluation(
    db: &dyn Db,
    project: Project,
    file: djls_source::File,
) -> PythonModuleEvaluationView {
    let module = file_to_module(db, project, file.path(db).to_path_buf())
        .expect("the evaluated file should have a canonical module identity");
    python_module_evaluation_for_module(db, project, module)
}

pub fn python_module_evaluation_for_module(
    db: &dyn Db,
    project: Project,
    module: PythonModule,
) -> PythonModuleEvaluationView {
    let values = evaluation::python_module_values(db, project, module.clone()).clone();
    let dependencies = evaluation::python_module_dependencies(db, project, module).clone();
    let (bindings, namespace_unknowns, syntax_errors, mutations, read_error) = match values {
        Ok(values) => (
            values
                .bindings
                .into_iter()
                .map(|(name, binding)| PythonBindingView {
                    name,
                    alternatives: binding_alternatives_view(&binding),
                })
                .collect(),
            values
                .namespace_remainder
                .map_or_else(Vec::new, |remainder| {
                    remainder
                        .causes
                        .into_iter()
                        .map(|cause| unknown_view(cause.unknown))
                        .collect()
                }),
            values.syntax_errors,
            values
                .mutations
                .into_iter()
                .map(|mutation| PythonMutationView {
                    binding: mutation.binding,
                    path: mutation
                        .path
                        .iter()
                        .cloned()
                        .map(mutation_path_segment_view)
                        .collect(),
                    operation: mutation_operation_view(mutation.operation),
                    origin: mutation.origin,
                })
                .collect(),
            None,
        ),
        Err(error) => (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(file_read_error_view(&error)),
        ),
    };
    PythonModuleEvaluationView {
        bindings,
        namespace_unknowns,
        syntax_errors,
        mutations,
        read_error,
        dependency_files: dependencies.files.into_iter().collect(),
        imports: dependencies
            .imports
            .into_iter()
            .map(import_outcome_view)
            .collect(),
    }
}

fn binding_alternatives_view(
    binding: &evaluation::PythonBinding,
) -> Vec<PythonBindingAlternativeView> {
    binding
        .alternatives()
        .cloned()
        .map(|alternative| match alternative {
            evaluation::PythonBindingState::Bound(bound) => {
                let binding_origins = bound.binding_origins().collect();
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: value_view(bound.value),
                    binding_origins,
                })
            }
            evaluation::PythonBindingState::Unbound => PythonBindingAlternativeView::Unbound,
        })
        .collect()
}

fn sequence_items_view(items: &[evaluation::PythonSequenceItem]) -> Vec<PythonSequenceItemView> {
    items
        .iter()
        .cloned()
        .map(|item| match item {
            evaluation::PythonSequenceItem::Value(value) => {
                PythonSequenceItemView::Value(value_view(value))
            }
            evaluation::PythonSequenceItem::UnknownElement(unknown) => {
                PythonSequenceItemView::UnknownElement(unknown_view(unknown))
            }
            evaluation::PythonSequenceItem::UnknownUnpack(unknown) => {
                PythonSequenceItemView::UnknownUnpack(unknown_view(unknown))
            }
        })
        .collect()
}

fn value_view(value: evaluation::PythonValue) -> PythonValueView {
    let origins = value.origins().collect();
    PythonValueView {
        kind: match value.into_kind() {
            evaluation::PythonValueKind::Str(value) => PythonValueKindView::Str(value),
            evaluation::PythonValueKind::Bool(value) => PythonValueKindView::Bool(value),
            evaluation::PythonValueKind::Path(value) => PythonValueKindView::Path(value),
            evaluation::PythonValueKind::List(list) => {
                PythonValueKindView::List(sequence_items_view(list.semantic_items()))
            }
            evaluation::PythonValueKind::Tuple(tuple) => {
                PythonValueKindView::Tuple(sequence_items_view(tuple.semantic_items()))
            }
            evaluation::PythonValueKind::Dict(dict) => PythonValueKindView::Dict(
                dict.mapping()
                    .projection()
                    .map(|item| match item {
                        evaluation::MappingProjection::Entry { key, value } => {
                            PythonDictItemView::Entry {
                                key: value_view(key.clone()),
                                value: value_view(value.clone()),
                            }
                        }
                        evaluation::MappingProjection::UnknownUnpack(unknown) => {
                            PythonDictItemView::UnknownUnpack(unknown_view(unknown.clone()))
                        }
                    })
                    .collect(),
            ),
            evaluation::PythonValueKind::Unknown(unknown) => {
                PythonValueKindView::Unknown(unknown_view(unknown))
            }
        },
        origins,
    }
}

fn unknown_view(unknown: evaluation::PythonUnknown) -> PythonUnknownView {
    let origins = unknown.origins().collect();
    PythonUnknownView {
        cause: match unknown.cause {
            evaluation::PythonUnknownCause::UnsupportedExpression => {
                PythonUnknownCauseView::UnsupportedExpression
            }
            evaluation::PythonUnknownCause::UnsupportedMutation => {
                PythonUnknownCauseView::UnsupportedMutation
            }
            evaluation::PythonUnknownCause::InvalidImport(error) => {
                PythonUnknownCauseView::InvalidImport(import_error_view(error))
            }
            evaluation::PythonUnknownCause::ImportNotFound(module) => {
                PythonUnknownCauseView::ImportNotFound(module)
            }
            evaluation::PythonUnknownCause::SkippedExternal(module) => {
                PythonUnknownCauseView::SkippedExternal(module)
            }
            evaluation::PythonUnknownCause::Unreadable(error) => {
                PythonUnknownCauseView::Unreadable(file_read_error_view(&error))
            }
            evaluation::PythonUnknownCause::SyntaxErrors(errors) => {
                PythonUnknownCauseView::SyntaxErrors(errors)
            }
            evaluation::PythonUnknownCause::Cycle => PythonUnknownCauseView::Cycle,
            evaluation::PythonUnknownCause::AlternativeLimitExceeded => {
                PythonUnknownCauseView::AlternativeLimitExceeded
            }
        },
        origins,
    }
}

fn file_read_error_view(error: &djls_source::FileReadError) -> PythonFileReadErrorView {
    PythonFileReadErrorView {
        path: error.path().to_path_buf(),
        kind: error.kind(),
    }
}

fn import_error_view(error: PythonImportError) -> PythonImportErrorView {
    match error {
        PythonImportError::InvalidModuleName(error) => {
            PythonImportErrorView::InvalidModuleName(error)
        }
        PythonImportError::EmptyAbsoluteImport => PythonImportErrorView::EmptyAbsoluteImport,
        PythonImportError::TooManyDots => PythonImportErrorView::TooManyDots,
    }
}

fn import_outcome_view(outcome: evaluation::PythonImportOutcome) -> PythonImportOutcomeView {
    match outcome {
        evaluation::PythonImportOutcome::Evaluated { edge, status } => match status {
            evaluation::PythonImportEvaluationStatus::Resolved => {
                PythonImportOutcomeView::Resolved {
                    origin: edge.origin,
                    file: edge.imported.file(),
                    importer_module: edge.importer.name().clone(),
                    imported_module: edge.imported.name().clone(),
                }
            }
            evaluation::PythonImportEvaluationStatus::SyntaxErrors(errors) => {
                PythonImportOutcomeView::SyntaxErrors {
                    origin: edge.origin,
                    file: edge.imported.file(),
                    importer_module: edge.importer.name().clone(),
                    imported_module: edge.imported.name().clone(),
                    errors,
                }
            }
            evaluation::PythonImportEvaluationStatus::Cycle { syntax_errors } => {
                PythonImportOutcomeView::Cycle {
                    origin: edge.origin,
                    file: edge.imported.file(),
                    importer_module: edge.importer.name().clone(),
                    imported_module: edge.imported.name().clone(),
                    syntax_errors,
                }
            }
        },
        evaluation::PythonImportOutcome::InvalidImport { origin, reason } => {
            PythonImportOutcomeView::InvalidImport {
                origin,
                reason: import_error_view(reason),
            }
        }
        evaluation::PythonImportOutcome::NotFound { origin, module } => {
            PythonImportOutcomeView::NotFound { origin, module }
        }
        evaluation::PythonImportOutcome::SkippedExternal { origin, module } => {
            PythonImportOutcomeView::SkippedExternal { origin, module }
        }
        evaluation::PythonImportOutcome::Unreadable { edge, error } => {
            PythonImportOutcomeView::Unreadable {
                origin: edge.origin,
                file: edge.imported.file(),
                importer_module: edge.importer.name().clone(),
                imported_module: edge.imported.name().clone(),
                error: file_read_error_view(&error),
            }
        }
    }
}

fn mutation_path_segment_view(
    segment: evaluation::PythonMutationPathSegment,
) -> PythonMutationPathSegmentView {
    match segment {
        evaluation::PythonMutationPathSegment::Index(index) => {
            PythonMutationPathSegmentView::Index(index)
        }
        evaluation::PythonMutationPathSegment::Key(key) => PythonMutationPathSegmentView::Key(key),
    }
}

fn mutation_operation_view(
    operation: evaluation::PythonMutationOperation,
) -> PythonMutationOperationView {
    match operation {
        evaluation::PythonMutationOperation::Append => PythonMutationOperationView::Append,
        evaluation::PythonMutationOperation::Extend => PythonMutationOperationView::Extend,
        evaluation::PythonMutationOperation::Insert => PythonMutationOperationView::Insert,
        evaluation::PythonMutationOperation::Remove => PythonMutationOperationView::Remove,
    }
}
