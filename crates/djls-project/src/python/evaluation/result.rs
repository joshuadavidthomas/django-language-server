use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;
use djls_source::FileReadError;
use djls_source::Origin;
use rustc_hash::FxHashSet;

use super::BranchConstraints;
use super::PythonBinding;
use super::PythonMutation;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::UniqueVec;
use super::origin_sort_key;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonSyntaxError;
use crate::python::module::PythonImportError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PythonModuleValues {
    pub(crate) bindings: BTreeMap<String, PythonBinding>,
    pub(crate) namespace_remainder: Option<PythonNamespaceRemainder>,
    pub(crate) syntax_errors: Vec<PythonSyntaxError>,
    pub(crate) syntax_impacts: Vec<PythonSyntaxImpact>,
    pub(crate) mutations: UniqueVec<PythonMutation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonSyntaxImpact {
    pub(crate) error: PythonSyntaxError,
    pub(crate) names: BTreeSet<String>,
    pub(crate) namespace_open: bool,
    pub(crate) excluded_names: BTreeSet<String>,
}

impl PythonSyntaxImpact {
    pub(crate) fn affects(&self, name: &str) -> bool {
        self.names.contains(name) || (self.namespace_open && !self.excluded_names.contains(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonNamespaceCause {
    pub(crate) unknown: PythonUnknown,
    pub(crate) constraints: BranchConstraints,
}

impl PythonNamespaceCause {
    pub(super) fn unconstrained(unknown: PythonUnknown) -> Self {
        Self {
            unknown,
            constraints: BranchConstraints::unconstrained(),
        }
    }

    pub(super) fn select_branch(&mut self, join: Origin, arm: usize) {
        self.constraints.select(join, arm);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonNamespaceRemainder {
    pub(crate) causes: Vec<PythonNamespaceCause>,
}

impl PythonNamespaceRemainder {
    pub(super) fn new(mut causes: Vec<PythonNamespaceCause>) -> Self {
        causes.sort_by_key(|cause| {
            (
                format!("{:?}", cause.unknown.cause),
                cause
                    .unknown
                    .origins()
                    .map(|origin| origin_sort_key(&origin))
                    .collect::<Vec<_>>(),
                format!("{:?}", cause.constraints),
            )
        });
        let mut normalized: Vec<PythonNamespaceCause> = Vec::new();
        for cause in causes {
            if let Some(existing) = normalized
                .iter_mut()
                .find(|existing| existing.unknown.cause == cause.unknown.cause)
            {
                existing.unknown.merge_origins(&cause.unknown);
                existing.constraints.merge(cause.constraints);
            } else {
                normalized.push(cause);
            }
        }
        Self { causes: normalized }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PythonModuleEvaluation {
    CycleSeed,
    Evaluated(EvaluatedPythonModule),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvaluatedPythonModule {
    values: Result<PythonModuleValues, FileReadError>,
    dependencies: PythonModuleDependencies,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PythonModuleDependencies {
    pub(crate) files: UniqueVec<File>,
    pub(crate) imports: UniqueVec<PythonImportOutcome>,
}

impl PythonModuleDependencies {
    pub(super) fn rooted(file: File) -> Self {
        Self {
            files: [file].into_iter().collect(),
            imports: UniqueVec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonImportEdge {
    pub(crate) origin: Origin,
    pub(crate) importer: PythonModule,
    pub(crate) imported: PythonModule,
}

impl PythonImportEdge {
    fn canonical_sort_key(&self) -> (String, u32, u32, String) {
        (
            format!("{:?}", self.importer),
            self.origin.span.start(),
            self.origin.span.length(),
            format!("{:?}", self.imported),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CycleMembership {
    Acyclic,
    Cycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonImportEvaluationStatus {
    Resolved,
    SyntaxErrors(Vec<PythonSyntaxError>),
    Cycle {
        syntax_errors: Vec<PythonSyntaxError>,
    },
}

impl PythonImportEvaluationStatus {
    fn into_syntax_errors(self) -> Vec<PythonSyntaxError> {
        match self {
            Self::Resolved => Vec::new(),
            Self::SyntaxErrors(errors)
            | Self::Cycle {
                syntax_errors: errors,
            } => errors,
        }
    }

    pub(super) fn from_syntax_errors(
        errors: Vec<PythonSyntaxError>,
        membership: CycleMembership,
    ) -> Self {
        match (membership, errors.is_empty()) {
            (CycleMembership::Cycle, _) => Self::Cycle {
                syntax_errors: errors,
            },
            (CycleMembership::Acyclic, true) => Self::Resolved,
            (CycleMembership::Acyclic, false) => Self::SyntaxErrors(errors),
        }
    }

    fn with_cycle_membership(self, membership: CycleMembership) -> Self {
        Self::from_syntax_errors(self.into_syntax_errors(), membership)
    }

    fn merged(self, other: Self, membership: CycleMembership) -> Self {
        let mut errors = self.into_syntax_errors();
        for error in other.into_syntax_errors() {
            if !errors.contains(&error) {
                errors.push(error);
            }
        }
        Self::from_syntax_errors(errors, membership)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonImportOutcome {
    Evaluated {
        edge: PythonImportEdge,
        status: PythonImportEvaluationStatus,
    },
    InvalidImport {
        origin: Origin,
        reason: PythonImportError,
    },
    NotFound {
        origin: Origin,
        module: PythonModuleName,
    },
    SkippedExternal {
        origin: Origin,
        module: PythonModuleName,
    },
    Unreadable {
        edge: PythonImportEdge,
        error: FileReadError,
    },
}

impl PythonImportOutcome {
    fn edge(&self) -> Option<&PythonImportEdge> {
        match self {
            Self::Evaluated { edge, .. } | Self::Unreadable { edge, .. } => Some(edge),
            Self::InvalidImport { .. } | Self::NotFound { .. } | Self::SkippedExternal { .. } => {
                None
            }
        }
    }
}

impl EvaluatedPythonModule {
    pub(super) fn new(
        mut values: Result<PythonModuleValues, FileReadError>,
        mut dependencies: PythonModuleDependencies,
        root: &PythonModule,
    ) -> Self {
        let import_graph = PythonImportGraph::new(std::mem::take(&mut dependencies.imports));
        let root_is_in_cycle = import_graph.root_participates_in_cycle(root);
        let root_file = root.file();
        if let Ok(values) = &mut values {
            values
                .mutations
                .sort_by_key(|mutation| format!("{mutation:?}"));
            if root_is_in_cycle && let Some(remainder) = &mut values.namespace_remainder {
                for cause in &mut remainder.causes {
                    if cause.unknown.cause == PythonUnknownCause::Cycle {
                        cause.unknown.replace_origins(None);
                    }
                }
                *remainder = PythonNamespaceRemainder::new(remainder.causes.clone());
            }
        }
        dependencies.imports = import_graph.canonicalized_outcomes();
        dependencies
            .files
            .sort_by_key(|file| (usize::from(*file != root_file), format!("{file:?}")));
        Self {
            values,
            dependencies,
        }
    }

    pub(super) fn values(&self) -> &Result<PythonModuleValues, FileReadError> {
        &self.values
    }

    pub(super) fn dependencies(&self) -> &PythonModuleDependencies {
        &self.dependencies
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Result<PythonModuleValues, FileReadError>,
        PythonModuleDependencies,
    ) {
        (self.values, self.dependencies)
    }

    pub(super) fn widened(mut self, previous: &Self, root: &PythonModule) -> Self {
        match (&previous.values, &mut self.values) {
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
                    computed_values.namespace_remainder =
                        Some(PythonNamespaceRemainder::new(vec![
                            PythonNamespaceCause::unconstrained(PythonUnknown::new(
                                PythonUnknownCause::Cycle,
                                None,
                            )),
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

        if previous.dependencies != self.dependencies {
            self.dependencies = self.dependencies.widened(&previous.dependencies, root);
        }
        Self::new(self.values, self.dependencies, root)
    }
}

impl PythonModuleDependencies {
    fn widened(self, previous: &Self, root: &PythonModule) -> Self {
        let mut candidates = previous.imports.clone();
        candidates.extend(self.imports.iter().cloned());
        let candidates = PythonImportGraph::new(candidates).canonicalized_outcomes();
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
        files.extend(self.files.iter().copied());
        if let Some(edge) = cycle {
            files.extend([edge.importer.file(), edge.imported.file()]);
        }
        files.sort_by_key(|file| (usize::from(*file != root_file), format!("{file:?}")));
        Self {
            files,
            imports: candidates,
        }
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
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::*;
    use crate::python::PythonSyntaxErrorClass;
    use crate::python::SearchPath;

    fn syntax_error(message: &str) -> PythonSyntaxError {
        PythonSyntaxError {
            class: PythonSyntaxErrorClass::Ordinary,
            span: Span::new(0, 0),
            message: message.to_string(),
        }
    }

    #[test]
    fn canonical_unknown_origins_are_empty_during_cycle_widening() {
        let root = module("root", 1);
        let previous_origin = Origin::new(root.file(), Span::new(10, 1));
        let computed_origin = Origin::new(root.file(), Span::new(20, 1));

        let mut previous_values = PythonModuleValues::default();
        previous_values.bindings.insert(
            "VALUE".to_string(),
            PythonBinding::bound(
                super::super::PythonValue::string("before".to_string(), previous_origin),
                previous_origin,
            ),
        );
        let mut computed_values = PythonModuleValues::default();
        computed_values.bindings.insert(
            "VALUE".to_string(),
            PythonBinding::bound(
                super::super::PythonValue::bool(true, computed_origin),
                computed_origin,
            ),
        );
        computed_values.namespace_remainder = Some(PythonNamespaceRemainder::new(vec![
            PythonNamespaceCause::unconstrained(PythonUnknown::new(
                PythonUnknownCause::UnsupportedExpression,
                [computed_origin],
            )),
        ]));

        let previous = EvaluatedPythonModule {
            values: Ok(previous_values),
            dependencies: PythonModuleDependencies::rooted(root.file()),
        };
        let computed = EvaluatedPythonModule {
            values: Ok(computed_values),
            dependencies: PythonModuleDependencies::rooted(root.file()),
        };
        let widened = computed.widened(&previous, &root);
        let values = widened
            .values()
            .as_ref()
            .expect("widening should remain readable");

        let bound = values
            .bindings
            .get("VALUE")
            .and_then(PythonBinding::single_bound)
            .expect("changed binding should widen to one cycle unknown");
        let unknown = bound
            .value
            .unknown_value()
            .expect("changed binding should become unknown");
        assert_eq!(unknown.cause, PythonUnknownCause::Cycle);
        assert!(unknown.origins().next().is_none());
        assert!(bound.value.origins().next().is_none());
        assert!(bound.binding_origins().next().is_none());

        let [cause] = values
            .namespace_remainder
            .as_ref()
            .expect("changed namespace should widen")
            .causes
            .as_slice()
        else {
            panic!("namespace widening should produce one cycle cause");
        };
        assert_eq!(cause.unknown.cause, PythonUnknownCause::Cycle);
        assert!(cause.unknown.origins().next().is_none());
    }

    #[test]
    fn import_status_cycle_membership_matrix() {
        let error = syntax_error("broken");

        assert_eq!(
            PythonImportEvaluationStatus::Resolved.with_cycle_membership(CycleMembership::Acyclic),
            PythonImportEvaluationStatus::Resolved
        );
        assert_eq!(
            PythonImportEvaluationStatus::SyntaxErrors(vec![error.clone()])
                .with_cycle_membership(CycleMembership::Acyclic),
            PythonImportEvaluationStatus::SyntaxErrors(vec![error.clone()])
        );
        assert_eq!(
            PythonImportEvaluationStatus::Resolved.with_cycle_membership(CycleMembership::Cycle),
            PythonImportEvaluationStatus::Cycle {
                syntax_errors: Vec::new(),
            }
        );
        assert_eq!(
            PythonImportEvaluationStatus::SyntaxErrors(vec![error.clone()])
                .with_cycle_membership(CycleMembership::Cycle),
            PythonImportEvaluationStatus::Cycle {
                syntax_errors: vec![error],
            }
        );
    }

    #[test]
    fn merged_import_status_preserves_unique_error_order() {
        let first = syntax_error("first");
        let second = syntax_error("second");

        assert_eq!(
            PythonImportEvaluationStatus::SyntaxErrors(vec![first.clone(), second.clone()]).merged(
                PythonImportEvaluationStatus::Cycle {
                    syntax_errors: vec![second, first.clone()],
                },
                CycleMembership::Cycle,
            ),
            PythonImportEvaluationStatus::Cycle {
                syntax_errors: vec![first, syntax_error("second")],
            }
        );
    }

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
    fn unreadable_edges_are_navigable_but_not_canonical_cycle_candidates() {
        let root = module("root", 1);
        let imported = module("imported", 2);
        let closing_edge = PythonImportEdge {
            origin: Origin::new(imported.file(), Span::new(0, 0)),
            importer: imported.clone(),
            imported: root.clone(),
        };
        let graph = PythonImportGraph::new(
            [
                PythonImportOutcome::Unreadable {
                    edge: PythonImportEdge {
                        origin: Origin::new(root.file(), Span::new(0, 0)),
                        importer: root.clone(),
                        imported: imported.clone(),
                    },
                    error: FileReadError::new(
                        Utf8PathBuf::from("/project/imported.py"),
                        std::io::ErrorKind::PermissionDenied,
                    ),
                },
                PythonImportOutcome::Evaluated {
                    edge: closing_edge.clone(),
                    status: cycle_status(),
                },
            ]
            .into_iter()
            .collect(),
        );

        assert!(graph.root_participates_in_cycle(&root));
        assert_eq!(graph.canonical_cycle_edges(), vec![closing_edge]);
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
