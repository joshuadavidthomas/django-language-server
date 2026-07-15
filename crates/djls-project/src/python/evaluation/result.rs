use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;
use djls_source::FileReadError;
use djls_source::Origin;

use super::BranchConstraints;
use super::PythonBinding;
use super::PythonMutation;
use super::PythonUnknown;
use super::UniqueVec;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonSyntaxError;
use crate::python::module::PythonImportError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonModuleValuesOutcome {
    Readable(PythonModuleValues),
    Unreadable(FileReadError),
}

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
                    .origin
                    .map(|origin| format!("{:?}", origin.file)),
                cause.unknown.origin.map(|origin| origin.span.start()),
                cause.unknown.origin.map(|origin| origin.span.length()),
                format!("{:?}", cause.constraints),
            )
        });
        let mut normalized: Vec<PythonNamespaceCause> = Vec::new();
        for cause in causes {
            if let Some(existing) = normalized
                .iter_mut()
                .find(|existing| existing.unknown == cause.unknown)
            {
                existing.constraints.merge(cause.constraints);
            } else {
                normalized.push(cause);
            }
        }
        Self { causes: normalized }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonModuleEvaluation {
    pub(super) values: PythonModuleValuesOutcome,
    pub(super) dependencies: PythonModuleDependencies,
    cycle_seed: bool,
}

impl PythonModuleEvaluation {
    pub(super) fn evaluated(
        values: PythonModuleValuesOutcome,
        dependencies: PythonModuleDependencies,
    ) -> Self {
        Self {
            values,
            dependencies,
            cycle_seed: false,
        }
    }

    pub(super) fn cycle_seed() -> Self {
        Self {
            values: PythonModuleValuesOutcome::Readable(PythonModuleValues::default()),
            dependencies: PythonModuleDependencies::default(),
            cycle_seed: true,
        }
    }

    pub(super) const fn is_cycle_seed(&self) -> bool {
        self.cycle_seed
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonImportEvaluationStatus {
    Resolved,
    SyntaxErrors(Vec<PythonSyntaxError>),
    Cycle {
        syntax_errors: Vec<PythonSyntaxError>,
    },
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
