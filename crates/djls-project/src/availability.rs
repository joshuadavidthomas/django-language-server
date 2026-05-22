use crate::Db;
use crate::ProjectDiscovery;
use crate::ProjectSourceFilesIssue;
use crate::ProjectSourceInventory;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectFactsAvailability {
    pub source_files: ProjectFactStatus<ProjectSourceFilesIssue>,
    pub discovery: ProjectFactStatus<ProjectDiscoveryUnavailableReason>,
}

impl ProjectFactsAvailability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self.source_files, ProjectFactStatus::Available)
            && matches!(self.discovery, ProjectFactStatus::Available)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectFactStatus<Issue> {
    Available,
    Unavailable { issue: Issue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDiscoveryUnavailableReason {
    NotLoaded,
    Failed {
        issues: crate::ProjectDiscoveryIssues,
    },
}

#[must_use]
pub fn project_facts_availability(db: &dyn Db) -> ProjectFactsAvailability {
    let project = db.project();
    let source_files = match project.source_inventory(db) {
        ProjectSourceInventory::Ready(_) => ProjectFactStatus::Available,
        ProjectSourceInventory::Unavailable { issue } => ProjectFactStatus::Unavailable { issue },
    };
    let discovery = match project.discovery(db) {
        ProjectDiscovery::Ready(_) => ProjectFactStatus::Available,
        ProjectDiscovery::Absent => ProjectFactStatus::Unavailable {
            issue: ProjectDiscoveryUnavailableReason::NotLoaded,
        },
        ProjectDiscovery::Unavailable { issues } => ProjectFactStatus::Unavailable {
            issue: ProjectDiscoveryUnavailableReason::Failed {
                issues: issues.clone(),
            },
        },
    };

    ProjectFactsAvailability {
        source_files,
        discovery,
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::SourceFiles;
    use salsa::Setter;

    use super::*;
    use crate::Project;
    use crate::ProjectDiscoveryIssue;
    use crate::ProjectDiscoveryIssues;
    use crate::ProjectDiscoverySet;
    use crate::ProjectEnvVars;
    use crate::RootDiscoveryInput;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        project: Option<Project>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn project(&self) -> Project {
            self.project.expect("test project should be initialized")
        }
    }

    impl TestDb {
        fn with_project() -> Self {
            let mut db = Self::default();
            db.project = Some(Project::virtual_project(&db));
            db
        }
    }

    #[test]
    fn availability_reports_absent_virtual_project_facts() {
        let db = TestDb::with_project();

        assert_eq!(
            project_facts_availability(&db),
            ProjectFactsAvailability {
                source_files: ProjectFactStatus::Unavailable {
                    issue: ProjectSourceFilesIssue::NotLoaded,
                },
                discovery: ProjectFactStatus::Unavailable {
                    issue: ProjectDiscoveryUnavailableReason::NotLoaded,
                },
            }
        );
    }

    #[test]
    fn availability_reports_unavailable_discovery_issues() {
        let mut db = TestDb::with_project();
        let issues =
            ProjectDiscoveryIssues::new(vec![ProjectDiscoveryIssue::FixtureDoesNotModelDiscovery])
                .expect("test issues should be non-empty");
        db.project()
            .set_discovery(&mut db)
            .to(ProjectDiscovery::Unavailable { issues });

        assert_eq!(
            project_facts_availability(&db).discovery,
            ProjectFactStatus::Unavailable {
                issue: ProjectDiscoveryUnavailableReason::Failed {
                    issues: ProjectDiscoveryIssues::new(vec![
                        ProjectDiscoveryIssue::FixtureDoesNotModelDiscovery,
                    ])
                    .expect("test issues should be non-empty"),
                },
            }
        );
    }

    #[test]
    fn availability_reports_available_discovery() {
        let mut db = TestDb::with_project();
        let root = RootDiscoveryInput::new(
            &db,
            camino::Utf8PathBuf::from("/workspace"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        let discovery = ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("test discovery set should be non-empty"),
        );
        db.project().set_discovery(&mut db).to(discovery);
        assert!(matches!(
            project_facts_availability(&db).discovery,
            ProjectFactStatus::Available
        ));
    }
}
