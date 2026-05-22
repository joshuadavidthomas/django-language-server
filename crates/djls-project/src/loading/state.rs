use camino::Utf8PathBuf;
use djls_source::FileSetSummary;
use djls_source::SourceFileSet;
use djls_source::SourceRootId;

use super::files::MergedDiscoveredSourceFileSetData;
use super::files::ProjectFileSetPartitions;

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
                previous: None,
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
                previous: None,
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
    Ready(ReadyProjectSourceFiles),
    Deferred {
        issue: ProjectSourceFilesIssue,
        previous: Option<ReadyProjectSourceFiles>,
    },
    Unavailable {
        issue: ProjectSourceFilesIssue,
        previous: Option<ReadyProjectSourceFiles>,
    },
    Failed {
        issue: ProjectSourceFilesIssue,
        previous: Option<ReadyProjectSourceFiles>,
    },
    Stale {
        previous: ReadyProjectSourceFiles,
    },
}

impl ProjectSourceFilesAvailability {
    #[must_use]
    pub fn ready_or_previous(&self) -> Option<ReadyProjectSourceFiles> {
        match self {
            Self::Ready(files) | Self::Stale { previous: files } => Some(files.clone()),
            Self::Deferred { previous, .. }
            | Self::Unavailable { previous, .. }
            | Self::Failed { previous, .. } => previous.clone(),
            Self::Loading => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyProjectSourceFiles {
    pub(crate) partitions: ProjectFileSetPartitions,
    merged: SourceFileSet,
}

impl ReadyProjectSourceFiles {
    #[must_use]
    pub(crate) fn new(partitions: ProjectFileSetPartitions, merged: SourceFileSet) -> Self {
        Self { partitions, merged }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn materialized_for_test(
        partitions: ProjectFileSetPartitions,
        merged: SourceFileSet,
    ) -> Self {
        Self::new(partitions, merged)
    }

    #[must_use]
    pub fn merged(&self) -> SourceFileSet {
        self.merged
    }

    #[must_use]
    pub(crate) fn discovered_data(&self) -> MergedDiscoveredSourceFileSetData {
        self.partitions.merged_discovered_data()
    }

    #[must_use]
    pub fn summary(&self, db: &dyn djls_source::Db) -> FileSetSummary {
        *self.merged.data(db).summary()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProjectSourceFilesBuildState {
    #[allow(dead_code)]
    Discovered(ProjectSourceFilesDiscovered),
    #[allow(dead_code)]
    Materialized(ReadyProjectSourceFiles),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectSourceFilesDiscovered {
    partitions: ProjectFileSetPartitions,
}

impl ProjectSourceFilesBuildState {
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn discovered_data(&self) -> MergedDiscoveredSourceFileSetData {
        match self {
            Self::Discovered(files) => files.partitions.merged_discovered_data(),
            Self::Materialized(files) => files.discovered_data(),
        }
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
    PartitionConflict {
        path: Utf8PathBuf,
        winner: super::FileSetPartitionId,
        shadowed: super::FileSetPartitionId,
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
                previous: None,
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

    fn ready_source_files(db: &TestDb) -> ReadyProjectSourceFiles {
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path.clone(), FileRootKind::Project);
        let file_path = root_path.join("templates/index.html");
        let file = File::new(db, file_path.clone(), 0);
        let loaded = LoadedSourceFile::new(file_path, root_id, file);
        let data = SourceFileSetData::new(vec![SourceRootEntry::new(root)], vec![loaded])
            .expect("source file set should be coherent");
        let set = SourceFileSet::new(db, data);
        ReadyProjectSourceFiles::materialized_for_test(ProjectFileSetPartitions::empty(), set)
    }

    #[test]
    fn loading_state_not_loaded_is_production_safe() {
        let db = TestDb::default();
        let state = ProjectLoadingState::not_loaded(&db);

        assert_eq!(
            state.source_files(&db),
            ProjectSourceFilesAvailability::Unavailable {
                issue: ProjectSourceFilesIssue::NotLoaded,
                previous: None,
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

    #[salsa::tracked]
    fn source_file_count_probe(db: &dyn crate::Db) -> Option<usize> {
        match db.project_loading_state().source_files(db) {
            ProjectSourceFilesAvailability::Ready(files) => {
                Some(files.summary(db).included_files())
            }
            _ => None,
        }
    }

    #[test]
    fn loading_state_invalidation_source_files_transition_invalidates_probe_query() {
        let mut db = TestDb::new_with_loading_state();
        assert_eq!(source_file_count_probe(&db), None);

        db.set_project_source_files_availability(ProjectSourceFilesAvailability::Ready(
            ready_source_files(&db),
        ));

        assert_eq!(source_file_count_probe(&db), Some(1));
    }

    #[test]
    fn begin_loading_run_without_previous_source_files_sets_loading() {
        let mut db = TestDb::new_with_loading_state();

        db.begin_project_loading_run();

        assert_eq!(
            db.project_loading_state().source_files(&db),
            ProjectSourceFilesAvailability::Loading
        );
    }

    #[test]
    fn begin_loading_run_preserves_ready_source_files_as_stale() {
        let mut db = TestDb::new_with_loading_state();
        let files = ready_source_files(&db);
        db.set_project_source_files_availability(ProjectSourceFilesAvailability::Ready(
            files.clone(),
        ));

        db.begin_project_loading_run();

        assert_eq!(
            db.project_loading_state().source_files(&db),
            ProjectSourceFilesAvailability::Stale { previous: files }
        );
    }

    #[test]
    fn ready_project_source_files_summary_comes_from_merged_file_set() {
        let db = TestDb::default();
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path.clone(), FileRootKind::Project);
        let file_path = root_path.join("templates/index.html");
        let file = File::new(&db, file_path.clone(), 0);
        let loaded = LoadedSourceFile::new(file_path, root_id, file);
        let data = SourceFileSetData::new(vec![SourceRootEntry::new(root.clone())], vec![loaded])
            .expect("source file set should be coherent");
        let set = SourceFileSet::new(&db, data);

        let files =
            ReadyProjectSourceFiles::materialized_for_test(ProjectFileSetPartitions::empty(), set);

        assert_eq!(files.summary(&db), FileSetSummary::new(1));
    }
}
