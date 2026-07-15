use super::BranchConstraints;
use super::EvaluationContext;
use super::EvaluationState;
use super::File;
use super::Origin;
use super::PythonImportOutcome;
use super::PythonImportRequest;
use super::PythonModule;
use super::PythonModuleValuesOutcome;
use super::PythonNamespaceCause;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::ast;
use super::extend_ordered_unique;

pub(super) fn walk_import_from(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    import: &ast::StmtImportFrom,
) {
    let origin = context.origin(import);
    let is_star = import.names.iter().any(|alias| alias.name.as_str() == "*");
    let request = PythonImportRequest {
        level: import.level,
        module: import.module.as_ref().map(ast::Identifier::as_str),
        importer: context.file.path(context.db),
    };
    let module = match PythonModule::resolve_import(context.db, context.project, request) {
        Err(reason) => {
            state.record_import(PythonImportOutcome::InvalidImport {
                origin,
                reason: reason.clone(),
            });
            apply_failed_import(
                state,
                import,
                is_star,
                origin,
                PythonUnknownCause::InvalidImport(reason),
            );
            return;
        }
        Ok(None) => {
            let Some(module) = import
                .module
                .as_ref()
                .and_then(|name| crate::python::PythonModuleName::parse(name.as_str()).ok())
            else {
                return;
            };
            state.record_import(PythonImportOutcome::NotFound {
                origin,
                module: module.clone(),
            });
            apply_failed_import(
                state,
                import,
                is_star,
                origin,
                PythonUnknownCause::ImportNotFound(module),
            );
            return;
        }
        Ok(Some(module)) => module,
    };
    if !is_star && !module.search_path().is_first_party() {
        state.record_import(PythonImportOutcome::SkippedExternal {
            origin,
            module: module.name().clone(),
        });
        apply_failed_import(
            state,
            import,
            false,
            origin,
            PythonUnknownCause::SkippedExternal(module.name().clone()),
        );
        return;
    }

    apply_resolved_import(context, state, import, is_star, origin, module.file());
}

fn apply_resolved_import(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    import: &ast::StmtImportFrom,
    is_star: bool,
    origin: Origin,
    imported_file: File,
) {
    extend_ordered_unique(&mut state.dependencies.files, &[imported_file]);
    let imported =
        super::super::query::evaluate_python_module(context.db, context.project, imported_file);
    if imported.is_cycle_seed() {
        state.record_import(PythonImportOutcome::Cycle {
            origin,
            file: imported_file,
        });
        apply_failed_import(state, import, is_star, origin, PythonUnknownCause::Cycle);
        return;
    }
    state.absorb_dependencies(&imported);
    match &imported.values {
        PythonModuleValuesOutcome::Unreadable(error) => {
            state.record_import(PythonImportOutcome::Unreadable {
                origin,
                file: imported_file,
                error: error.clone(),
            });
            apply_failed_import(
                state,
                import,
                is_star,
                origin,
                PythonUnknownCause::Unreadable(error.clone()),
            );
        }
        PythonModuleValuesOutcome::Readable(values) => {
            let outcome = if values.syntax_errors.is_empty() {
                PythonImportOutcome::Resolved {
                    origin,
                    file: imported_file,
                }
            } else {
                PythonImportOutcome::SyntaxErrors {
                    origin,
                    file: imported_file,
                    errors: values.syntax_errors.clone(),
                }
            };
            state.record_import(outcome);
            if is_star {
                state.apply_star_import(values, origin);
            } else {
                for alias in &import.names {
                    let imported_name = alias.name.as_str();
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or(imported_name, ast::Identifier::as_str);
                    state.bind_named_import(
                        values,
                        imported_name,
                        bound_name,
                        context.origin(alias),
                    );
                }
            }
        }
    }
}

fn apply_failed_import(
    state: &mut EvaluationState,
    import: &ast::StmtImportFrom,
    is_star: bool,
    origin: Origin,
    cause: PythonUnknownCause,
) {
    if is_star {
        state.degrade_all_bindings(&cause, origin, &BranchConstraints::unconstrained());
        state
            .namespace_causes
            .push(PythonNamespaceCause::unconstrained(PythonUnknown {
                cause,
                origin: Some(origin),
            }));
    } else {
        for alias in &import.names {
            let bound_name = alias
                .asname
                .as_ref()
                .map_or_else(|| alias.name.as_str(), ast::Identifier::as_str);
            state.bind_unknown(bound_name, &cause, origin);
        }
    }
}
