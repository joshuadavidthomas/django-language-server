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
pub(crate) enum PythonModuleEvaluation {
    CycleSeed,
    Evaluated {
        values: Result<PythonModuleValues, FileReadError>,
        dependencies: PythonModuleDependencies,
    },
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
    pub(super) fn canonical_sort_key(&self) -> (String, u32, u32, String) {
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

    pub(super) fn with_cycle_membership(self, membership: CycleMembership) -> Self {
        Self::from_syntax_errors(self.into_syntax_errors(), membership)
    }

    pub(super) fn merged(self, other: Self, membership: CycleMembership) -> Self {
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
    pub(super) fn edge(&self) -> Option<&PythonImportEdge> {
        match self {
            Self::Evaluated { edge, .. } | Self::Unreadable { edge, .. } => Some(edge),
            Self::InvalidImport { .. } | Self::NotFound { .. } | Self::SkippedExternal { .. } => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use djls_source::Span;

    use super::*;
    use crate::python::PythonSyntaxErrorClass;

    fn syntax_error(message: &str) -> PythonSyntaxError {
        PythonSyntaxError {
            class: PythonSyntaxErrorClass::Ordinary,
            span: Span::new(0, 0),
            message: message.to_string(),
        }
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
}
