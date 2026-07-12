use std::collections::BTreeSet;

use djls_source::File;

use super::PythonBinding;
use super::PythonBindingAlternative;
use super::PythonBoundValue;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleEvaluation;
use super::PythonModuleValues;
use super::PythonModuleValuesOutcome;
use super::PythonNamespaceCause;
use super::PythonNamespaceRemainder;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::RecoveredPythonModuleResult;
use crate::python::python_syntax_errors;
use crate::python::recovered_python_module;

#[salsa::tracked(
    cycle_initial=evaluate_python_module_cycle_initial,
    cycle_fn=evaluate_python_module_cycle_recover,
)]
pub(crate) fn evaluate_python_module(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> PythonModuleEvaluation {
    let body = match recovered_python_module(db, file) {
        RecoveredPythonModuleResult::Module(module) => module.body(db),
        RecoveredPythonModuleResult::Unreadable(error) => {
            return PythonModuleEvaluation::evaluated(
                PythonModuleValuesOutcome::Unreadable(error),
                PythonModuleDependencies {
                    files: vec![file],
                    imports: Vec::new(),
                },
            );
        }
        RecoveredPythonModuleResult::NotPython => {
            return PythonModuleEvaluation::evaluated(
                PythonModuleValuesOutcome::Readable(PythonModuleValues::default()),
                PythonModuleDependencies {
                    files: vec![file],
                    imports: Vec::new(),
                },
            );
        }
    };
    let mut evaluation = super::evaluator::evaluate_body(db, project, file, body);
    if let PythonModuleValuesOutcome::Readable(values) = &mut evaluation.values {
        values.syntax_errors = python_syntax_errors(db, file).cloned().unwrap_or_default();
        values.syntax_impacts =
            super::touched_names::collect_syntax_impacts(body, &values.syntax_errors);
    }
    normalize_cycle_evaluation(&mut evaluation, file);
    evaluation
}

#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_values(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> PythonModuleValuesOutcome {
    evaluate_python_module(db, project, file).values.clone()
}

#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_dependencies(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> PythonModuleDependencies {
    evaluate_python_module(db, project, file)
        .dependencies
        .clone()
}

fn evaluate_python_module_cycle_initial(
    _db: &dyn ProjectDb,
    _id: salsa::Id,
    _project: Project,
    _file: File,
) -> PythonModuleEvaluation {
    PythonModuleEvaluation::cycle_seed()
}

fn evaluate_python_module_cycle_recover(
    _db: &dyn ProjectDb,
    cycle: &salsa::Cycle,
    previous: &PythonModuleEvaluation,
    computed: PythonModuleEvaluation,
    _project: Project,
    file: File,
) -> PythonModuleEvaluation {
    assert!(
        cycle.iteration() < 12,
        "Python module cycle should converge within twelve iterations"
    );
    let mut recovered = if previous.is_cycle_seed() || previous == &computed {
        computed
    } else {
        widen_cycle_evaluation(previous, computed, file)
    };
    normalize_cycle_evaluation(&mut recovered, file);
    recovered
}

fn normalize_cycle_evaluation(evaluation: &mut PythonModuleEvaluation, root: File) {
    let root_is_in_cycle =
        root_participates_in_import_cycle(&evaluation.dependencies.imports, root);
    if let PythonModuleValuesOutcome::Readable(values) = &mut evaluation.values {
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
        .sort_by_key(|file| (usize::from(*file != root), format!("{file:?}")));
    evaluation.dependencies.files.dedup();
}

fn widen_cycle_evaluation(
    previous: &PythonModuleEvaluation,
    mut computed: PythonModuleEvaluation,
    root: File,
) -> PythonModuleEvaluation {
    match (&previous.values, &mut computed.values) {
        (
            PythonModuleValuesOutcome::Readable(previous_values),
            PythonModuleValuesOutcome::Readable(computed_values),
        ) => {
            let names = previous_values
                .bindings
                .0
                .keys()
                .chain(computed_values.bindings.0.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for name in names {
                if previous_values.bindings.get(&name) != computed_values.bindings.get(&name) {
                    computed_values.bindings.insert(name, cycle_binding());
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
            merge_unique(&mut computed_values.mutations, &previous_values.mutations);
            computed_values
                .mutations
                .sort_by_key(|mutation| format!("{mutation:?}"));
        }
        (
            PythonModuleValuesOutcome::Unreadable(previous),
            PythonModuleValuesOutcome::Unreadable(current),
        ) if previous == current => {}
        (
            PythonModuleValuesOutcome::Readable(_) | PythonModuleValuesOutcome::Unreadable(_),
            PythonModuleValuesOutcome::Unreadable(_),
        )
        | (PythonModuleValuesOutcome::Unreadable(_), PythonModuleValuesOutcome::Readable(_)) => {}
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
    root: File,
) -> PythonModuleDependencies {
    let mut candidates = previous.imports.clone();
    merge_unique(&mut candidates, &computed.imports);
    normalize_cycle_edges(&mut candidates);
    let cycle = candidates
        .iter()
        .find(|outcome| matches!(outcome, PythonImportOutcome::Cycle { .. }))
        .cloned();

    let mut files = vec![root];
    merge_unique(&mut files, &previous.files);
    merge_unique(&mut files, &computed.files);
    let mut imports = candidates;
    if let Some(PythonImportOutcome::Cycle { origin, file }) = cycle {
        merge_unique(&mut files, &[origin.file, file]);
    }
    files[1..].sort_by_key(|file| format!("{file:?}"));
    imports.sort_by_key(|outcome| format!("{outcome:?}"));
    imports.dedup();
    PythonModuleDependencies { files, imports }
}

fn cycle_binding() -> PythonBinding {
    PythonBinding::new(vec![PythonBindingAlternative::Bound(PythonBoundValue {
        value: PythonValue::unknown(PythonUnknownCause::Cycle, None),
        binding_origins: Vec::new(),
    })])
}

fn root_participates_in_import_cycle(imports: &[PythonImportOutcome], root: File) -> bool {
    let edges = imports
        .iter()
        .filter_map(|outcome| match outcome {
            PythonImportOutcome::Resolved { origin, file }
            | PythonImportOutcome::Cycle { origin, file } => Some((*origin, *file)),
            PythonImportOutcome::InvalidImport { .. }
            | PythonImportOutcome::NotFound { .. }
            | PythonImportOutcome::SkippedExternal { .. }
            | PythonImportOutcome::Unreadable { .. }
            | PythonImportOutcome::SyntaxErrors { .. } => None,
        })
        .collect::<Vec<_>>();

    edges
        .iter()
        .any(|(origin, target)| origin.file == root && path_exists(&edges, *target, root))
}

fn normalize_cycle_edges(imports: &mut Vec<PythonImportOutcome>) {
    if !imports
        .iter()
        .any(|outcome| matches!(outcome, PythonImportOutcome::Cycle { .. }))
    {
        imports.sort_by_key(|outcome| format!("{outcome:?}"));
        imports.dedup();
        return;
    }

    let edges = imports
        .iter()
        .filter_map(|outcome| match outcome {
            PythonImportOutcome::Resolved { origin, file }
            | PythonImportOutcome::Cycle { origin, file } => Some((*origin, *file)),
            PythonImportOutcome::InvalidImport { .. }
            | PythonImportOutcome::NotFound { .. }
            | PythonImportOutcome::SkippedExternal { .. }
            | PythonImportOutcome::Unreadable { .. }
            | PythonImportOutcome::SyntaxErrors { .. } => None,
        })
        .collect::<Vec<_>>();
    let edge_sort_key = |(origin, target): &(djls_source::Origin, File)| {
        (
            format!("{:?}", origin.file),
            origin.span.start(),
            origin.span.length(),
            format!("{target:?}"),
        )
    };
    let canonical = edges
        .iter()
        .copied()
        .filter(|(origin, target)| path_exists(&edges, *target, origin.file))
        .min_by_key(edge_sort_key)
        .or_else(|| {
            imports
                .iter()
                .filter_map(|outcome| match outcome {
                    PythonImportOutcome::Cycle { origin, file } => Some((*origin, *file)),
                    _ => None,
                })
                .min_by_key(edge_sort_key)
        });

    for outcome in imports.iter_mut() {
        let (PythonImportOutcome::Resolved { origin, file }
        | PythonImportOutcome::Cycle { origin, file }) = outcome
        else {
            continue;
        };
        let edge = (*origin, *file);
        *outcome = if Some(edge) == canonical {
            PythonImportOutcome::Cycle {
                origin: edge.0,
                file: edge.1,
            }
        } else {
            PythonImportOutcome::Resolved {
                origin: edge.0,
                file: edge.1,
            }
        };
    }
    imports.sort_by_key(|outcome| format!("{outcome:?}"));
    imports.dedup();
}

fn path_exists(edges: &[(djls_source::Origin, File)], start: File, destination: File) -> bool {
    let mut pending = vec![start];
    let mut visited = BTreeSet::new();
    while let Some(file) = pending.pop() {
        if file == destination {
            return true;
        }
        if !visited.insert(format!("{file:?}")) {
            continue;
        }
        pending.extend(
            edges
                .iter()
                .filter_map(|(origin, target)| (origin.file == file).then_some(*target)),
        );
    }
    false
}

fn merge_unique<T: Clone + PartialEq>(target: &mut Vec<T>, incoming: &[T]) {
    for item in incoming {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}
