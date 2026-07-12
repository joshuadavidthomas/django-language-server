mod evaluation;

pub(crate) fn testing_python_module_evaluation(
    db: &dyn crate::Db,
    project: crate::Project,
    file: djls_source::File,
) -> crate::testing::PythonModuleEvaluationView {
    let values = evaluation::python_module_values(db, project, file).clone();
    let dependencies = evaluation::python_module_dependencies(db, project, file).clone();
    let (bindings, namespace_unknowns, syntax_errors, mutations, read_error) = match values {
        evaluation::PythonModuleValuesOutcome::Readable(values) => (
            values
                .bindings
                .0
                .into_iter()
                .map(|(name, binding)| crate::testing::PythonBindingView {
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
                .map(|mutation| crate::testing::PythonMutationView {
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
    crate::testing::PythonModuleEvaluationView {
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
) -> Vec<crate::testing::PythonBindingAlternativeView> {
    binding
        .alternatives()
        .cloned()
        .map(|alternative| match alternative {
            evaluation::PythonBindingAlternative::Bound(bound) => {
                crate::testing::PythonBindingAlternativeView::Bound(
                    crate::testing::PythonBoundValueView {
                        value: value_view(bound.value),
                        binding_origins: bound.binding_origins,
                    },
                )
            }
            evaluation::PythonBindingAlternative::Unbound => {
                crate::testing::PythonBindingAlternativeView::Unbound
            }
        })
        .collect()
}

fn value_view(value: evaluation::PythonValue) -> crate::testing::PythonValueView {
    let origins = value.origins().collect();
    crate::testing::PythonValueView {
        kind: match value.kind {
            evaluation::PythonValueKind::Str(value) => {
                crate::testing::PythonValueKindView::Str(value)
            }
            evaluation::PythonValueKind::Bool(value) => {
                crate::testing::PythonValueKindView::Bool(value)
            }
            evaluation::PythonValueKind::Path(value) => {
                crate::testing::PythonValueKindView::Path(value)
            }
            evaluation::PythonValueKind::List(list) => crate::testing::PythonValueKindView::List(
                list.items
                    .into_iter()
                    .map(|item| match item {
                        evaluation::PythonListItem::Value(value) => {
                            crate::testing::PythonListItemView::Value(value_view(value))
                        }
                        evaluation::PythonListItem::UnknownElement(unknown) => {
                            crate::testing::PythonListItemView::UnknownElement(unknown_view(
                                unknown,
                            ))
                        }
                        evaluation::PythonListItem::UnknownUnpack(unknown) => {
                            crate::testing::PythonListItemView::UnknownUnpack(unknown_view(unknown))
                        }
                    })
                    .collect(),
            ),
            evaluation::PythonValueKind::Dict(dict) => crate::testing::PythonValueKindView::Dict(
                dict.items
                    .into_iter()
                    .map(|item| match item {
                        evaluation::PythonDictItem::Entry { key, value } => {
                            crate::testing::PythonDictItemView::Entry {
                                key: value_view(key),
                                value: value_view(value),
                            }
                        }
                        evaluation::PythonDictItem::UnknownUnpack(unknown) => {
                            crate::testing::PythonDictItemView::UnknownUnpack(unknown_view(unknown))
                        }
                    })
                    .collect(),
            ),
            evaluation::PythonValueKind::Unknown(unknown) => {
                crate::testing::PythonValueKindView::Unknown(unknown_view(unknown))
            }
        },
        origins,
    }
}

fn unknown_view(unknown: evaluation::PythonUnknown) -> crate::testing::PythonUnknownView {
    crate::testing::PythonUnknownView {
        cause: match unknown.cause {
            evaluation::PythonUnknownCause::UnsupportedExpression => {
                crate::testing::PythonUnknownCauseView::UnsupportedExpression
            }
            evaluation::PythonUnknownCause::UnsupportedMutation => {
                crate::testing::PythonUnknownCauseView::UnsupportedMutation
            }
            evaluation::PythonUnknownCause::InvalidImport(error) => {
                crate::testing::PythonUnknownCauseView::InvalidImport(import_error_view(error))
            }
            evaluation::PythonUnknownCause::ImportNotFound(module) => {
                crate::testing::PythonUnknownCauseView::ImportNotFound(module)
            }
            evaluation::PythonUnknownCause::SkippedExternal(module) => {
                crate::testing::PythonUnknownCauseView::SkippedExternal(module)
            }
            evaluation::PythonUnknownCause::Unreadable(error) => {
                crate::testing::PythonUnknownCauseView::Unreadable(file_read_error_view(&error))
            }
            evaluation::PythonUnknownCause::SyntaxErrors(errors) => {
                crate::testing::PythonUnknownCauseView::SyntaxErrors(errors)
            }
            evaluation::PythonUnknownCause::Cycle => crate::testing::PythonUnknownCauseView::Cycle,
            evaluation::PythonUnknownCause::AlternativeLimitExceeded => {
                crate::testing::PythonUnknownCauseView::AlternativeLimitExceeded
            }
        },
        origin: unknown.origin,
    }
}

fn file_read_error_view(
    error: &djls_source::FileReadError,
) -> crate::testing::PythonFileReadErrorView {
    crate::testing::PythonFileReadErrorView {
        path: error.path().to_path_buf(),
        kind: error.kind(),
    }
}

fn import_error_view(
    error: crate::python::module::PythonImportError,
) -> crate::testing::PythonImportErrorView {
    match error {
        crate::python::module::PythonImportError::InvalidModuleName(error) => {
            crate::testing::PythonImportErrorView::InvalidModuleName(error)
        }
        crate::python::module::PythonImportError::EmptyAbsoluteImport => {
            crate::testing::PythonImportErrorView::EmptyAbsoluteImport
        }
        crate::python::module::PythonImportError::EmptyRelativeImport => {
            crate::testing::PythonImportErrorView::EmptyRelativeImport
        }
        crate::python::module::PythonImportError::ImporterOutsideSearchPaths(path) => {
            crate::testing::PythonImportErrorView::ImporterOutsideSearchPaths(path)
        }
        crate::python::module::PythonImportError::ImporterIsNotPythonSource(path) => {
            crate::testing::PythonImportErrorView::ImporterIsNotPythonSource(path)
        }
        crate::python::module::PythonImportError::TooManyDots => {
            crate::testing::PythonImportErrorView::TooManyDots
        }
    }
}

fn import_outcome_view(
    outcome: evaluation::PythonImportOutcome,
) -> crate::testing::PythonImportOutcomeView {
    match outcome {
        evaluation::PythonImportOutcome::Resolved { origin, file } => {
            crate::testing::PythonImportOutcomeView::Resolved { origin, file }
        }
        evaluation::PythonImportOutcome::InvalidImport { origin, reason } => {
            crate::testing::PythonImportOutcomeView::InvalidImport {
                origin,
                reason: import_error_view(reason),
            }
        }
        evaluation::PythonImportOutcome::NotFound { origin, module } => {
            crate::testing::PythonImportOutcomeView::NotFound { origin, module }
        }
        evaluation::PythonImportOutcome::SkippedExternal { origin, module } => {
            crate::testing::PythonImportOutcomeView::SkippedExternal { origin, module }
        }
        evaluation::PythonImportOutcome::Unreadable {
            origin,
            file,
            error,
        } => crate::testing::PythonImportOutcomeView::Unreadable {
            origin,
            file,
            error: file_read_error_view(&error),
        },
        evaluation::PythonImportOutcome::SyntaxErrors {
            origin,
            file,
            errors,
        } => crate::testing::PythonImportOutcomeView::SyntaxErrors {
            origin,
            file,
            errors,
        },
        evaluation::PythonImportOutcome::Cycle { origin, file } => {
            crate::testing::PythonImportOutcomeView::Cycle { origin, file }
        }
    }
}

fn mutation_access_view(
    access: evaluation::PythonMutationAccess,
) -> crate::testing::PythonMutationAccessView {
    match access {
        evaluation::PythonMutationAccess::Index(index) => {
            crate::testing::PythonMutationAccessView::Index(index)
        }
        evaluation::PythonMutationAccess::Key(key) => {
            crate::testing::PythonMutationAccessView::Key(key)
        }
    }
}
pub(crate) use self::evaluation::BranchConstraints;
pub(crate) use self::evaluation::PythonBindingAlternative;
pub(crate) use self::evaluation::PythonBoundValue;
pub(crate) use self::evaluation::PythonDict;
pub(crate) use self::evaluation::PythonDictItem;
pub(crate) use self::evaluation::PythonList;
pub(crate) use self::evaluation::PythonListItem;
pub(crate) use self::evaluation::PythonModuleValues;
pub(crate) use self::evaluation::PythonModuleValuesOutcome;
pub(crate) use self::evaluation::PythonMutation;
pub(crate) use self::evaluation::PythonMutationAccess;
pub(crate) use self::evaluation::PythonUnknown;
pub(crate) use self::evaluation::PythonUnknownCause;
pub(crate) use self::evaluation::PythonValue;
pub(crate) use self::evaluation::PythonValueKind;
pub(crate) use self::evaluation::python_module_dependencies;
pub(crate) use self::evaluation::python_module_values;
