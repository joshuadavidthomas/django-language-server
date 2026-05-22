#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichment {
    Absent,
    Disabled,
    Unavailable { issues: ProjectEnrichmentIssues },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEnrichmentIssues(Vec<ProjectEnrichmentIssue>);

impl ProjectEnrichmentIssues {
    pub fn new(issues: Vec<ProjectEnrichmentIssue>) -> Result<Self, EmptyProjectEnrichmentIssues> {
        if issues.is_empty() {
            return Err(EmptyProjectEnrichmentIssues);
        }
        Ok(Self(issues))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[ProjectEnrichmentIssue] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyProjectEnrichmentIssues;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichmentIssue {
    FixtureDoesNotModelEnrichment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_initial_states_do_not_model_core_startup_readiness() {
        assert_eq!(ProjectEnrichment::Absent, ProjectEnrichment::Absent);
        assert_eq!(ProjectEnrichment::Disabled, ProjectEnrichment::Disabled);
        assert_eq!(
            ProjectEnrichment::Unavailable {
                issues: ProjectEnrichmentIssues::new(vec![
                    ProjectEnrichmentIssue::FixtureDoesNotModelEnrichment,
                ])
                .expect("unavailable enrichment needs an issue"),
            },
            ProjectEnrichment::Unavailable {
                issues: ProjectEnrichmentIssues::new(vec![
                    ProjectEnrichmentIssue::FixtureDoesNotModelEnrichment,
                ])
                .expect("unavailable enrichment needs an issue"),
            }
        );
    }

    #[test]
    fn unavailable_enrichment_requires_at_least_one_issue() {
        assert_eq!(
            ProjectEnrichmentIssues::new(Vec::new()),
            Err(EmptyProjectEnrichmentIssues)
        );
    }
}
