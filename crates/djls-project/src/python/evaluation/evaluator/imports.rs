use djls_source::FileReadError;

use super::BranchConstraints;
use super::EvaluationState;
use super::Evaluator;
use super::Origin;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleValues;
use super::PythonModuleValuesOutcome;
use super::PythonNamespaceCause;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::ast;
use crate::python::PythonImportError;
use crate::python::evaluation::PythonImportEdge;
use crate::python::evaluation::PythonImportEvaluationStatus;
use crate::python::module::PythonImportRequest;
use crate::python::module::PythonImportResolution;
use crate::python::module::PythonModule;

impl Evaluator<'_> {
    pub(super) fn evaluate_import_from(&mut self, statement: &ast::StmtImportFrom) {
        let import = FromImport::lower(self, statement);
        let result = match self.resolve_import(&import) {
            Ok(module) => self.evaluate_imported_module(module),
            Err(failure) => FromImportResult::Failed(failure.into()),
        };
        self.state.apply_import(&import, result);
    }

    fn resolve_import(
        &self,
        import: &FromImport<'_>,
    ) -> Result<PythonModule, ImportResolutionFailure> {
        let request = PythonImportRequest {
            level: import.level,
            module: import.module,
            importer: &import.importer,
        };
        let module = match PythonModule::resolve_import(self.db, self.project, request) {
            Err(error) => return Err(ImportResolutionFailure::Invalid(error)),
            Ok(PythonImportResolution::Missing(module)) => {
                return Err(ImportResolutionFailure::NotFound(module));
            }
            Ok(PythonImportResolution::Found(module)) => module,
        };
        if matches!(import.selection, ImportSelection::Named(_))
            && !module.search_path().is_first_party()
        {
            return Err(ImportResolutionFailure::SkippedExternal(
                module.name().clone(),
            ));
        }
        Ok(module)
    }

    fn evaluate_imported_module(&self, module: PythonModule) -> FromImportResult {
        let evaluation =
            super::super::query::evaluate_python_module(self.db, self.project, module.clone());
        if evaluation.is_cycle_seed() {
            return FromImportResult::Failed(ImportFailure::Cycle { module });
        }

        let dependencies = evaluation.dependencies;
        match evaluation.values {
            PythonModuleValuesOutcome::Unreadable(error) => {
                FromImportResult::Failed(ImportFailure::Unreadable {
                    module,
                    error,
                    dependencies,
                })
            }
            PythonModuleValuesOutcome::Readable(values) => FromImportResult::Loaded {
                module,
                values,
                dependencies,
            },
        }
    }
}

struct FromImport<'ast> {
    origin: Origin,
    importer: PythonModule,
    level: u32,
    module: Option<&'ast str>,
    selection: ImportSelection<'ast>,
}

impl<'ast> FromImport<'ast> {
    fn lower(evaluator: &Evaluator<'_>, statement: &'ast ast::StmtImportFrom) -> Self {
        let selection = if statement
            .names
            .iter()
            .any(|alias| alias.name.as_str() == "*")
        {
            ImportSelection::Star
        } else {
            ImportSelection::Named(
                statement
                    .names
                    .iter()
                    .map(|alias| {
                        let imported = alias.name.as_str();
                        ImportedBinding {
                            imported,
                            bound: alias
                                .asname
                                .as_ref()
                                .map_or(imported, ast::Identifier::as_str),
                            origin: evaluator.origin(alias),
                        }
                    })
                    .collect(),
            )
        };
        Self {
            origin: evaluator.origin(statement),
            importer: evaluator.module.clone(),
            level: statement.level,
            module: statement.module.as_ref().map(ast::Identifier::as_str),
            selection,
        }
    }
}

enum ImportSelection<'ast> {
    Star,
    Named(Vec<ImportedBinding<'ast>>),
}

struct ImportedBinding<'ast> {
    imported: &'ast str,
    bound: &'ast str,
    origin: Origin,
}

enum FromImportResult {
    Failed(ImportFailure),
    Loaded {
        module: PythonModule,
        values: PythonModuleValues,
        dependencies: PythonModuleDependencies,
    },
}

enum ImportResolutionFailure {
    Invalid(PythonImportError),
    NotFound(crate::python::PythonModuleName),
    SkippedExternal(crate::python::PythonModuleName),
}

impl From<ImportResolutionFailure> for ImportFailure {
    fn from(failure: ImportResolutionFailure) -> Self {
        match failure {
            ImportResolutionFailure::Invalid(error) => Self::Invalid(error),
            ImportResolutionFailure::NotFound(module) => Self::NotFound(module),
            ImportResolutionFailure::SkippedExternal(module) => Self::SkippedExternal(module),
        }
    }
}

enum ImportFailure {
    Invalid(PythonImportError),
    NotFound(crate::python::PythonModuleName),
    SkippedExternal(crate::python::PythonModuleName),
    Cycle {
        module: PythonModule,
    },
    Unreadable {
        module: PythonModule,
        error: FileReadError,
        dependencies: PythonModuleDependencies,
    },
}

impl EvaluationState {
    fn apply_import(&mut self, import: &FromImport<'_>, result: FromImportResult) {
        match result {
            FromImportResult::Failed(failure) => self.apply_import_failure(import, failure),
            FromImportResult::Loaded {
                module,
                values,
                dependencies,
            } => {
                self.dependencies.files.insert(module.file());
                self.absorb_dependencies(&dependencies);
                let edge = PythonImportEdge {
                    origin: import.origin,
                    importer: import.importer.clone(),
                    imported: module,
                };
                let status = if values.syntax_errors.is_empty() {
                    PythonImportEvaluationStatus::Resolved
                } else {
                    PythonImportEvaluationStatus::SyntaxErrors(values.syntax_errors.clone())
                };
                let outcome = PythonImportOutcome::Evaluated { edge, status };
                self.record_import(outcome);
                match &import.selection {
                    ImportSelection::Star => self.apply_star_import(&values, import.origin),
                    ImportSelection::Named(bindings) => {
                        for binding in bindings {
                            self.bind_named_import(
                                &values,
                                binding.imported,
                                binding.bound,
                                binding.origin,
                            );
                        }
                    }
                }
            }
        }
    }

    fn apply_import_failure(&mut self, import: &FromImport<'_>, failure: ImportFailure) {
        let (outcome, cause) = match failure {
            ImportFailure::Invalid(error) => (
                PythonImportOutcome::InvalidImport {
                    origin: import.origin,
                    reason: error.clone(),
                },
                PythonUnknownCause::InvalidImport(error),
            ),
            ImportFailure::NotFound(module) => (
                PythonImportOutcome::NotFound {
                    origin: import.origin,
                    module: module.clone(),
                },
                PythonUnknownCause::ImportNotFound(module),
            ),
            ImportFailure::SkippedExternal(module) => (
                PythonImportOutcome::SkippedExternal {
                    origin: import.origin,
                    module: module.clone(),
                },
                PythonUnknownCause::SkippedExternal(module),
            ),
            ImportFailure::Cycle { module } => {
                self.dependencies.files.insert(module.file());
                (
                    PythonImportOutcome::Evaluated {
                        edge: PythonImportEdge {
                            origin: import.origin,
                            importer: import.importer.clone(),
                            imported: module,
                        },
                        status: PythonImportEvaluationStatus::Cycle {
                            syntax_errors: Vec::new(),
                        },
                    },
                    PythonUnknownCause::Cycle,
                )
            }
            ImportFailure::Unreadable {
                module,
                error,
                dependencies,
            } => {
                self.dependencies.files.insert(module.file());
                self.absorb_dependencies(&dependencies);
                (
                    PythonImportOutcome::Unreadable {
                        edge: PythonImportEdge {
                            origin: import.origin,
                            importer: import.importer.clone(),
                            imported: module,
                        },
                        error: error.clone(),
                    },
                    PythonUnknownCause::Unreadable(error),
                )
            }
        };
        self.record_import(outcome);
        match &import.selection {
            ImportSelection::Star => {
                self.degrade_all_bindings(
                    &cause,
                    import.origin,
                    &BranchConstraints::unconstrained(),
                );
                self.namespace_causes
                    .push(PythonNamespaceCause::unconstrained(PythonUnknown {
                        cause,
                        origin: Some(import.origin),
                    }));
            }
            ImportSelection::Named(bindings) => {
                for binding in bindings {
                    self.bind_unknown(binding.bound, &cause, import.origin);
                }
            }
        }
    }
}
