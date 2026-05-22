use std::collections::BTreeMap;

use camino::Utf8PathBuf;

use crate::InstalledApp;
use crate::Interpreter;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichment {
    Absent,
    Disabled,
    Fresh(ProjectEnrichmentHints),
    CachedStale {
        hints: ProjectEnrichmentHints,
        issue: ProjectEnrichmentIssue,
    },
    Failed {
        issue: ProjectEnrichmentIssue,
    },
    Unavailable {
        issue: ProjectEnrichmentIssue,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectEnrichmentHints {
    runtime_template_dirs: Vec<Utf8PathBuf>,
    runtime_template_libraries: BTreeMap<String, String>,
    runtime_installed_apps: Vec<InstalledApp>,
    deep_extraction_hints: DeepExtractionHints,
}

impl ProjectEnrichmentHints {
    #[must_use]
    pub fn new(
        runtime_template_dirs: Vec<Utf8PathBuf>,
        runtime_template_libraries: BTreeMap<String, String>,
        runtime_installed_apps: Vec<InstalledApp>,
        deep_extraction_hints: DeepExtractionHints,
    ) -> Self {
        Self {
            runtime_template_dirs,
            runtime_template_libraries,
            runtime_installed_apps,
            deep_extraction_hints,
        }
    }

    #[must_use]
    pub fn runtime_template_dirs(&self) -> &[Utf8PathBuf] {
        &self.runtime_template_dirs
    }

    #[must_use]
    pub fn runtime_template_libraries(&self) -> &BTreeMap<String, String> {
        &self.runtime_template_libraries
    }

    #[must_use]
    pub fn runtime_installed_apps(&self) -> &[InstalledApp] {
        &self.runtime_installed_apps
    }

    #[must_use]
    pub fn deep_extraction_hints(&self) -> &DeepExtractionHints {
        &self.deep_extraction_hints
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeepExtractionHints {
    module_hints: BTreeMap<String, DeepExtractionModuleHint>,
}

impl DeepExtractionHints {
    #[must_use]
    pub fn module_hints(&self) -> &BTreeMap<String, DeepExtractionModuleHint> {
        &self.module_hints
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeepExtractionModuleHint {
    has_tags: bool,
    has_filters: bool,
    has_models: bool,
}

impl DeepExtractionModuleHint {
    #[must_use]
    pub fn new(has_tags: bool, has_filters: bool, has_models: bool) -> Self {
        Self {
            has_tags,
            has_filters,
            has_models,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichmentDraft {
    Disabled,
    Fresh(ProjectEnrichmentHints),
    CachedStale {
        hints: ProjectEnrichmentHints,
        issue: ProjectEnrichmentIssue,
    },
    Failed {
        issue: ProjectEnrichmentIssue,
    },
    Unavailable {
        issue: ProjectEnrichmentIssue,
    },
}

impl ProjectEnrichmentDraft {
    #[must_use]
    pub fn into_enrichment(self) -> ProjectEnrichment {
        match self {
            Self::Disabled => ProjectEnrichment::Disabled,
            Self::Fresh(hints) => ProjectEnrichment::Fresh(hints),
            Self::CachedStale { hints, issue } => ProjectEnrichment::CachedStale { hints, issue },
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
    CacheStale {
        key: EnrichmentCacheKey,
        age: CacheAge,
    },
    CacheReadFailed {
        kind: CacheIssueKind,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnrichmentCacheKey(String);

impl EnrichmentCacheKey {
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheAge {
    seconds: u64,
}

impl CacheAge {
    #[must_use]
    pub fn from_seconds(seconds: u64) -> Self {
        Self { seconds }
    }

    #[must_use]
    pub fn seconds(self) -> u64 {
        self.seconds
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheIssueKind {
    NotFound,
    Invalid,
    Io,
}

#[must_use]
pub fn merge_template_libraries(
    static_inventory: &crate::TemplateTagLibraryInventory,
    enrichment: &ProjectEnrichment,
) -> BTreeMap<String, String> {
    let mut libraries = static_inventory
        .libraries()
        .iter()
        .filter_map(|library| match library.resolution() {
            crate::TemplateTagLibraryResolution::Resolved { .. }
            | crate::TemplateTagLibraryResolution::Builtin => {
                Some((library.name().to_string(), library.name().to_string()))
            }
            crate::TemplateTagLibraryResolution::Unresolved { .. }
            | crate::TemplateTagLibraryResolution::Ambiguous { .. } => None,
        })
        .collect::<BTreeMap<_, _>>();

    let hints = match enrichment {
        ProjectEnrichment::Fresh(hints) | ProjectEnrichment::CachedStale { hints, .. } => hints,
        ProjectEnrichment::Absent
        | ProjectEnrichment::Disabled
        | ProjectEnrichment::Failed { .. }
        | ProjectEnrichment::Unavailable { .. } => return libraries,
    };
    for (name, module) in hints.runtime_template_libraries() {
        libraries
            .entry(name.clone())
            .or_insert_with(|| module.clone());
    }
    libraries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_draft_lowers_to_domain_state() {
        let issue = ProjectEnrichmentIssue::CacheReadFailed {
            kind: CacheIssueKind::NotFound,
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
    fn cache_age_records_seconds() {
        assert_eq!(CacheAge::from_seconds(42).seconds(), 42);
    }

    #[test]
    fn cache_as_hint_preserves_stale_runtime_values() {
        let hints = ProjectEnrichmentHints::new(
            vec!["/workspace/templates".into()],
            BTreeMap::from([("ui".to_string(), "blog.ui".to_string())]),
            Vec::new(),
            DeepExtractionHints::default(),
        );
        let issue = ProjectEnrichmentIssue::CacheStale {
            key: EnrichmentCacheKey::new("workspace"),
            age: CacheAge::from_seconds(3600),
        };

        let enrichment = ProjectEnrichmentDraft::CachedStale {
            hints: hints.clone(),
            issue: issue.clone(),
        }
        .into_enrichment();

        assert_eq!(enrichment, ProjectEnrichment::CachedStale { hints, issue });
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
