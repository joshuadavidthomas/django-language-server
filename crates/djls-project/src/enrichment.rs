use std::collections::BTreeMap;

mod runtime;

pub use crate::enrichment::runtime::load_runtime_project_enrichment;
use crate::Interpreter;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichment {
    Absent,
    Disabled,
    Fresh(ProjectEnrichmentHints),
    Failed { issue: ProjectEnrichmentIssue },
    Unavailable { issue: ProjectEnrichmentIssue },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectEnrichmentHints {
    template_libraries: BTreeMap<String, String>,
}

impl ProjectEnrichmentHints {
    #[must_use]
    pub fn new(runtime_template_libraries: BTreeMap<String, String>) -> Self {
        Self {
            template_libraries: runtime_template_libraries,
        }
    }

    #[must_use]
    pub fn runtime_template_libraries(&self) -> &BTreeMap<String, String> {
        &self.template_libraries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichmentDraft {
    Disabled,
    Fresh(ProjectEnrichmentHints),
    Failed { issue: ProjectEnrichmentIssue },
    Unavailable { issue: ProjectEnrichmentIssue },
}

impl ProjectEnrichmentDraft {
    #[must_use]
    pub fn into_enrichment(self) -> ProjectEnrichment {
        match self {
            Self::Disabled => ProjectEnrichment::Disabled,
            Self::Fresh(hints) => ProjectEnrichment::Fresh(hints),
            Self::Failed { issue } => ProjectEnrichment::Failed { issue },
            Self::Unavailable { issue } => ProjectEnrichment::Unavailable { issue },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichmentIssue {
    RuntimeUnavailable {
        interpreter: Option<Interpreter>,
        kind: RuntimeUnavailableKind,
    },
    InspectorFailed {
        kind: InspectorFailureKind,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_draft_lowers_to_domain_state() {
        let issue = ProjectEnrichmentIssue::InspectorFailed {
            kind: InspectorFailureKind::InvalidJson,
        };
        assert_eq!(
            ProjectEnrichmentDraft::Failed {
                issue: issue.clone()
            }
            .into_enrichment(),
            ProjectEnrichment::Failed { issue }
        );
    }

    #[test]
    fn enrichment_compat_keeps_absent_disabled_and_unavailable_states() {
        assert_eq!(ProjectEnrichment::Absent, ProjectEnrichment::Absent);
        assert_eq!(ProjectEnrichment::Disabled, ProjectEnrichment::Disabled);
        assert_eq!(
            ProjectEnrichmentDraft::Unavailable {
                issue: ProjectEnrichmentIssue::FixtureDoesNotModelEnrichment,
            }
            .into_enrichment(),
            ProjectEnrichment::Unavailable {
                issue: ProjectEnrichmentIssue::FixtureDoesNotModelEnrichment,
            }
        );
    }
}
