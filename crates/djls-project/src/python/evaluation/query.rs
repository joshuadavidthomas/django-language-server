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
use super::result::CycleMembership;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::RecoveredPythonModule;

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
    let parsed = match RecoveredPythonModule::from_file(db, file) {
        Ok(Some(parsed)) => parsed,
        Err(error) => {
            return PythonModuleEvaluation::Evaluated {
                values: Err(error),
                dependencies: PythonModuleDependencies::rooted(file),
            };
        }
        Ok(None) => {
            return PythonModuleEvaluation::Evaluated {
                values: Ok(PythonModuleValues::default()),
                dependencies: PythonModuleDependencies::rooted(file),
            };
        }
    };
    let body = parsed.body(db);
    let (mut module_values, mut dependencies) =
        super::evaluator::evaluate_body(db, project, module.clone(), body);
    module_values.syntax_errors = parsed.syntax_errors(db).to_vec();
    module_values.syntax_impacts =
        super::touched_names::collect_syntax_impacts(body, &module_values.syntax_errors);
    let mut values = Ok(module_values);
    normalize_cycle_evaluation(&mut values, &mut dependencies, &module);
    PythonModuleEvaluation::Evaluated {
        values,
        dependencies,
    }
}

// This projection gives value consumers an independent red-green cutoff when only dependencies
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_values(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> Result<PythonModuleValues, FileReadError> {
    match evaluate_python_module(db, project, module) {
        PythonModuleEvaluation::CycleSeed => {
            unreachable!("cycle seed escaped Python module evaluation")
        }
        PythonModuleEvaluation::Evaluated { values, .. } => values.clone(),
    }
}

// This projection gives dependency consumers an independent red-green cutoff when only values
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_dependencies(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> PythonModuleDependencies {
    match evaluate_python_module(db, project, module) {
        PythonModuleEvaluation::CycleSeed => {
            unreachable!("cycle seed escaped Python module evaluation")
        }
        PythonModuleEvaluation::Evaluated { dependencies, .. } => dependencies.clone(),
    }
}

fn evaluate_python_module_cycle_initial(
    _db: &dyn ProjectDb,
    _id: salsa::Id,
    _project: Project,
    _module: PythonModule,
) -> PythonModuleEvaluation {
    PythonModuleEvaluation::CycleSeed
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
    let unchanged = previous == &computed;
    let PythonModuleEvaluation::Evaluated {
        mut values,
        mut dependencies,
    } = computed
    else {
        unreachable!("cycle seed cannot be a computed evaluation")
    };
    match previous {
        PythonModuleEvaluation::CycleSeed => {}
        PythonModuleEvaluation::Evaluated { .. } if unchanged => {}
        PythonModuleEvaluation::Evaluated {
            values: previous_values,
            dependencies: previous_dependencies,
        } => widen_cycle_evaluation(
            previous_values,
            previous_dependencies,
            &mut values,
            &mut dependencies,
            &module,
        ),
    }
    normalize_cycle_evaluation(&mut values, &mut dependencies, &module);
    PythonModuleEvaluation::Evaluated {
        values,
        dependencies,
    }
}

fn normalize_cycle_evaluation(
    values: &mut Result<PythonModuleValues, FileReadError>,
    dependencies: &mut PythonModuleDependencies,
    root: &PythonModule,
) {
    let import_graph = PythonImportGraph::new(std::mem::take(&mut dependencies.imports));
    let root_is_in_cycle = import_graph.root_participates_in_cycle(root);
    let root_file = root.file();
    if let Ok(values) = values {
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
    dependencies.imports = import_graph.canonicalized_outcomes();
    dependencies
        .files
        .sort_by_key(|file| (usize::from(*file != root_file), format!("{file:?}")));
}

fn widen_cycle_evaluation(
    previous_values: &Result<PythonModuleValues, FileReadError>,
    previous_dependencies: &PythonModuleDependencies,
    computed_values: &mut Result<PythonModuleValues, FileReadError>,
    computed_dependencies: &mut PythonModuleDependencies,
    root: &PythonModule,
) {
    match (previous_values, computed_values) {
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

    if previous_dependencies != computed_dependencies {
        *computed_dependencies =
            widen_cycle_dependencies(previous_dependencies, computed_dependencies, root);
    }
}

fn widen_cycle_dependencies(
    previous: &PythonModuleDependencies,
    computed: &PythonModuleDependencies,
    root: &PythonModule,
) -> PythonModuleDependencies {
    let mut candidates = previous.imports.clone();
    candidates.extend(computed.imports.iter().cloned());
    let mut candidates = PythonImportGraph::new(candidates).canonicalized_outcomes();
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

struct PythonImportGraph {
    outcomes: UniqueVec<PythonImportOutcome>,
}

impl PythonImportGraph {
    fn new(outcomes: UniqueVec<PythonImportOutcome>) -> Self {
        Self { outcomes }
    }

    fn root_participates_in_cycle(&self, root: &PythonModule) -> bool {
        self.outcomes
            .iter()
            .filter_map(PythonImportOutcome::edge)
            .any(|edge| edge.importer == *root && self.path_exists(&edge.imported, root))
    }

    fn path_exists(&self, start: &PythonModule, destination: &PythonModule) -> bool {
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
                self.outcomes
                    .iter()
                    .filter_map(PythonImportOutcome::edge)
                    .filter(|edge| edge.importer == module)
                    .map(|edge| edge.imported.clone()),
            );
        }
        false
    }

    fn canonical_cycle_edges(&self) -> Vec<PythonImportEdge> {
        let mut cyclic = self
            .outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                PythonImportOutcome::Evaluated { edge, .. } => Some(edge),
                PythonImportOutcome::InvalidImport { .. }
                | PythonImportOutcome::NotFound { .. }
                | PythonImportOutcome::SkippedExternal { .. }
                | PythonImportOutcome::Unreadable { .. } => None,
            })
            .filter(|edge| self.path_exists(&edge.imported, &edge.importer))
            .collect::<Vec<_>>();
        cyclic.sort_by_key(|edge| edge.canonical_sort_key());

        let mut canonical = Vec::new();
        for edge in cyclic {
            if !canonical.iter().any(|existing: &PythonImportEdge| {
                self.path_exists(&existing.importer, &edge.importer)
                    && self.path_exists(&edge.importer, &existing.importer)
            }) {
                canonical.push(edge.clone());
            }
        }

        if canonical.is_empty() {
            canonical.extend(self.outcomes.iter().filter_map(|outcome| match outcome {
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
            canonical.sort_by_key(PythonImportEdge::canonical_sort_key);
        }

        canonical
    }

    fn canonicalized_outcomes(self) -> UniqueVec<PythonImportOutcome> {
        let has_cycle = self.outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                PythonImportOutcome::Evaluated {
                    status: PythonImportEvaluationStatus::Cycle { .. },
                    ..
                }
            )
        });
        let canonical = if has_cycle {
            self.canonical_cycle_edges()
        } else {
            Vec::new()
        };

        let mut normalized: Vec<PythonImportOutcome> = Vec::new();
        for outcome in self.outcomes {
            match outcome {
                PythonImportOutcome::Evaluated { edge, status } => {
                    let membership = if canonical.contains(&edge) {
                        CycleMembership::Cycle
                    } else {
                        CycleMembership::Acyclic
                    };
                    if let Some(index) = normalized.iter().position(|outcome| {
                        matches!(outcome, PythonImportOutcome::Evaluated { edge: candidate, .. } if candidate == &edge)
                    }) {
                        let PythonImportOutcome::Evaluated {
                            status: existing, ..
                        } = normalized.remove(index)
                        else {
                            unreachable!("matched evaluated import outcome")
                        };
                        normalized.push(PythonImportOutcome::Evaluated {
                            edge,
                            status: existing.merged(status, membership),
                        });
                    } else {
                        normalized.push(PythonImportOutcome::Evaluated {
                            edge,
                            status: status.with_cycle_membership(membership),
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
        let mut outcomes = UniqueVec::from(normalized);
        outcomes.sort_by_key(|outcome| format!("{outcome:?}"));
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::Origin;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::*;
    use crate::python::PythonModuleName;
    use crate::python::SearchPath;

    fn module(name: &str, id: u64) -> PythonModule {
        let path = format!("/project/{name}.py");
        PythonModule::new(
            PythonModuleName::parse(name).unwrap(),
            None,
            Utf8PathBuf::from(&path),
            File::from_id(Id::from_bits(id)),
            SearchPath::FirstParty(Utf8PathBuf::from("/project")),
        )
    }

    fn evaluated_edge(
        source: &PythonModule,
        destination: &PythonModule,
        status: PythonImportEvaluationStatus,
    ) -> PythonImportOutcome {
        PythonImportOutcome::Evaluated {
            edge: PythonImportEdge {
                origin: Origin::new(source.file(), Span::new(0, 0)),
                importer: source.clone(),
                imported: destination.clone(),
            },
            status,
        }
    }

    fn cycle_status() -> PythonImportEvaluationStatus {
        PythonImportEvaluationStatus::Cycle {
            syntax_errors: Vec::new(),
        }
    }

    #[test]
    fn acyclic_graph_has_no_cycle_participant_or_canonical_edge() {
        let root = module("root", 1);
        let imported = module("imported", 2);
        let graph = PythonImportGraph::new(
            [evaluated_edge(
                &root,
                &imported,
                PythonImportEvaluationStatus::Resolved,
            )]
            .into_iter()
            .collect(),
        );

        assert!(!graph.root_participates_in_cycle(&root));
        assert!(graph.canonical_cycle_edges().is_empty());
    }

    #[test]
    fn direct_cycle_has_one_canonical_edge() {
        let root = module("root", 1);
        let imported = module("imported", 2);
        let graph = PythonImportGraph::new(
            [
                evaluated_edge(&root, &imported, cycle_status()),
                evaluated_edge(&imported, &root, cycle_status()),
            ]
            .into_iter()
            .collect(),
        );

        assert!(graph.root_participates_in_cycle(&root));
        assert_eq!(graph.canonical_cycle_edges().len(), 1);
    }

    #[test]
    fn disjoint_cycles_each_have_one_canonical_edge() {
        let first = module("first", 1);
        let second = module("second", 2);
        let third = module("third", 3);
        let fourth = module("fourth", 4);
        let graph = PythonImportGraph::new(
            [
                evaluated_edge(&first, &second, cycle_status()),
                evaluated_edge(&second, &first, cycle_status()),
                evaluated_edge(&third, &fourth, cycle_status()),
                evaluated_edge(&fourth, &third, cycle_status()),
            ]
            .into_iter()
            .collect(),
        );

        assert_eq!(graph.canonical_cycle_edges().len(), 2);
    }

    #[test]
    fn canonical_cycle_choice_is_independent_of_input_order() {
        let root = module("root", 1);
        let imported = module("imported", 2);
        let forward = evaluated_edge(&root, &imported, cycle_status());
        let reverse = evaluated_edge(&imported, &root, cycle_status());

        let first =
            PythonImportGraph::new([forward.clone(), reverse.clone()].into_iter().collect())
                .canonicalized_outcomes();
        let second = PythonImportGraph::new([reverse, forward].into_iter().collect())
            .canonicalized_outcomes();

        assert_eq!(first, second);
    }

    #[test]
    fn existing_cycle_edge_is_retained_when_reachability_has_not_converged() {
        let root = module("root", 1);
        let imported = module("imported", 2);
        let graph = PythonImportGraph::new(
            [evaluated_edge(&root, &imported, cycle_status())]
                .into_iter()
                .collect(),
        );

        let outcomes = graph.canonicalized_outcomes();

        assert!(matches!(
            outcomes.iter().next(),
            Some(PythonImportOutcome::Evaluated {
                status: PythonImportEvaluationStatus::Cycle { .. },
                ..
            })
        ));
    }
}
