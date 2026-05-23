use crate::enrichment::ProjectEnrichment;
use crate::root_discovery::ProjectRootDiscovery;
use crate::root_discovery::ProjectRootDiscoveryIssue;
use crate::root_discovery::ProjectRootDiscoveryIssues;
use crate::source_files::SourceFileInventory;
use crate::source_files::SourceFilesFixtureSurface;
use crate::source_files::SourceFilesIssue;

#[salsa::input]
#[derive(Debug)]
pub struct Project {
    pub source_inventory: SourceFileInventory,
    #[returns(ref)]
    pub root_discovery: ProjectRootDiscovery,
    #[returns(ref)]
    pub enrichment: ProjectEnrichment,
}

impl Project {
    pub fn virtual_project(db: &dyn crate::Db) -> Self {
        Self::new(
            db,
            SourceFileInventory::Unavailable {
                issue: SourceFilesIssue::NotLoaded,
            },
            ProjectRootDiscovery::Absent,
            ProjectEnrichment::Absent,
        )
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn fixture_unavailable(db: &dyn crate::Db) -> Self {
        Self::new(
            db,
            SourceFileInventory::Unavailable {
                issue: SourceFilesIssue::FixtureUnavailable {
                    surface: SourceFilesFixtureSurface::SourceFiles,
                },
            },
            ProjectRootDiscovery::Unavailable {
                issues: ProjectRootDiscoveryIssues::new(vec![
                    ProjectRootDiscoveryIssue::FixtureDoesNotModelDiscovery,
                ])
                .expect("fixture discovery issue should be non-empty"),
            },
            ProjectEnrichment::Absent,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_source::FileRootKind;
    use djls_source::LoadedSourceFile;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_source::SourceRoot;
    use djls_source::SourceRootEntry;
    use djls_source::SourceRootId;

    use super::*;
    use crate::source_files::ReadySourceFiles;
    use crate::source_files::SourceFileSetPartitions;
    use crate::Db as ProjectDb;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        project: Mutex<Option<Project>>,
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

        fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
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
            SourceFileInventory::Unavailable {
                issue: SourceFilesIssue::FixtureUnavailable {
                    surface: SourceFilesFixtureSurface::SourceFiles,
                },
            }
        );
        assert_eq!(
            *project.root_discovery(&db),
            ProjectRootDiscovery::Unavailable {
                issues: ProjectRootDiscoveryIssues::new(vec![
                    ProjectRootDiscoveryIssue::FixtureDoesNotModelDiscovery,
                ])
                .expect("fixture discovery issue should be non-empty"),
            }
        );
        assert_eq!(*project.enrichment(&db), ProjectEnrichment::Absent);
    }

    fn ready_source_files(db: &TestDb) -> ReadySourceFiles {
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path.clone(), FileRootKind::Project);
        let file_path = root_path.join("templates/index.html");
        let file = File::new(db, file_path.clone(), 0);
        let loaded = LoadedSourceFile::new(file_path, root_id, file);
        let data = SourceFileSetData::new(vec![SourceRootEntry::new(root)], vec![loaded])
            .expect("source file set should be coherent");
        let set = SourceFileSet::new(db, data);
        ReadySourceFiles::materialized_for_test(SourceFileSetPartitions::default(), set)
    }

    #[test]
    fn virtual_project_is_production_safe() {
        let db = TestDb::default();
        let project = Project::virtual_project(&db);

        assert_eq!(
            project.source_inventory(&db),
            SourceFileInventory::Unavailable {
                issue: SourceFilesIssue::NotLoaded,
            }
        );
        assert_eq!(*project.root_discovery(&db), ProjectRootDiscovery::Absent);
        assert_eq!(*project.enrichment(&db), ProjectEnrichment::Absent);
    }

    #[salsa::tracked]
    fn source_file_count_probe(db: &dyn crate::Db) -> Option<usize> {
        match db.project().source_inventory(db) {
            SourceFileInventory::Ready(files) => Some(files.summary(db).included_files()),
            SourceFileInventory::Unavailable { .. } => None,
        }
    }

    #[test]
    fn source_inventory_transition_invalidates_probe_query() {
        let mut db = TestDb::new_with_project();
        assert_eq!(source_file_count_probe(&db), None);

        db.set_source_file_inventory(SourceFileInventory::Ready(ready_source_files(&db)));

        assert_eq!(source_file_count_probe(&db), Some(1));
    }

    #[test]
    fn ready_source_files_summary_comes_from_merged_file_set() {
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

        let files =
            ReadySourceFiles::materialized_for_test(SourceFileSetPartitions::default(), set);

        assert_eq!(files.summary(&db), djls_source::FileSetSummary::new(1));
    }
}
