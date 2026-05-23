use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileKind;

use crate::Db;
use crate::Project;
use crate::ProjectDiscovery;
use crate::ProjectSourceInventory;
use crate::PyModuleName;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportRoot {
    path: Utf8PathBuf,
    kind: ImportRootKind,
}

impl ImportRoot {
    #[must_use]
    pub fn new(path: Utf8PathBuf, kind: ImportRootKind) -> Self {
        Self { path, kind }
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        self.path.as_path()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ImportRootKind {
    SourceRoot,
    SrcConvention,
    PythonPath,
    SitePackagesHint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedModule {
    module: PyModuleName,
    location: ModuleLocation,
}

impl ResolvedModule {
    #[must_use]
    pub fn location(&self) -> &ModuleLocation {
        &self.location
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleLocation {
    ModuleFile { file: File, path: Utf8PathBuf },
    Package { file: File, path: Utf8PathBuf },
}

impl ModuleLocation {
    #[must_use]
    pub fn file(&self) -> File {
        match self {
            Self::ModuleFile { file, .. } | Self::Package { file, .. } => *file,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        match self {
            Self::ModuleFile { path, .. } | Self::Package { path, .. } => path.as_path(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleResolution {
    requested: PyModuleName,
    outcome: ModuleResolutionOutcome,
}

impl ModuleResolution {
    #[must_use]
    pub fn outcome(&self) -> &ModuleResolutionOutcome {
        &self.outcome
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleResolutionOutcome {
    Resolved(ResolvedModule),
    Unresolved(ModuleResolutionError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleResolutionError {
    NoImportRoots,
    RootUnavailable(Utf8PathBuf),
    NotFound,
    MultipleCandidates(Vec<ResolvedModule>),
    UnsupportedModuleName,
}

#[salsa::tracked(returns(ref))]
pub fn import_roots(db: &dyn Db, project: Project) -> Vec<ImportRoot> {
    let mut roots = Vec::new();

    if let ProjectSourceInventory::Ready(files) = project.source_inventory(db) {
        for entry in files.merged().data(db).roots() {
            push_import_root(
                &mut roots,
                entry.root().path().to_owned(),
                ImportRootKind::SourceRoot,
            );
            let src = entry.root().path().join("src");
            if files
                .merged()
                .data(db)
                .files()
                .iter()
                .any(|file| file.path().starts_with(src.as_path()))
            {
                push_import_root(&mut roots, src, ImportRootKind::SrcConvention);
            }
        }
    }

    if let ProjectDiscovery::Ready(discovery) = project.discovery(db) {
        for root in discovery.roots() {
            for pythonpath in root.pythonpath(db) {
                push_import_root(&mut roots, pythonpath.clone(), ImportRootKind::PythonPath);
            }
            if let Some(interpreter) = root.interpreter(db) {
                if let Some(python) = interpreter.python_path(root.root(db)) {
                    if let Some(prefix) = python.parent().and_then(Utf8Path::parent) {
                        push_import_root(
                            &mut roots,
                            prefix.join("lib").join("site-packages"),
                            ImportRootKind::SitePackagesHint,
                        );
                    }
                }
            }
        }
    }

    roots.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    roots.dedup_by(|left, right| left.path == right.path && left.kind == right.kind);
    roots
}

#[must_use]
pub fn module_name_for_path(
    db: &dyn Db,
    project: Project,
    path: &Utf8Path,
) -> Option<PyModuleName> {
    import_roots(db, project)
        .iter()
        .filter(|root| path.starts_with(root.path()))
        .max_by_key(|root| root.path().as_str().len())
        .and_then(|root| path.strip_prefix(root.path()).ok())
        .and_then(|relative| PyModuleName::from_relative_python_module(relative).ok())
}

#[salsa::tracked(returns(ref))]
#[allow(clippy::too_many_lines)]
pub fn resolve_module(db: &dyn Db, project: Project, requested: PyModuleName) -> ModuleResolution {
    let roots = import_roots(db, project).clone();
    if roots.is_empty() {
        return ModuleResolution {
            requested,
            outcome: ModuleResolutionOutcome::Unresolved(ModuleResolutionError::NoImportRoots),
        };
    }

    let ProjectSourceInventory::Ready(files) = project.source_inventory(db) else {
        let root = roots
            .first()
            .map(|root| root.path.clone())
            .unwrap_or_default();
        return ModuleResolution {
            requested,
            outcome: ModuleResolutionOutcome::Unresolved(ModuleResolutionError::RootUnavailable(
                root,
            )),
        };
    };

    let Some(module_relative) = module_relative_path(&requested) else {
        return ModuleResolution {
            requested,
            outcome: ModuleResolutionOutcome::Unresolved(
                ModuleResolutionError::UnsupportedModuleName,
            ),
        };
    };

    let mut candidates = Vec::new();
    let data = files.merged().data(db);
    let loaded_roots = data
        .roots()
        .iter()
        .map(|entry| entry.root().path().to_owned())
        .collect::<Vec<_>>();
    let mut deferred_roots = roots
        .iter()
        .filter(|root| !root_is_loaded(root.path(), &loaded_roots))
        .map(|root| root.path.clone())
        .collect::<Vec<_>>();
    deferred_roots.sort();
    deferred_roots.dedup();

    for root in roots
        .iter()
        .filter(|root| root_is_loaded(root.path(), &loaded_roots))
    {
        let module_file = root
            .path
            .join(module_relative.as_path())
            .with_extension("py");
        let package_file = root
            .path
            .join(module_relative.as_path())
            .join("__init__.py");
        for file in data
            .files()
            .iter()
            .filter(|file| file.kind() == FileKind::Python)
        {
            if file.path() == module_file.as_path() {
                candidates.push(ResolvedModule {
                    module: requested.clone(),
                    location: ModuleLocation::ModuleFile {
                        file: file.file(),
                        path: file.path().to_owned(),
                    },
                });
            }
            if file.path() == package_file.as_path() {
                candidates.push(ResolvedModule {
                    module: requested.clone(),
                    location: ModuleLocation::Package {
                        file: file.file(),
                        path: file.path().to_owned(),
                    },
                });
            }
        }
    }

    candidates.sort_by(|left, right| left.location.path().cmp(right.location.path()));
    candidates.dedup_by(|left, right| left.location.path() == right.location.path());

    let outcome = match candidates.len() {
        0 => {
            if let Some(root) = deferred_roots.into_iter().next() {
                ModuleResolutionOutcome::Unresolved(ModuleResolutionError::RootUnavailable(root))
            } else {
                ModuleResolutionOutcome::Unresolved(ModuleResolutionError::NotFound)
            }
        }
        1 => ModuleResolutionOutcome::Resolved(candidates.remove(0)),
        _ => ModuleResolutionOutcome::Unresolved(ModuleResolutionError::MultipleCandidates(
            candidates,
        )),
    };

    ModuleResolution { requested, outcome }
}

fn root_is_loaded(root: &Utf8Path, loaded_roots: &[Utf8PathBuf]) -> bool {
    loaded_roots
        .iter()
        .any(|loaded| root.starts_with(loaded.as_path()))
}

fn push_import_root(roots: &mut Vec<ImportRoot>, path: Utf8PathBuf, kind: ImportRootKind) {
    roots.push(ImportRoot::new(path, kind));
}

fn module_relative_path(module: &PyModuleName) -> Option<Utf8PathBuf> {
    let mut path = Utf8PathBuf::new();
    for part in module.as_str().split('.') {
        if part.is_empty() {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::FileRootKind;
    use djls_source::LoadedSourceFile;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_source::SourceRoot;
    use djls_source::SourceRootEntry;
    use djls_source::SourceRootId;

    use super::*;
    use crate::ProjectDiscoverySet;
    use crate::ProjectEnrichment;
    use crate::ProjectEnvVars;
    use crate::ProjectSourceFilesIssue;
    use crate::ReadyProjectSourceFiles;
    use crate::RootDiscoveryInput;

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

    fn ready_inventory(db: &TestDb, roots: &[&str], paths: &[&str]) -> ProjectSourceInventory {
        let source_roots = roots
            .iter()
            .map(|root| {
                let path = Utf8PathBuf::from(root);
                SourceRoot::new(SourceRootId::new(path.clone()), path, FileRootKind::Project)
            })
            .collect::<Vec<_>>();
        let root_entries = source_roots
            .iter()
            .cloned()
            .map(SourceRootEntry::new)
            .collect::<Vec<_>>();
        let files = paths
            .iter()
            .map(|path| {
                let path = Utf8PathBuf::from(path);
                let root = source_roots
                    .iter()
                    .filter(|root| path.starts_with(root.path()))
                    .max_by_key(|root| root.path().as_str().len())
                    .expect("test path should be under source root");
                LoadedSourceFile::new(
                    path.clone(),
                    root.id().clone(),
                    db.get_or_create_file(path.as_path()),
                )
            })
            .collect::<Vec<_>>();
        let data = SourceFileSetData::new(root_entries, files).expect("test data should be valid");
        ProjectSourceInventory::Ready(ReadyProjectSourceFiles::new(
            crate::loading::files::ProjectFileSetPartitions::default(),
            SourceFileSet::new(db, data),
        ))
    }

    fn discovery(db: &TestDb, root: &str, pythonpath: Vec<Utf8PathBuf>) -> ProjectDiscovery {
        let root = RootDiscoveryInput::new(
            db,
            Utf8PathBuf::from(root),
            None,
            None,
            Vec::new(),
            pythonpath,
            ProjectEnvVars::default(),
            Vec::new(),
        );
        ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        )
    }

    #[test]
    fn resolver_import_roots_include_source_src_convention_and_pythonpath() {
        let mut db = TestDb::with_project();
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &["/workspace/src/app/models.py"],
        ));
        db.set_project_discovery(discovery(
            &db,
            "/workspace",
            vec![Utf8PathBuf::from("/workspace/libs")],
        ));

        let roots = import_roots(&db, db.project());

        assert!(roots.iter().any(|root| {
            root.path() == Utf8Path::new("/workspace") && root.kind == ImportRootKind::SourceRoot
        }));
        assert!(roots.iter().any(|root| {
            root.path() == Utf8Path::new("/workspace/src")
                && root.kind == ImportRootKind::SrcConvention
        }));
        assert!(roots.iter().any(|root| {
            root.path() == Utf8Path::new("/workspace/libs")
                && root.kind == ImportRootKind::PythonPath
        }));
    }

    #[test]
    fn resolver_resolves_module_file_and_package_init() {
        let mut db = TestDb::with_project();
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &[
                "/workspace/app/models.py",
                "/workspace/app/conf/__init__.py",
            ],
        ));

        let models = resolve_module(
            &db,
            db.project(),
            PyModuleName::parse("app.models").unwrap(),
        );
        let conf = resolve_module(&db, db.project(), PyModuleName::parse("app.conf").unwrap());

        assert!(matches!(
            models.outcome(),
            ModuleResolutionOutcome::Resolved(ResolvedModule {
                location: ModuleLocation::ModuleFile { .. },
                ..
            })
        ));
        assert!(matches!(
            conf.outcome(),
            ModuleResolutionOutcome::Resolved(ResolvedModule {
                location: ModuleLocation::Package { .. },
                ..
            })
        ));
    }

    #[test]
    fn resolver_reports_ambiguous_modules_across_roots() {
        let mut db = TestDb::with_project();
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace/a", "/workspace/b"],
            &["/workspace/a/app/models.py", "/workspace/b/app/models.py"],
        ));

        let resolution = resolve_module(
            &db,
            db.project(),
            PyModuleName::parse("app.models").unwrap(),
        );

        assert!(matches!(
            resolution.outcome(),
            ModuleResolutionOutcome::Unresolved(ModuleResolutionError::MultipleCandidates(candidates))
                if candidates.len() == 2
        ));
    }

    #[test]
    fn resolver_defers_not_found_when_known_pythonpath_root_is_unloaded() {
        let mut db = TestDb::with_project();
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &["/workspace/app/models.py"],
        ));
        db.set_project_discovery(discovery(
            &db,
            "/workspace",
            vec![Utf8PathBuf::from("/external/libs")],
        ));

        let resolution = resolve_module(
            &db,
            db.project(),
            PyModuleName::parse("external_app.models").unwrap(),
        );

        assert!(matches!(
            resolution.outcome(),
            ModuleResolutionOutcome::Unresolved(ModuleResolutionError::RootUnavailable(root))
                if root == &Utf8PathBuf::from("/external/libs")
        ));
    }

    #[test]
    fn resolver_defers_when_import_roots_known_but_sources_unloaded() {
        let mut db = TestDb::with_project();
        db.set_project_discovery(discovery(
            &db,
            "/workspace",
            vec![Utf8PathBuf::from("/workspace/libs")],
        ));

        let resolution = resolve_module(
            &db,
            db.project(),
            PyModuleName::parse("app.models").unwrap(),
        );

        assert!(matches!(
            resolution.outcome(),
            ModuleResolutionOutcome::Unresolved(ModuleResolutionError::RootUnavailable(_))
        ));
    }
}
