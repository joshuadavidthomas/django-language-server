use djls_source::FileReadError;

use super::BranchConstraints;
use super::EvaluationState;
use super::Evaluator;
use super::Origin;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleValues;
use super::PythonNamespaceCause;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::ast;
use crate::python::PythonImportError;
use crate::python::evaluation::PythonImportEdge;
use crate::python::evaluation::PythonImportEvaluationStatus;
use crate::python::evaluation::PythonModuleEvaluation;
use crate::python::module::PythonImportRequest;
use crate::python::module::PythonImportResolutionError;
use crate::python::module::PythonModule;

impl Evaluator<'_> {
    pub(super) fn evaluate_import_from(&mut self, statement: &ast::StmtImportFrom) {
        let import = FromImport::lower(self, statement);
        let result = self
            .resolve_import(&import)
            .and_then(|module| self.evaluate_imported_module(module));
        self.state.apply_import(&import, result);
    }

    fn resolve_import(&self, import: &FromImport<'_>) -> Result<PythonModule, ImportFailure> {
        let request = PythonImportRequest {
            level: import.level,
            module: import.module,
            importer: &import.importer,
        };
        let module = match PythonModule::resolve_import(self.db, self.project, request) {
            Ok(module) => module,
            Err(PythonImportResolutionError::Invalid(error)) => {
                return Err(ImportFailure::Invalid(error));
            }
            Err(PythonImportResolutionError::NotFound(module)) => {
                return Err(ImportFailure::NotFound(module));
            }
        };
        if !module.search_path().is_project_code() {
            return Err(ImportFailure::SkippedExternal(module.name().clone()));
        }
        Ok(module)
    }

    fn evaluate_imported_module(
        &self,
        module: PythonModule,
    ) -> Result<LoadedImport, ImportFailure> {
        match super::super::query::evaluate_python_module(self.db, self.project, module.clone()) {
            PythonModuleEvaluation::CycleSeed => Err(ImportFailure::Cycle { module }),
            PythonModuleEvaluation::Evaluated {
                values,
                dependencies,
            } => match values {
                Ok(values) => Ok(LoadedImport {
                    module,
                    values,
                    dependencies,
                }),
                Err(error) => Err(ImportFailure::Unreadable(Box::new(UnreadableImport {
                    module,
                    error,
                    dependencies,
                }))),
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

struct LoadedImport {
    module: PythonModule,
    values: PythonModuleValues,
    dependencies: PythonModuleDependencies,
}

struct UnreadableImport {
    module: PythonModule,
    error: FileReadError,
    dependencies: PythonModuleDependencies,
}

enum ImportFailure {
    Invalid(PythonImportError),
    NotFound(crate::python::PythonModuleName),
    SkippedExternal(crate::python::PythonModuleName),
    Cycle { module: PythonModule },
    Unreadable(Box<UnreadableImport>),
}

impl EvaluationState {
    fn apply_import(
        &mut self,
        import: &FromImport<'_>,
        result: Result<LoadedImport, ImportFailure>,
    ) {
        match result {
            Err(failure) => self.apply_import_failure(import, failure),
            Ok(LoadedImport {
                module,
                values,
                dependencies,
            }) => {
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
            ImportFailure::Unreadable(unreadable) => {
                self.dependencies.files.insert(unreadable.module.file());
                self.absorb_dependencies(&unreadable.dependencies);
                (
                    PythonImportOutcome::Unreadable {
                        edge: PythonImportEdge {
                            origin: import.origin,
                            importer: import.importer.clone(),
                            imported: unreadable.module,
                        },
                        error: unreadable.error.clone(),
                    },
                    PythonUnknownCause::Unreadable(unreadable.error),
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
