use camino::Utf8PathBuf;
use djls_source::FileSetSummary;
use djls_source::SourceFileSet;
use djls_source::SourceRootId;

#[salsa::input]
#[derive(Debug)]
pub struct ProjectLoadingState {
    pub source_files: ProjectSourceFilesAvailability,
    pub discovery: ProjectDiscoveryAvailability,
    pub enrichment: ProjectEnrichmentState,
}

impl ProjectLoadingState {
    pub fn not_loaded(db: &dyn crate::Db) -> Self {
        Self::new(
            db,
            ProjectSourceFilesAvailability::Unavailable {
                issue: ProjectSourceFilesIssue::NotLoaded,
            },
            ProjectDiscoveryAvailability::Unavailable {
                issue: ProjectDiscoveryIssue::NotLoaded,
            },
            ProjectEnrichmentState::NotStarted,
        )
    }

    pub fn fixture_unavailable(db: &dyn crate::Db) -> Self {
        Self::new(
            db,
            ProjectSourceFilesAvailability::Unavailable {
                issue: ProjectSourceFilesIssue::FixtureUnavailable {
                    surface: ProjectSourceFilesFixtureSurface::SourceFiles,
                },
            },
            ProjectDiscoveryAvailability::Unavailable {
                issue: ProjectDiscoveryIssue::FixtureDoesNotModelDiscovery,
            },
            ProjectEnrichmentState::NotStarted,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectSourceFilesAvailability {
    Loading,
    Ready(ProjectSourceFiles),
    Unavailable { issue: ProjectSourceFilesIssue },
    Stale { previous: ProjectSourceFiles },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSourceFiles {
    merged: SourceFileSet,
}

impl ProjectSourceFiles {
    #[must_use]
    pub fn new(merged: SourceFileSet) -> Self {
        Self { merged }
    }

    #[must_use]
    pub fn merged(&self) -> SourceFileSet {
        self.merged
    }

    #[must_use]
    pub fn summary(&self, db: &dyn djls_source::Db) -> FileSetSummary {
        *self.merged.data(db).summary()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectSourceFilesIssue {
    NotLoaded,
    MissingRoot {
        root: SourceRootId,
        path: Utf8PathBuf,
    },
    DuplicateRoot {
        root: SourceRootId,
        duplicate_path: Utf8PathBuf,
    },
    WalkFailed {
        root: SourceRootId,
        path: Utf8PathBuf,
        error_kind: std::io::ErrorKind,
    },
    FixtureUnavailable {
        surface: ProjectSourceFilesFixtureSurface,
    },
    StaleDocument {
        path: Utf8PathBuf,
    },
    MaterializationFailed {
        path: Utf8PathBuf,
        error_kind: std::io::ErrorKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectSourceFilesFixtureSurface {
    SourceFiles,
    Partitions,
    Materialization,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDiscoveryAvailability {
    Unavailable { issue: ProjectDiscoveryIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDiscoveryIssue {
    NotLoaded,
    FixtureDoesNotModelDiscovery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichmentState {
    NotStarted,
    Unavailable { issue: ProjectEnrichmentIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichmentIssue {
    FixtureDoesNotModelEnrichment,
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_source::FileRootKind;
    use djls_source::LoadedSourceFile;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_source::SourceRoot;
    use djls_source::SourceRootEntry;

    use super::*;
    use crate::Db as ProjectDb;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        loading_state: std::sync::Mutex<Option<ProjectLoadingState>>,
    }

    impl TestDb {
        fn new_with_loading_state() -> Self {
            let db = Self::default();
            let state = ProjectLoadingState::fixture_unavailable(&db);
            *db.loading_state.lock().unwrap() = Some(state);
            db
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl SourceDb for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, _path: &camino::Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn project_loading_state(&self) -> ProjectLoadingState {
            self.loading_state
                .lock()
                .unwrap()
                .expect("test database should initialize project loading state")
        }
    }

    #[test]
    fn loading_state_fixture_starts_with_generation_free_unavailable_source_files() {
        let db = TestDb::new_with_loading_state();
        let state = db.project_loading_state();

        assert_eq!(
            state.source_files(&db),
            ProjectSourceFilesAvailability::Unavailable {
                issue: ProjectSourceFilesIssue::FixtureUnavailable {
                    surface: ProjectSourceFilesFixtureSurface::SourceFiles,
                },
            }
        );
        assert_eq!(
            state.discovery(&db),
            ProjectDiscoveryAvailability::Unavailable {
                issue: ProjectDiscoveryIssue::FixtureDoesNotModelDiscovery,
            }
        );
        assert_eq!(state.enrichment(&db), ProjectEnrichmentState::NotStarted);
    }

    #[test]
    fn loading_state_not_loaded_is_production_safe() {
        let db = TestDb::default();
        let state = ProjectLoadingState::not_loaded(&db);

        assert_eq!(
            state.source_files(&db),
            ProjectSourceFilesAvailability::Unavailable {
                issue: ProjectSourceFilesIssue::NotLoaded,
            }
        );
        assert_eq!(
            state.discovery(&db),
            ProjectDiscoveryAvailability::Unavailable {
                issue: ProjectDiscoveryIssue::NotLoaded,
            }
        );
        assert_eq!(state.enrichment(&db), ProjectEnrichmentState::NotStarted);
    }

    #[test]
    fn project_source_files_summary_comes_from_merged_file_set() {
        let db = TestDb::default();
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path.clone(), FileRootKind::Project);
        let file_path = root_path.join("templates/index.html");
        let file = File::new(&db, file_path.clone(), 0);
        let loaded = LoadedSourceFile::new(file_path, root_id, file);
        let data = SourceFileSetData::new(vec![SourceRootEntry::new(root)], vec![loaded])
            .expect("source file set should be coherent");
        let set = SourceFileSet::new(&db, data);

        let files = ProjectSourceFiles::new(set);

        assert_eq!(files.summary(&db), FileSetSummary::new(1));
    }
}
