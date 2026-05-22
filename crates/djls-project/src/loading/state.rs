use camino::Utf8PathBuf;
use djls_source::FileSetSummary;
use djls_source::SourceFileSet;
use djls_source::SourceRootId;

use super::files::MergedDiscoveredSourceFileSetData;
use super::files::ProjectFileSetPartitions;
use crate::ProjectDiscovery;
use crate::ProjectDiscoveryIssue;
use crate::ProjectDiscoveryIssues;
use crate::ProjectEnrichment;

#[salsa::input]
#[derive(Debug)]
pub struct Project {
    pub source_inventory: ProjectSourceInventory,
    #[returns(ref)]
    pub discovery: ProjectDiscovery,
    #[returns(ref)]
    pub enrichment: ProjectEnrichment,
}

impl Project {
    pub fn virtual_project(db: &dyn crate::Db) -> Self {
        Self::new(
            db,
            ProjectSourceInventory::Unavailable {
                issue: ProjectSourceFilesIssue::NotLoaded,
            },
            ProjectDiscovery::Absent,
            ProjectEnrichment::Absent,
        )
    }

    pub fn fixture_unavailable(db: &dyn crate::Db) -> Self {
        Self::new(
            db,
            ProjectSourceInventory::Unavailable {
                issue: ProjectSourceFilesIssue::FixtureUnavailable {
                    surface: ProjectSourceFilesFixtureSurface::SourceFiles,
                },
            },
            ProjectDiscovery::Unavailable {
                issues: ProjectDiscoveryIssues::new(vec![
                    ProjectDiscoveryIssue::FixtureDoesNotModelDiscovery,
                ])
                .expect("fixture discovery issue should be non-empty"),
            },
            ProjectEnrichment::Absent,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectSourceInventory {
    Ready(ReadyProjectSourceFiles),
    Unavailable { issue: ProjectSourceFilesIssue },
}

impl ProjectSourceInventory {
    #[must_use]
    pub fn ready(&self) -> Option<ReadyProjectSourceFiles> {
        match self {
            Self::Ready(files) => Some(files.clone()),
            Self::Unavailable { .. } => None,
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
        project: std::sync::Mutex<Option<Project>>,
    }

    impl TestDb {
        fn new_with_project() -> Self {
            let db = Self::default();
            let project = Project::fixture_unavailable(&db);
            *db.project.lock().unwrap() = Some(project);
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
        fn project(&self) -> Project {
            self.project
                .lock()
                .unwrap()
                .expect("test database should initialize project")
        }
    }

    #[test]
    fn project_fixture_starts_with_generation_free_unavailable_source_inventory() {
        let db = TestDb::new_with_project();
        let project = db.project();

        assert_eq!(
            project.source_inventory(&db),
            ProjectSourceInventory::Unavailable {
                issue: ProjectSourceFilesIssue::FixtureUnavailable {
                    surface: ProjectSourceFilesFixtureSurface::SourceFiles,
                },
            }
        );
        assert_eq!(
            *project.discovery(&db),
            ProjectDiscovery::Unavailable {
                issues: ProjectDiscoveryIssues::new(vec![
                    ProjectDiscoveryIssue::FixtureDoesNotModelDiscovery,
                ])
                .expect("fixture discovery issue should be non-empty"),
            }
        );
        assert_eq!(*project.enrichment(&db), ProjectEnrichment::Absent);
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
    fn virtual_project_is_production_safe() {
        let db = TestDb::default();
        let project = Project::virtual_project(&db);

        assert_eq!(
            project.source_inventory(&db),
            ProjectSourceInventory::Unavailable {
                issue: ProjectSourceFilesIssue::NotLoaded,
            }
        );
        assert_eq!(*project.discovery(&db), ProjectDiscovery::Absent);
        assert_eq!(*project.enrichment(&db), ProjectEnrichment::Absent);
    }

    #[salsa::tracked]
    fn source_file_count_probe(db: &dyn crate::Db) -> Option<usize> {
        match db.project().source_inventory(db) {
            ProjectSourceInventory::Ready(files) => Some(files.summary(db).included_files()),
            ProjectSourceInventory::Unavailable { .. } => None,
        }
    }

    #[test]
    fn source_inventory_transition_invalidates_probe_query() {
        let mut db = TestDb::new_with_project();
        assert_eq!(source_file_count_probe(&db), None);

        db.set_project_source_inventory(ProjectSourceInventory::Ready(ready_source_files(&db)));

        assert_eq!(source_file_count_probe(&db), Some(1));
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
