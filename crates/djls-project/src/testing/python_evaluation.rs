use camino::Utf8PathBuf;
use djls_source::File;

use crate::db::Db;
use crate::project::Project;
use crate::python::InvalidModuleName;
use crate::python::PythonImportError;
use crate::python::PythonModuleName;
use crate::python::PythonSyntaxError;
use crate::python::evaluation;

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
    List(Vec<PythonListItemView>),
    Dict(Vec<PythonDictItemView>),
    Unknown(PythonUnknownView),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonListItemView {
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
    pub origin: Option<djls_source::Origin>,
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
    EmptyRelativeImport,
    ImporterOutsideSearchPaths(String),
    ImporterIsNotPythonSource(String),
    TooManyDots,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonImportOutcomeView {
    Resolved {
        origin: djls_source::Origin,
        file: File,
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
        error: PythonFileReadErrorView,
    },
    SyntaxErrors {
        origin: djls_source::Origin,
        file: File,
        errors: Vec<PythonSyntaxError>,
    },
    Cycle {
        origin: djls_source::Origin,
        file: File,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonMutationView {
    pub root: String,
    pub access: Vec<PythonMutationAccessView>,
    pub method: String,
    pub origin: djls_source::Origin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PythonMutationAccessView {
    Index(usize),
    Key(String),
}

pub fn python_module_evaluation(
    db: &dyn Db,
    project: Project,
    file: djls_source::File,
) -> PythonModuleEvaluationView {
    let values = evaluation::python_module_values(db, project, file).clone();
    let dependencies = evaluation::python_module_dependencies(db, project, file).clone();
    let (bindings, namespace_unknowns, syntax_errors, mutations, read_error) = match values {
        evaluation::PythonModuleValuesOutcome::Readable(values) => (
            values
                .bindings
                .0
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
                    root: mutation.root,
                    access: mutation
                        .access
                        .into_iter()
                        .map(mutation_access_view)
                        .collect(),
                    method: mutation.method,
                    origin: mutation.origin,
                })
                .collect(),
            None,
        ),
        evaluation::PythonModuleValuesOutcome::Unreadable(error) => (
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
        dependency_files: dependencies.files,
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
            evaluation::PythonBindingAlternative::Bound(bound) => {
                PythonBindingAlternativeView::Bound(PythonBoundValueView {
                    value: value_view(bound.value),
                    binding_origins: bound.binding_origins,
                })
            }
            evaluation::PythonBindingAlternative::Unbound => PythonBindingAlternativeView::Unbound,
        })
        .collect()
}

fn value_view(value: evaluation::PythonValue) -> PythonValueView {
    let origins = value.origins().collect();
    PythonValueView {
        kind: match value.kind {
            evaluation::PythonValueKind::Str(value) => PythonValueKindView::Str(value),
            evaluation::PythonValueKind::Bool(value) => PythonValueKindView::Bool(value),
            evaluation::PythonValueKind::Path(value) => PythonValueKindView::Path(value),
            evaluation::PythonValueKind::List(list) => PythonValueKindView::List(
                list.items
                    .into_iter()
                    .map(|item| match item {
                        evaluation::PythonListItem::Value(value) => {
                            PythonListItemView::Value(value_view(value))
                        }
                        evaluation::PythonListItem::UnknownElement(unknown) => {
                            PythonListItemView::UnknownElement(unknown_view(unknown))
                        }
                        evaluation::PythonListItem::UnknownUnpack(unknown) => {
                            PythonListItemView::UnknownUnpack(unknown_view(unknown))
                        }
                    })
                    .collect(),
            ),
            evaluation::PythonValueKind::Dict(dict) => PythonValueKindView::Dict(
                dict.items
                    .into_iter()
                    .map(|item| match item {
                        evaluation::PythonDictItem::Entry { key, value } => {
                            PythonDictItemView::Entry {
                                key: value_view(key),
                                value: value_view(value),
                            }
                        }
                        evaluation::PythonDictItem::UnknownUnpack(unknown) => {
                            PythonDictItemView::UnknownUnpack(unknown_view(unknown))
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
        origin: unknown.origin,
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
        PythonImportError::EmptyRelativeImport => PythonImportErrorView::EmptyRelativeImport,
        PythonImportError::ImporterOutsideSearchPaths(path) => {
            PythonImportErrorView::ImporterOutsideSearchPaths(path)
        }
        PythonImportError::ImporterIsNotPythonSource(path) => {
            PythonImportErrorView::ImporterIsNotPythonSource(path)
        }
        PythonImportError::TooManyDots => PythonImportErrorView::TooManyDots,
    }
}

fn import_outcome_view(outcome: evaluation::PythonImportOutcome) -> PythonImportOutcomeView {
    match outcome {
        evaluation::PythonImportOutcome::Resolved { origin, file } => {
            PythonImportOutcomeView::Resolved { origin, file }
        }
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
        evaluation::PythonImportOutcome::Unreadable {
            origin,
            file,
            error,
        } => PythonImportOutcomeView::Unreadable {
            origin,
            file,
            error: file_read_error_view(&error),
        },
        evaluation::PythonImportOutcome::SyntaxErrors {
            origin,
            file,
            errors,
        } => PythonImportOutcomeView::SyntaxErrors {
            origin,
            file,
            errors,
        },
        evaluation::PythonImportOutcome::Cycle { origin, file } => {
            PythonImportOutcomeView::Cycle { origin, file }
        }
    }
}

fn mutation_access_view(access: evaluation::PythonMutationAccess) -> PythonMutationAccessView {
    match access {
        evaluation::PythonMutationAccess::Index(index) => PythonMutationAccessView::Index(index),
        evaluation::PythonMutationAccess::Key(key) => PythonMutationAccessView::Key(key),
    }
}
