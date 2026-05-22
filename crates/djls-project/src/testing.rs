use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::LoadedSourceFile;
use djls_source::SourceFileSet;
use djls_source::SourceFileSetData;
use djls_source::SourceRoot;
use djls_source::SourceRootEntry;
use djls_source::SourceRootId;

use crate::Db;
use crate::ProjectDiscoverySet;
use crate::ProjectEnvVars;
use crate::ProjectSourceInventory;
use crate::ReadyProjectSourceFiles;
use crate::RootDiscoveryInput;

#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn source_file_set_for_test(
    db: &dyn Db,
    root: impl Into<Utf8PathBuf>,
    paths: impl IntoIterator<Item = Utf8PathBuf>,
) -> SourceFileSet {
    let root_path = root.into();
    let root_id = SourceRootId::new(root_path.clone());
    let root = SourceRoot::new(root_id.clone(), root_path, FileRootKind::Project);
    let roots = vec![SourceRootEntry::new(root)];
    let files = paths
        .into_iter()
        .map(|path| {
            LoadedSourceFile::new(path.clone(), root_id.clone(), db.get_or_create_file(&path))
        })
        .collect::<Vec<_>>();
    let data = SourceFileSetData::new(roots, files).expect("test source file set should be valid");
    SourceFileSet::new(db, data)
}

#[must_use]
pub fn ready_source_inventory_for_test(
    db: &dyn Db,
    root: impl Into<Utf8PathBuf>,
    paths: impl IntoIterator<Item = Utf8PathBuf>,
) -> ProjectSourceInventory {
    ready_source_inventory_with_roots_for_test(db, vec![root.into()], paths)
}

#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn ready_source_inventory_with_roots_for_test(
    db: &dyn Db,
    roots: Vec<Utf8PathBuf>,
    paths: impl IntoIterator<Item = Utf8PathBuf>,
) -> ProjectSourceInventory {
    let roots = roots
        .into_iter()
        .map(|root_path| {
            SourceRoot::new(
                SourceRootId::new(root_path.clone()),
                root_path,
                FileRootKind::Project,
            )
        })
        .collect::<Vec<_>>();
    let root_entries = roots
        .iter()
        .cloned()
        .map(SourceRootEntry::new)
        .collect::<Vec<_>>();
    let files = paths
        .into_iter()
        .map(|path| {
            let root = roots
                .iter()
                .filter(|root| path.starts_with(root.path()))
                .max_by_key(|root| root.path().as_str().len())
                .expect("test path should be inside a root");
            LoadedSourceFile::new(
                path.clone(),
                root.id().clone(),
                db.get_or_create_file(&path),
            )
        })
        .collect::<Vec<_>>();
    let data =
        SourceFileSetData::new(root_entries, files).expect("test source file set should be valid");
    ProjectSourceInventory::Ready(ReadyProjectSourceFiles::merged_for_test(
        SourceFileSet::new(db, data),
    ))
}

#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn project_discovery_set_for_test(
    db: &dyn Db,
    root: impl Into<Utf8PathBuf>,
) -> ProjectDiscoverySet {
    let root = RootDiscoveryInput::new(
        db,
        root.into(),
        None,
        None,
        Vec::new(),
        Vec::new(),
        ProjectEnvVars::default(),
        Vec::new(),
    );
    ProjectDiscoverySet::new(vec![root]).expect("test discovery should have one root")
}

#[must_use]
pub fn manage_py_path(root: &Utf8Path) -> Utf8PathBuf {
    root.join("manage.py")
}

#[must_use]
pub fn settings_file_path(root: &Utf8Path, package: &str) -> Utf8PathBuf {
    root.join(package).join("settings.py")
}

#[must_use]
pub fn package_init_path(root: &Utf8Path, package: &str) -> Utf8PathBuf {
    root.join(package).join("__init__.py")
}

#[must_use]
pub fn template_path(root: &Utf8Path, relative: &str) -> Utf8PathBuf {
    root.join("templates").join(relative)
}

#[must_use]
pub fn app_dir(root: &Utf8Path, app: &str) -> Utf8PathBuf {
    root.join(app)
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use djls_source::SourceFiles;

    use super::*;
    use crate::Project;
    use crate::ProjectDiscovery;
    use crate::ProjectEnrichment;
    use crate::ProjectSourceFilesIssue;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        project: OnceLock<Project>,
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
            *self.project.get().expect("test project initialized")
        }
    }

    impl TestDb {
        fn with_project() -> Self {
            let db = Self::default();
            db.project
                .set(Project::new(
                    &db,
                    ProjectSourceInventory::Unavailable {
                        issue: ProjectSourceFilesIssue::NotLoaded,
                    },
                    ProjectDiscovery::Absent,
                    ProjectEnrichment::Absent,
                ))
                .expect("project should initialize once");
            db
        }
    }

    #[test]
    fn testing_helpers_build_source_inventory_and_discovery_without_django() {
        let db = TestDb::with_project();
        let root = Utf8PathBuf::from("/workspace");
        let paths = vec![
            manage_py_path(&root),
            package_init_path(&root, "config"),
            settings_file_path(&root, "config"),
            template_path(&root, "index.html"),
        ];

        let inventory = ready_source_inventory_for_test(&db, root.clone(), paths);
        let discovery = project_discovery_set_for_test(&db, root.clone());

        let ProjectSourceInventory::Ready(files) = inventory else {
            panic!("source inventory should be ready");
        };
        assert_eq!(files.merged().data(&db).files().len(), 4);
        assert_eq!(discovery.roots()[0].root(&db), &root);
        assert_eq!(app_dir(&root, "blog"), Utf8PathBuf::from("/workspace/blog"));
    }
}
