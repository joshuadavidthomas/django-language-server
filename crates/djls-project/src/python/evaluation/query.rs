use std::collections::BTreeSet;

use djls_source::FileReadError;
use rustc_hash::FxHashSet;

use super::PythonBinding;
use super::PythonImportEdge;
use super::PythonImportEvaluationStatus;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleEvaluation;
use super::PythonModuleValues;
use super::PythonNamespaceCause;
use super::PythonNamespaceRemainder;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::UniqueVec;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::recovered_python_module;

// Salsa tracked-query keys are by-value; `module` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(
    cycle_initial=evaluate_python_module_cycle_initial,
    cycle_fn=evaluate_python_module_cycle_recover,
)]
pub(crate) fn evaluate_python_module(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> PythonModuleEvaluation {
    let file = module.file();
    let parsed = match recovered_python_module(db, file) {
        Ok(Some(parsed)) => parsed,
        Err(error) => {
            return PythonModuleEvaluation::evaluated(
                Err(error),
                PythonModuleDependencies::rooted(file),
            );
        }
        Ok(None) => {
            return PythonModuleEvaluation::evaluated(
                Ok(PythonModuleValues::default()),
                PythonModuleDependencies::rooted(file),
            );
        }
    };
    let body = parsed.body(db);
    let mut evaluation = super::evaluator::evaluate_body(db, project, module.clone(), body);
    if let Ok(values) = &mut evaluation.values {
        values.syntax_errors = parsed.syntax_errors(db).to_vec();
        values.syntax_impacts =
            super::touched_names::collect_syntax_impacts(body, &values.syntax_errors);
    }
    normalize_cycle_evaluation(&mut evaluation, &module);
    evaluation
}

// This projection gives value consumers an independent red-green cutoff when only dependencies
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_values(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> Result<PythonModuleValues, FileReadError> {
    evaluate_python_module(db, project, module).values.clone()
}

// This projection gives dependency consumers an independent red-green cutoff when only values
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_dependencies(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> PythonModuleDependencies {
    evaluate_python_module(db, project, module)
        .dependencies
        .clone()
}

fn evaluate_python_module_cycle_initial(
    _db: &dyn ProjectDb,
    _id: salsa::Id,
    _project: Project,
    _module: PythonModule,
) -> PythonModuleEvaluation {
    PythonModuleEvaluation::cycle_seed()
}

// Salsa requires cycle recovery callbacks to accept the tracked-query keys by value.
#[allow(clippy::needless_pass_by_value)]
fn evaluate_python_module_cycle_recover(
    _db: &dyn ProjectDb,
    cycle: &salsa::Cycle,
    previous: &PythonModuleEvaluation,
    computed: PythonModuleEvaluation,
    _project: Project,
    module: PythonModule,
) -> PythonModuleEvaluation {
    assert!(
        cycle.iteration() < 12,
        "Python module cycle should converge within twelve iterations"
    );
    let mut recovered = if previous.is_cycle_seed() || previous == &computed {
        computed
    } else {
        widen_cycle_evaluation(previous, computed, &module)
    };
    normalize_cycle_evaluation(&mut recovered, &module);
    recovered
}

fn normalize_cycle_evaluation(evaluation: &mut PythonModuleEvaluation, root: &PythonModule) {
    let root_is_in_cycle =
        root_participates_in_import_cycle(evaluation.dependencies.imports.as_slice(), root);
    let root_file = root.file();
    if let Ok(values) = &mut evaluation.values {
        values
            .mutations
            .sort_by_key(|mutation| format!("{mutation:?}"));
        if root_is_in_cycle && let Some(remainder) = &mut values.namespace_remainder {
            for cause in &mut remainder.causes {
                if cause.unknown.cause == PythonUnknownCause::Cycle {
                    cause.unknown.origin = None;
                }
            }
            *remainder = PythonNamespaceRemainder::new(remainder.causes.clone());
        }
    }
    normalize_cycle_edges(&mut evaluation.dependencies.imports);
    evaluation
        .dependencies
        .files
        .sort_by_key(|file| (usize::from(*file != root_file), format!("{file:?}")));
}

fn widen_cycle_evaluation(
    previous: &PythonModuleEvaluation,
    mut computed: PythonModuleEvaluation,
    root: &PythonModule,
) -> PythonModuleEvaluation {
    match (&previous.values, &mut computed.values) {
        (Ok(previous_values), Ok(computed_values)) => {
            let names = previous_values
                .bindings
                .keys()
                .chain(computed_values.bindings.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for name in names {
                if previous_values.bindings.get(&name) != computed_values.bindings.get(&name) {
                    computed_values
                        .bindings
                        .insert(name, PythonBinding::originless_cycle_unknown());
                }
            }
            if previous_values.namespace_remainder != computed_values.namespace_remainder {
                computed_values.namespace_remainder = Some(PythonNamespaceRemainder::new(vec![
                    PythonNamespaceCause::unconstrained(PythonUnknown {
                        cause: PythonUnknownCause::Cycle,
                        origin: None,
                    }),
                ]));
            }
            computed_values
                .mutations
                .extend(previous_values.mutations.iter().cloned());
            computed_values
                .mutations
                .sort_by_key(|mutation| format!("{mutation:?}"));
        }
        (Ok(_) | Err(_), Err(_)) | (Err(_), Ok(_)) => {}
    }

    if previous.dependencies != computed.dependencies {
        computed.dependencies =
            widen_cycle_dependencies(&previous.dependencies, &computed.dependencies, root);
    }
    computed
}

fn widen_cycle_dependencies(
    previous: &PythonModuleDependencies,
    computed: &PythonModuleDependencies,
    root: &PythonModule,
) -> PythonModuleDependencies {
    let mut candidates = previous.imports.clone();
    candidates.extend(computed.imports.iter().cloned());
    normalize_cycle_edges(&mut candidates);
    let cycle = candidates.iter().find_map(|outcome| match outcome {
        PythonImportOutcome::Evaluated {
            edge,
            status: PythonImportEvaluationStatus::Cycle { .. },
        } => Some(edge),
        PythonImportOutcome::Evaluated { .. }
        | PythonImportOutcome::InvalidImport { .. }
        | PythonImportOutcome::NotFound { .. }
        | PythonImportOutcome::SkippedExternal { .. }
        | PythonImportOutcome::Unreadable { .. } => None,
    });

    let root_file = root.file();
    let mut files = [root_file].into_iter().collect::<UniqueVec<_>>();
    files.extend(previous.files.iter().copied());
    files.extend(computed.files.iter().copied());
    if let Some(edge) = cycle {
        files.extend([edge.importer.file(), edge.imported.file()]);
    }
    files.sort_by_key(|file| (usize::from(*file != root_file), format!("{file:?}")));
    candidates.sort_by_key(|outcome| format!("{outcome:?}"));
    PythonModuleDependencies {
        files,
        imports: candidates,
    }
}

fn root_participates_in_import_cycle(imports: &[PythonImportOutcome], root: &PythonModule) -> bool {
    let edges = imports.iter().filter_map(import_edge).collect::<Vec<_>>();
    edges
        .iter()
        .any(|edge| edge.importer == *root && path_exists(&edges, &edge.imported, root))
}

fn normalize_cycle_edges(imports: &mut UniqueVec<PythonImportOutcome>) {
    let edges = imports.iter().filter_map(import_edge).collect::<Vec<_>>();
    let has_cycle = imports.iter().any(|outcome| {
        matches!(
            outcome,
            PythonImportOutcome::Evaluated {
                status: PythonImportEvaluationStatus::Cycle { .. },
                ..
            }
        )
    });
    let canonical = if has_cycle {
        canonical_cycle_edges(imports, &edges)
    } else {
        Vec::new()
    };

    let mut normalized = Vec::new();
    for outcome in std::mem::take(imports) {
        match outcome {
            PythonImportOutcome::Evaluated { edge, status } => {
                let is_cycle = canonical.contains(&edge);
                if let Some(PythonImportOutcome::Evaluated {
                    status: existing,
                    ..
                }) = normalized.iter_mut().find(|outcome| {
                    matches!(outcome, PythonImportOutcome::Evaluated { edge: candidate, .. } if candidate == &edge)
                }) {
                    merge_import_status(existing, status, is_cycle);
                } else {
                    normalized.push(PythonImportOutcome::Evaluated {
                        edge,
                        status: normalize_import_status(status, is_cycle),
                    });
                }
            }
            outcome @ (PythonImportOutcome::InvalidImport { .. }
            | PythonImportOutcome::NotFound { .. }
            | PythonImportOutcome::SkippedExternal { .. }
            | PythonImportOutcome::Unreadable { .. }) => {
                if !normalized.contains(&outcome) {
                    normalized.push(outcome);
                }
            }
        }
    }
    *imports = normalized.into();
    imports.sort_by_key(|outcome| format!("{outcome:?}"));
}

fn canonical_cycle_edges(
    imports: &UniqueVec<PythonImportOutcome>,
    edges: &[&PythonImportEdge],
) -> Vec<PythonImportEdge> {
    let mut cyclic = imports
        .iter()
        .filter_map(|outcome| match outcome {
            PythonImportOutcome::Evaluated { edge, .. } => Some(edge),
            PythonImportOutcome::InvalidImport { .. }
            | PythonImportOutcome::NotFound { .. }
            | PythonImportOutcome::SkippedExternal { .. }
            | PythonImportOutcome::Unreadable { .. } => None,
        })
        .filter(|edge| path_exists(edges, &edge.imported, &edge.importer))
        .collect::<Vec<_>>();
    cyclic.sort_by_key(|edge| import_edge_sort_key(edge));

    let mut canonical = Vec::new();
    for edge in cyclic {
        if !canonical.iter().any(|existing: &PythonImportEdge| {
            path_exists(edges, &existing.importer, &edge.importer)
                && path_exists(edges, &edge.importer, &existing.importer)
        }) {
            canonical.push(edge.clone());
        }
    }

    if canonical.is_empty() {
        canonical.extend(imports.iter().filter_map(|outcome| match outcome {
            PythonImportOutcome::Evaluated {
                edge,
                status: PythonImportEvaluationStatus::Cycle { .. },
            } => Some(edge.clone()),
            PythonImportOutcome::Evaluated { .. }
            | PythonImportOutcome::InvalidImport { .. }
            | PythonImportOutcome::NotFound { .. }
            | PythonImportOutcome::SkippedExternal { .. }
            | PythonImportOutcome::Unreadable { .. } => None,
        }));
        canonical.sort_by_key(import_edge_sort_key);
    }

    canonical
}

fn merge_import_status(
    existing: &mut PythonImportEvaluationStatus,
    incoming: PythonImportEvaluationStatus,
    is_cycle: bool,
) {
    let mut errors = import_status_errors(std::mem::replace(
        existing,
        PythonImportEvaluationStatus::Resolved,
    ));
    for error in import_status_errors(incoming) {
        if !errors.contains(&error) {
            errors.push(error);
        }
    }
    *existing = import_status_from_errors(errors, is_cycle);
}

fn normalize_import_status(
    status: PythonImportEvaluationStatus,
    is_cycle: bool,
) -> PythonImportEvaluationStatus {
    import_status_from_errors(import_status_errors(status), is_cycle)
}

fn import_status_errors(
    status: PythonImportEvaluationStatus,
) -> Vec<crate::python::PythonSyntaxError> {
    match status {
        PythonImportEvaluationStatus::Resolved => Vec::new(),
        PythonImportEvaluationStatus::SyntaxErrors(errors)
        | PythonImportEvaluationStatus::Cycle {
            syntax_errors: errors,
        } => errors,
    }
}

fn import_status_from_errors(
    errors: Vec<crate::python::PythonSyntaxError>,
    is_cycle: bool,
) -> PythonImportEvaluationStatus {
    if is_cycle {
        PythonImportEvaluationStatus::Cycle {
            syntax_errors: errors,
        }
    } else if errors.is_empty() {
        PythonImportEvaluationStatus::Resolved
    } else {
        PythonImportEvaluationStatus::SyntaxErrors(errors)
    }
}

fn import_edge(outcome: &PythonImportOutcome) -> Option<&PythonImportEdge> {
    match outcome {
        PythonImportOutcome::Evaluated { edge, .. }
        | PythonImportOutcome::Unreadable { edge, .. } => Some(edge),
        PythonImportOutcome::InvalidImport { .. }
        | PythonImportOutcome::NotFound { .. }
        | PythonImportOutcome::SkippedExternal { .. } => None,
    }
}

fn import_edge_sort_key(edge: &PythonImportEdge) -> (String, u32, u32, String) {
    (
        format!("{:?}", edge.importer),
        edge.origin.span.start(),
        edge.origin.span.length(),
        format!("{:?}", edge.imported),
    )
}

fn path_exists(
    edges: &[&PythonImportEdge],
    start: &PythonModule,
    destination: &PythonModule,
) -> bool {
    let mut pending = vec![start.clone()];
    let mut visited = FxHashSet::default();
    while let Some(module) = pending.pop() {
        if module == *destination {
            return true;
        }
        if !visited.insert(module.clone()) {
            continue;
        }
        pending.extend(
            edges
                .iter()
                .filter(|edge| edge.importer == module)
                .map(|edge| edge.imported.clone()),
        );
    }
    false
}
