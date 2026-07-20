use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::mem;

use djls_source::File;
use djls_source::FileReadError;
use djls_source::Origin;
use rustc_hash::FxHashSet;

use super::BranchConstraints;
use super::PythonBinding;
use super::PythonModuleObjects;
use super::PythonMutation;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::StructuralOrd;
use super::UniqueVec;
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

impl StructuralOrd for PythonNamespaceCause {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.unknown
            .structural_cmp(&other.unknown)
            .then_with(|| self.constraints.structural_cmp(&other.constraints))
    }
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
pub(crate) struct PythonNamespaceRemainder(Vec<PythonNamespaceCause>);

impl PythonNamespaceRemainder {
    pub(super) fn new(mut causes: Vec<PythonNamespaceCause>) -> Self {
        causes.sort_by(PythonNamespaceCause::structural_cmp);
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
        Self(normalized)
    }

    pub(crate) fn as_slice(&self) -> &[PythonNamespaceCause] {
        &self.0
    }

    pub(crate) fn into_causes(self) -> Vec<PythonNamespaceCause> {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PythonModuleEvaluation {
    CycleSeed,
    Evaluated(Box<EvaluatedPythonModule>),
}

impl PythonModuleEvaluation {
    pub(super) fn evaluated(module: EvaluatedPythonModule) -> Self {
        Self::Evaluated(Box::new(module))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvaluatedPythonModule {
    values: Result<PythonModuleValues, FileReadError>,
    dependencies: PythonModuleDependencies,
    /// Private recursive-import effect data. It is intentionally part of this
    /// internal result's equality (so imported effects can trigger the core
    /// query) but is never projected into settings-facing `PythonModuleValues`.
    module_objects: PythonModuleObjects,
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

    /// Whether any recorded import outcome participates in a cycle. Cyclic
    /// dependency sets are canonicalized into an entry-order-independent order;
    /// acyclic sets keep their semantic first-seen order.
    fn has_cycle_outcome(&self) -> bool {
        self.imports.iter().any(|outcome| {
            matches!(
                outcome,
                PythonImportOutcome::Evaluated {
                    status: PythonImportEvaluationStatus::Cycle { .. },
                    ..
                }
            )
        })
    }

    fn sort_files(&mut self, root: File) {
        self.files.sort_by(|left, right| {
            (*left != root)
                .cmp(&(*right != root))
                .then_with(|| left.structural_cmp(right))
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonImportEdge {
    pub(crate) origin: Origin,
    pub(crate) importer: PythonModule,
    pub(crate) imported: PythonModule,
}

impl StructuralOrd for PythonImportEdge {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.importer
            .structural_cmp(&other.importer)
            .then_with(|| self.origin.structural_cmp(&other.origin))
            .then_with(|| self.imported.structural_cmp(&other.imported))
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
    /// Status precedence preserves the former variant-name ordering:
    /// Cycle < Resolved < `SyntaxErrors`.
    fn structural_rank(&self) -> u8 {
        match self {
            Self::Cycle { .. } => 0,
            Self::Resolved => 1,
            Self::SyntaxErrors(_) => 2,
        }
    }
}

impl StructuralOrd for PythonImportEvaluationStatus {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        let ordering = self.structural_rank().cmp(&other.structural_rank());
        if ordering != Ordering::Equal {
            return ordering;
        }
        match (self, other) {
            (Self::Resolved, Self::Resolved) => Ordering::Equal,
            (Self::SyntaxErrors(left), Self::SyntaxErrors(right))
            | (
                Self::Cycle {
                    syntax_errors: left,
                },
                Self::Cycle {
                    syntax_errors: right,
                },
            ) => left.as_slice().structural_cmp(right.as_slice()),
            (
                Self::Resolved | Self::SyntaxErrors(_) | Self::Cycle { .. },
                Self::Resolved | Self::SyntaxErrors(_) | Self::Cycle { .. },
            ) => unreachable!("equal import-status ranks identify the same variant"),
        }
    }
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
    /// Outcome precedence preserves the established observable variant order.
    fn structural_rank(&self) -> u8 {
        match self {
            Self::Evaluated { .. } => 0,
            Self::InvalidImport { .. } => 1,
            Self::NotFound { .. } => 2,
            Self::SkippedExternal { .. } => 3,
            Self::Unreadable { .. } => 4,
        }
    }
}

impl StructuralOrd for PythonImportOutcome {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        let ordering = self.structural_rank().cmp(&other.structural_rank());
        if ordering != Ordering::Equal {
            return ordering;
        }
        match (self, other) {
            (
                Self::Evaluated {
                    edge: left_edge,
                    status: left_status,
                },
                Self::Evaluated {
                    edge: right_edge,
                    status: right_status,
                },
            ) => left_edge
                .structural_cmp(right_edge)
                .then_with(|| left_status.structural_cmp(right_status)),
            (
                Self::InvalidImport {
                    origin: left_origin,
                    reason: left_reason,
                },
                Self::InvalidImport {
                    origin: right_origin,
                    reason: right_reason,
                },
            ) => left_origin
                .structural_cmp(right_origin)
                .then_with(|| left_reason.structural_cmp(right_reason)),
            (
                Self::NotFound {
                    origin: left_origin,
                    module: left_module,
                },
                Self::NotFound {
                    origin: right_origin,
                    module: right_module,
                },
            )
            | (
                Self::SkippedExternal {
                    origin: left_origin,
                    module: left_module,
                },
                Self::SkippedExternal {
                    origin: right_origin,
                    module: right_module,
                },
            ) => left_origin
                .structural_cmp(right_origin)
                .then_with(|| left_module.cmp(right_module)),
            (
                Self::Unreadable {
                    edge: left_edge,
                    error: left_error,
                },
                Self::Unreadable {
                    edge: right_edge,
                    error: right_error,
                },
            ) => left_edge
                .structural_cmp(right_edge)
                .then_with(|| left_error.structural_cmp(right_error)),
            (
                Self::Evaluated { .. }
                | Self::InvalidImport { .. }
                | Self::NotFound { .. }
                | Self::SkippedExternal { .. }
                | Self::Unreadable { .. },
                Self::Evaluated { .. }
                | Self::InvalidImport { .. }
                | Self::NotFound { .. }
                | Self::SkippedExternal { .. }
                | Self::Unreadable { .. },
            ) => unreachable!("equal import-outcome ranks identify the same variant"),
        }
    }
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
        module_objects: PythonModuleObjects,
        root: &PythonModule,
    ) -> Self {
        let import_graph = PythonImportGraph::new(mem::take(&mut dependencies.imports));
        let root_is_in_cycle = import_graph.root_participates_in_cycle(root);
        let root_file = root.file();
        if let Ok(values) = &mut values {
            values.mutations.sort_by(PythonMutation::structural_cmp);
            if root_is_in_cycle && let Some(remainder) = &mut values.namespace_remainder {
                for cause in &mut remainder.0 {
                    if cause.unknown.cause == PythonUnknownCause::Cycle {
                        cause.unknown.replace_origins(None);
                    }
                }
                *remainder = PythonNamespaceRemainder::new(remainder.0.clone());
            }
        }
        dependencies.imports = import_graph.canonicalized_outcomes();
        // Files/outcomes keep first-seen root-to-leaf insertion order for the
        // acyclic case. Only a cycle needs entry-order-independent
        // canonicalization, so only then is the structural file order imposed.
        if dependencies.has_cycle_outcome() {
            dependencies.sort_files(root_file);
        }
        Self {
            values,
            dependencies,
            module_objects,
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
        PythonModuleObjects,
    ) {
        (self.values, self.dependencies, self.module_objects)
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
                    .sort_by(PythonMutation::structural_cmp);
            }
            (Ok(_) | Err(_), Err(_)) | (Err(_), Ok(_)) => {}
        }

        if previous.dependencies != self.dependencies {
            self.dependencies = self.dependencies.widened(&previous.dependencies, root);
        }
        if previous.module_objects != self.module_objects {
            self.module_objects = self.module_objects.widen(&previous.module_objects);
        }
        Self::new(self.values, self.dependencies, self.module_objects, root)
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
        let mut dependencies = Self {
            files,
            imports: candidates,
        };
        dependencies.sort_files(root_file);
        dependencies
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
        cyclic.sort_by(|left, right| left.structural_cmp(right));

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
            canonical.sort_by(PythonImportEdge::structural_cmp);
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
        // Preserve first-seen outcome order for acyclic dependency sets; only
        // impose the structural canonical order when a cycle requires
        // entry-order independence.
        if has_cycle {
            normalized.sort_by(PythonImportOutcome::structural_cmp);
        }
        normalized.into()
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::io::ErrorKind;
    use std::slice;

    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::*;
    use crate::python::InvalidModuleName;
    use crate::python::PythonSyntaxErrorClass;
    use crate::python::SearchPath;
    use crate::python::evaluation::PythonValue;

    fn syntax_error(message: &str) -> PythonSyntaxError {
        PythonSyntaxError {
            class: PythonSyntaxErrorClass::Ordinary,
            span: Span::new(0, 0),
            message: message.to_string(),
        }
    }

    fn origin(file_index: u64, start: u32) -> Origin {
        Origin::new(
            File::from_id(Id::from_bits(file_index)),
            Span::new(start, 1),
        )
    }

    #[test]
    fn typed_module_order_namespace_causes_compare_unknowns_origins_and_constraints() {
        let base = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedExpression,
            [origin(15, 1)],
        ));
        let different_cause = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedMutation,
            [origin(15, 1)],
        ));
        let different_origin = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedExpression,
            [origin(16, 1)],
        ));
        let mut different_constraints = base.clone();
        different_constraints.select_branch(origin(15, 20), 1);

        assert_eq!(base.structural_cmp(&base), Ordering::Equal);
        for other in [&different_cause, &different_origin, &different_constraints] {
            assert_ne!(base.structural_cmp(other), Ordering::Equal);
            assert_eq!(
                base.structural_cmp(other),
                other.structural_cmp(&base).reverse()
            );
        }
    }

    #[test]
    fn typed_module_order_import_statuses_are_exhaustive_total_and_payload_complete() {
        let statuses = [
            PythonImportEvaluationStatus::Cycle {
                syntax_errors: Vec::new(),
            },
            PythonImportEvaluationStatus::Resolved,
            PythonImportEvaluationStatus::SyntaxErrors(Vec::new()),
        ];
        for (left_index, left) in statuses.iter().enumerate() {
            for (right_index, right) in statuses.iter().enumerate() {
                let ordering = left.structural_cmp(right);
                assert_eq!(ordering, right.structural_cmp(left).reverse());
                assert_eq!(ordering == Ordering::Equal, left == right);
                assert_eq!(ordering, left_index.cmp(&right_index));
            }
        }

        let base = PythonImportEvaluationStatus::SyntaxErrors(vec![syntax_error("a")]);
        let payloads = [
            PythonImportEvaluationStatus::SyntaxErrors(vec![PythonSyntaxError {
                class: PythonSyntaxErrorClass::Unsupported,
                ..syntax_error("a")
            }]),
            PythonImportEvaluationStatus::SyntaxErrors(vec![PythonSyntaxError {
                span: Span::new(1, 1),
                ..syntax_error("a")
            }]),
            PythonImportEvaluationStatus::SyntaxErrors(vec![syntax_error("b")]),
            PythonImportEvaluationStatus::SyntaxErrors(vec![syntax_error("a"), syntax_error("b")]),
        ];
        for other in &payloads {
            assert_ne!(base.structural_cmp(other), Ordering::Equal);
            assert_eq!(
                base.structural_cmp(other),
                other.structural_cmp(&base).reverse()
            );
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
                PythonValue::string("before".to_string(), previous_origin),
                previous_origin,
            ),
        );
        let mut computed_values = PythonModuleValues::default();
        computed_values.bindings.insert(
            "VALUE".to_string(),
            PythonBinding::bound(PythonValue::bool(true, computed_origin), computed_origin),
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
            module_objects: PythonModuleObjects::default(),
        };
        let computed = EvaluatedPythonModule {
            values: Ok(computed_values),
            dependencies: PythonModuleDependencies::rooted(root.file()),
            module_objects: PythonModuleObjects::default(),
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
        PythonModule::file_module(
            PythonModuleName::parse(name).unwrap(),
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
    fn typed_module_order_acyclic_dependencies_preserve_first_seen_order() {
        let root = module("root", 16);
        let numerically_first = File::from_id(Id::from_bits(15));
        let numerically_last = File::from_id(Id::from_bits(17));
        let evaluate = |files: [File; 3]| {
            EvaluatedPythonModule::new(
                Ok(PythonModuleValues::default()),
                PythonModuleDependencies {
                    files: files.into_iter().collect(),
                    imports: UniqueVec::new(),
                },
                PythonModuleObjects::default(),
                &root,
            )
        };

        // Acyclic dependency files keep first-seen insertion order; the root is
        // first because real evaluation seeds it first, not because of a sort.
        let forward = evaluate([root.file(), numerically_last, numerically_first]);
        assert_eq!(
            forward
                .dependencies
                .files
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [root.file(), numerically_last, numerically_first]
        );
        let reversed = evaluate([root.file(), numerically_first, numerically_last]);
        assert_eq!(
            reversed
                .dependencies
                .files
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [root.file(), numerically_first, numerically_last]
        );
    }

    #[test]
    fn typed_module_order_import_edges_compare_complete_module_and_origin_identity() {
        let source = module("same", 15);
        let destination = module("imported", 17);
        let base = PythonImportEdge {
            origin: Origin::new(source.file(), Span::new(1, 1)),
            importer: source.clone(),
            imported: destination.clone(),
        };
        let unequal = [
            PythonImportEdge {
                importer: module("same", 16),
                ..base.clone()
            },
            PythonImportEdge {
                origin: Origin::new(File::from_id(Id::from_bits(16)), Span::new(1, 1)),
                ..base.clone()
            },
            PythonImportEdge {
                imported: module("imported", 18),
                ..base.clone()
            },
        ];

        assert_eq!(base.structural_cmp(&base), Ordering::Equal);
        for other in &unequal {
            assert_ne!(base.structural_cmp(other), Ordering::Equal);
            assert_eq!(
                base.structural_cmp(other),
                other.structural_cmp(&base).reverse()
            );
        }
    }

    #[test]
    fn typed_module_order_import_outcomes_are_exhaustive_total_and_preserve_first_seen_order() {
        let source = module("importer", 15);
        let destination = module("imported", 16);
        let edge = PythonImportEdge {
            origin: Origin::new(source.file(), Span::new(1, 1)),
            importer: source,
            imported: destination,
        };
        let outcomes = [
            PythonImportOutcome::Evaluated {
                edge: edge.clone(),
                status: PythonImportEvaluationStatus::Resolved,
            },
            PythonImportOutcome::InvalidImport {
                origin: edge.origin,
                reason: PythonImportError::TooManyDots,
            },
            PythonImportOutcome::NotFound {
                origin: edge.origin,
                module: PythonModuleName::parse("missing").unwrap(),
            },
            PythonImportOutcome::SkippedExternal {
                origin: edge.origin,
                module: PythonModuleName::parse("external").unwrap(),
            },
            PythonImportOutcome::Unreadable {
                edge,
                error: FileReadError::new(
                    Utf8PathBuf::from("/project/unreadable.py"),
                    ErrorKind::PermissionDenied,
                ),
            },
        ];

        for (left_index, left) in outcomes.iter().enumerate() {
            for (right_index, right) in outcomes.iter().enumerate() {
                let ordering = left.structural_cmp(right);
                assert_eq!(ordering, right.structural_cmp(left).reverse());
                assert_eq!(ordering == Ordering::Equal, left == right);
                assert_eq!(ordering, left_index.cmp(&right_index));
            }
        }

        // Acyclic outcome sets keep first-seen insertion order; only a cycle
        // imposes the input-independent canonical order (covered by the cycle
        // tests below).
        let forward = PythonImportGraph::new(outcomes.clone().into_iter().collect())
            .canonicalized_outcomes()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(forward, outcomes.to_vec());
        let reversed = PythonImportGraph::new(outcomes.iter().rev().cloned().collect())
            .canonicalized_outcomes()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(reversed, outcomes.iter().rev().cloned().collect::<Vec<_>>());
    }

    #[test]
    fn typed_module_order_import_outcomes_compare_every_variant_payload() {
        let source = module("importer", 15);
        let destination = module("imported", 16);
        let edge = PythonImportEdge {
            origin: Origin::new(source.file(), Span::new(1, 1)),
            importer: source,
            imported: destination,
        };
        let pairs = [
            (
                PythonImportOutcome::Evaluated {
                    edge: edge.clone(),
                    status: PythonImportEvaluationStatus::Resolved,
                },
                PythonImportOutcome::Evaluated {
                    edge: edge.clone(),
                    status: PythonImportEvaluationStatus::SyntaxErrors(vec![syntax_error("a")]),
                },
            ),
            (
                PythonImportOutcome::InvalidImport {
                    origin: edge.origin,
                    reason: PythonImportError::EmptyAbsoluteImport,
                },
                PythonImportOutcome::InvalidImport {
                    origin: edge.origin,
                    reason: PythonImportError::InvalidModuleName(
                        InvalidModuleName::InvalidSegment("!".to_string()),
                    ),
                },
            ),
            (
                PythonImportOutcome::NotFound {
                    origin: edge.origin,
                    module: PythonModuleName::parse("a").unwrap(),
                },
                PythonImportOutcome::NotFound {
                    origin: Origin::new(edge.origin.file, Span::new(2, 1)),
                    module: PythonModuleName::parse("b").unwrap(),
                },
            ),
            (
                PythonImportOutcome::SkippedExternal {
                    origin: edge.origin,
                    module: PythonModuleName::parse("a").unwrap(),
                },
                PythonImportOutcome::SkippedExternal {
                    origin: edge.origin,
                    module: PythonModuleName::parse("b").unwrap(),
                },
            ),
            (
                PythonImportOutcome::Unreadable {
                    edge: edge.clone(),
                    error: FileReadError::new(
                        Utf8PathBuf::from("/project/a.py"),
                        ErrorKind::NotFound,
                    ),
                },
                PythonImportOutcome::Unreadable {
                    edge,
                    error: FileReadError::new(
                        Utf8PathBuf::from("/project/a.py"),
                        ErrorKind::PermissionDenied,
                    ),
                },
            ),
        ];

        for (left, right) in pairs {
            assert_ne!(left.structural_cmp(&right), Ordering::Equal);
            assert_eq!(
                left.structural_cmp(&right),
                right.structural_cmp(&left).reverse()
            );
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
    fn typed_module_order_cycle_selection_breaks_old_debug_collisions_stably() {
        // These modules have the same diagnostic Debug view but distinct typed
        // File identity. The canonical edge must not depend on insertion order.
        let first = module("same", 15);
        let second = module("same", 16);
        let forward = evaluated_edge(&first, &second, cycle_status());
        let reverse = evaluated_edge(&second, &first, cycle_status());
        let PythonImportOutcome::Evaluated {
            edge: expected_edge,
            ..
        } = &forward
        else {
            unreachable!("helper always creates an evaluated edge")
        };

        let first_order =
            PythonImportGraph::new([forward.clone(), reverse.clone()].into_iter().collect())
                .canonical_cycle_edges();
        let reversed_order =
            PythonImportGraph::new([reverse.clone(), forward.clone()].into_iter().collect())
                .canonical_cycle_edges();
        assert_eq!(first_order.as_slice(), slice::from_ref(expected_edge));
        assert_eq!(first_order, reversed_order);

        let first_outcomes =
            PythonImportGraph::new([forward.clone(), reverse.clone()].into_iter().collect())
                .canonicalized_outcomes();
        let reversed_outcomes = PythonImportGraph::new([reverse, forward].into_iter().collect())
            .canonicalized_outcomes();
        assert_eq!(first_outcomes, reversed_outcomes);
    }

    #[test]
    fn typed_module_order_overlapping_cycle_selection_is_reversal_stable() {
        let first = module("first", 15);
        let second = module("second", 16);
        let third = module("third", 17);
        let outcomes = [
            evaluated_edge(&first, &second, cycle_status()),
            evaluated_edge(&second, &first, cycle_status()),
            evaluated_edge(&second, &third, cycle_status()),
            evaluated_edge(&third, &second, cycle_status()),
        ];

        let forward =
            PythonImportGraph::new(outcomes.clone().into_iter().collect()).canonical_cycle_edges();
        let reversed =
            PythonImportGraph::new(outcomes.into_iter().rev().collect()).canonical_cycle_edges();

        assert_eq!(forward.len(), 1);
        assert_eq!(forward, reversed);
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
                        ErrorKind::PermissionDenied,
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
