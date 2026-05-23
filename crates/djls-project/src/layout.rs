use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use rustc_hash::FxHashMap;

use crate::Db;
use crate::Project;
use crate::ProjectSourceFilesIssue;
use crate::ProjectSourceInventory;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectLayoutIndexOutcome {
    Ready(ProjectLayoutIndex),
    Absent { issue: ProjectLayoutIssue },
    Unavailable { issue: ProjectLayoutIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectLayoutIssue {
    SourceInventoryAbsent,
    SourceInventoryUnavailable { issue: ProjectSourceFilesIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsModuleCandidatesOutcome {
    Ready(Vec<String>),
    LayoutUnavailable { issue: ProjectLayoutIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLayoutIndex {
    roots: Vec<Utf8PathBuf>,
    files: Vec<LayoutFile>,
    file_by_path: FxHashMap<Utf8PathBuf, File>,
}

impl ProjectLayoutIndex {
    fn new(data: &djls_source::SourceFileSetData) -> Self {
        let mut roots = data
            .roots()
            .iter()
            .map(|entry| entry.root().path().to_owned())
            .collect::<Vec<_>>();
        roots.sort();
        let mut files = data
            .files()
            .iter()
            .map(|file| LayoutFile {
                path: file.path().to_owned(),
                file: file.file(),
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let file_by_path = files
            .iter()
            .map(|file| (file.path.clone(), file.file))
            .collect();

        Self {
            roots,
            files,
            file_by_path,
        }
    }

    #[must_use]
    pub fn file_path(&self, file: File) -> Option<&Utf8Path> {
        self.files
            .iter()
            .find(|entry| entry.file == file)
            .map(|entry| entry.path.as_path())
    }

    #[must_use]
    pub fn file_for_path(&self, path: &Utf8Path) -> Option<File> {
        self.file_by_path.get(path).copied()
    }

    #[must_use]
    pub fn files_by_name(&self, name: &str) -> Vec<File> {
        self.files
            .iter()
            .filter(|entry| entry.path.file_name() == Some(name))
            .map(|entry| entry.file)
            .collect()
    }

    #[must_use]
    pub fn module_name_for_path(&self, path: &Utf8Path) -> Option<crate::PyModuleName> {
        let root = self
            .roots
            .iter()
            .filter(|root| path.starts_with(root.as_path()))
            .max_by_key(|root| root.as_str().len())?;
        let relative = path.strip_prefix(root).ok()?.with_extension("");
        let components = relative
            .components()
            .map(|component| component.as_str())
            .collect::<Vec<_>>();
        if components.is_empty() {
            return None;
        }
        crate::PyModuleName::parse(&components.join(".")).ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LayoutFile {
    path: Utf8PathBuf,
    file: File,
}

#[salsa::tracked(returns(ref))]
pub fn project_layout_index(db: &dyn Db, project: Project) -> ProjectLayoutIndexOutcome {
    match project.source_inventory(db) {
        ProjectSourceInventory::Ready(files) => {
            let data = files.merged().data(db);
            ProjectLayoutIndexOutcome::Ready(ProjectLayoutIndex::new(data))
        }
        ProjectSourceInventory::Unavailable {
            issue: ProjectSourceFilesIssue::NotLoaded,
        } => ProjectLayoutIndexOutcome::Absent {
            issue: ProjectLayoutIssue::SourceInventoryAbsent,
        },
        ProjectSourceInventory::Unavailable { issue } => ProjectLayoutIndexOutcome::Unavailable {
            issue: ProjectLayoutIssue::SourceInventoryUnavailable { issue },
        },
    }
}

#[salsa::tracked(returns(ref))]
pub fn settings_module_candidates(
    db: &dyn Db,
    project: Project,
) -> SettingsModuleCandidatesOutcome {
    let layout = match project_layout_index(db, project) {
        ProjectLayoutIndexOutcome::Ready(layout) => layout,
        ProjectLayoutIndexOutcome::Absent { issue }
        | ProjectLayoutIndexOutcome::Unavailable { issue } => {
            return SettingsModuleCandidatesOutcome::LayoutUnavailable {
                issue: issue.clone(),
            };
        }
    };

    let mut candidates = layout
        .files_by_name("settings.py")
        .into_iter()
        .filter_map(|file| layout.file_path(file))
        .filter_map(|path| layout.module_name_for_path(path))
        .map(|module| module.as_str().to_string())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    SettingsModuleCandidatesOutcome::Ready(candidates)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::FileRootKind;
    use djls_source::LoadedSourceFile;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_source::SourceRoot;
    use djls_source::SourceRootEntry;
    use djls_source::SourceRootId;
    use salsa::Database;
    use salsa::Setter;

    use super::*;
    use crate::loading::state::ProjectSourceFilesFixtureSurface;
    use crate::ProjectEnrichment;
    use crate::ReadyProjectSourceFiles;

    #[salsa::db]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        project: Option<Project>,
        events: Arc<Mutex<Vec<salsa::Event>>>,
    }

    impl Default for TestDb {
        fn default() -> Self {
            let events = Arc::new(Mutex::new(Vec::new()));
            let storage = salsa::Storage::new(Some(Box::new({
                let events = Arc::clone(&events);
                move |event| {
                    events
                        .lock()
                        .expect("event log is not poisoned")
                        .push(event);
                }
            })));
            let mut db = Self {
                storage,
                files: SourceFiles::default(),
                project: None,
                events,
            };
            db.project = Some(Project::virtual_project(&db));
            db
        }
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
        fn take_events(&self) -> Vec<salsa::Event> {
            std::mem::take(&mut *self.events.lock().expect("event log is not poisoned"))
        }

        fn tracked_query_executed(&self, events: &[salsa::Event], query_name: &str) -> bool {
            events.iter().any(|event| match &event.kind {
                salsa::EventKind::WillExecute { database_key } => self
                    .ingredient_debug_name(database_key.ingredient_index())
                    .contains(query_name),
                _ => false,
            })
        }
    }

    fn ready_inventory(db: &TestDb, paths: &[&str]) -> ProjectSourceInventory {
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path, FileRootKind::Project);
        let roots = vec![SourceRootEntry::new(root)];
        let files = paths
            .iter()
            .map(|path| {
                let path = Utf8PathBuf::from(path);
                LoadedSourceFile::new(path.clone(), root_id.clone(), File::new(db, path, 0))
            })
            .collect::<Vec<_>>();
        let data = SourceFileSetData::new(roots, files).expect("test data should be valid");
        let set = SourceFileSet::new(db, data);
        ProjectSourceInventory::Ready(ReadyProjectSourceFiles::merged_for_test(set))
    }

    #[test]
    fn layout_reports_absent_and_unavailable_source_inventory() {
        let mut db = TestDb::default();

        assert_eq!(
            project_layout_index(&db, db.project()).clone(),
            ProjectLayoutIndexOutcome::Absent {
                issue: ProjectLayoutIssue::SourceInventoryAbsent,
            }
        );

        db.set_project_source_inventory(ProjectSourceInventory::Unavailable {
            issue: ProjectSourceFilesIssue::FixtureUnavailable {
                surface: ProjectSourceFilesFixtureSurface::SourceFiles,
            },
        });

        assert_eq!(
            project_layout_index(&db, db.project()).clone(),
            ProjectLayoutIndexOutcome::Unavailable {
                issue: ProjectLayoutIssue::SourceInventoryUnavailable {
                    issue: ProjectSourceFilesIssue::FixtureUnavailable {
                        surface: ProjectSourceFilesFixtureSurface::SourceFiles,
                    },
                },
            }
        );
    }

    #[test]
    fn ready_layout_indexes_paths_names_extensions_children_and_packages() {
        let mut db = TestDb::default();
        db.set_project_source_inventory(ready_inventory(
            &db,
            &[
                "/workspace/app/__init__.py",
                "/workspace/app/models.py",
                "/workspace/app/templates/index.html",
                "/workspace/project/settings.py",
            ],
        ));

        let ProjectLayoutIndexOutcome::Ready(index) = project_layout_index(&db, db.project())
        else {
            panic!("layout should be ready");
        };
        let models = index
            .file_for_path(Utf8Path::new("/workspace/app/models.py"))
            .expect("models.py should be indexed");
        assert_eq!(
            index.file_path(models),
            Some(Utf8Path::new("/workspace/app/models.py"))
        );
        assert_eq!(index.files_by_name("models.py"), vec![models]);
    }

    #[test]
    fn settings_candidates_do_not_treat_unavailable_layout_as_empty() {
        let mut db = TestDb::default();

        assert_eq!(
            settings_module_candidates(&db, db.project()).clone(),
            SettingsModuleCandidatesOutcome::LayoutUnavailable {
                issue: ProjectLayoutIssue::SourceInventoryAbsent,
            }
        );

        db.set_project_source_inventory(ready_inventory(
            &db,
            &[
                "/workspace/project/settings.py",
                "/workspace/project/dev/settings.py",
            ],
        ));

        assert_eq!(
            settings_module_candidates(&db, db.project()).clone(),
            SettingsModuleCandidatesOutcome::Ready(vec![
                "project.dev.settings".to_string(),
                "project.settings".to_string(),
            ])
        );
    }

    #[test]
    fn layout_invalidates_for_source_inventory_but_not_enrichment() {
        let mut db = TestDb::default();

        let _ = project_layout_file_count(&db, db.project());
        let _ = db.take_events();

        db.project()
            .set_enrichment(&mut db)
            .to(ProjectEnrichment::Disabled);
        assert_eq!(project_layout_file_count(&db, db.project()), 0);
        let events = db.take_events();
        assert!(!db.tracked_query_executed(&events, "project_layout_index"));

        db.set_project_source_inventory(ready_inventory(&db, &["/workspace/app/models.py"]));
        assert_eq!(project_layout_file_count(&db, db.project()), 1);
        let events = db.take_events();
        assert!(db.tracked_query_executed(&events, "project_layout_index"));
    }

    #[salsa::tracked]
    fn project_layout_file_count(db: &dyn crate::Db, project: Project) -> usize {
        match project_layout_index(db, project) {
            ProjectLayoutIndexOutcome::Ready(index) => index.files.len(),
            ProjectLayoutIndexOutcome::Absent { .. }
            | ProjectLayoutIndexOutcome::Unavailable { .. } => 0,
        }
    }
}
