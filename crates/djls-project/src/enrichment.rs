use std::collections::BTreeMap;

mod runtime;

pub use crate::enrichment::runtime::load_runtime_project_enrichment;
use crate::names::LibraryName;
use crate::names::PyModuleName;
use crate::Interpreter;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichment {
    Absent,
    Disabled,
    Fresh(RuntimeTemplateLibraries),
    Unresolved(ProjectEnrichmentIssue),
}

pub type RuntimeTemplateLibraries = BTreeMap<LibraryName, PyModuleName>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichmentIssue {
    RuntimeUnavailable {
        interpreter: Option<Interpreter>,
        kind: RuntimeUnavailableKind,
    },
    InspectorFailed(InspectorFailureKind),
    FixtureDoesNotModelEnrichment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeUnavailableKind {
    MissingPython,
    DjangoImportFailed,
    EnvironmentNotConfigured,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InspectorFailureKind {
    SubprocessFailed { status: Option<i32> },
    InvalidJson,
    TimedOut,
}
