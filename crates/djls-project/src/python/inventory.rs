use djls_source::File;

use crate::apps::installed_apps;
use crate::apps::InstalledAppResolution;
use crate::db::Db;
use crate::environments::DjangoEnvironmentId;
use crate::names::PyModuleName;
use crate::project::Project;
use crate::provenance::Origin;
use crate::provenance::OriginSet;
use crate::resolver::module_name_for_path;
use crate::source_files::SourceFileInventory;
use crate::templates::template_tag_libraries;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonModule {
    module: PyModuleName,
    file: File,
    origin: OriginSet,
}

impl PythonModule {
    #[must_use]
    pub fn module(&self) -> &PyModuleName {
        &self.module
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }

    #[must_use]
    pub fn origin(&self) -> &OriginSet {
        &self.origin
    }
}

#[salsa::tracked(returns(ref))]
pub fn model_modules(db: &dyn Db, project: Project) -> Vec<PythonModule> {
    let SourceFileInventory::Ready(files) = project.source_inventory(db) else {
        return Vec::new();
    };

    let data = files.merged().data(db);
    let all_paths = data
        .files()
        .iter()
        .map(|entry| entry.path().to_owned())
        .collect::<Vec<_>>();
    let mut modules = Vec::new();

    for env in crate::environments::known_django_environment_ids(db, project) {
        for entry in data.files() {
            let path = entry.path();
            if !is_model_module_candidate(path, &all_paths) {
                continue;
            }
            let Some(module) = installed_app_module_name_for_path(db, project, &env, path)
                .or_else(|| module_name_for_path(db, project, path))
            else {
                continue;
            };
            push_python_module(&mut modules, module, entry.file());
        }
    }

    modules.sort_by(|left, right| left.module.cmp(&right.module));
    modules
}

#[salsa::tracked(returns(ref))]
pub fn template_tag_modules(db: &dyn Db, project: Project) -> Vec<PythonModule> {
    let SourceFileInventory::Ready(files) = project.source_inventory(db) else {
        return Vec::new();
    };

    let data = files.merged().data(db);
    let mut modules = Vec::new();

    for env in crate::environments::known_django_environment_ids(db, project) {
        let template_tag_files = template_tag_libraries(db, project, env.clone()).resolved_files();
        for entry in data.files() {
            if !template_tag_files.contains(&entry.file()) {
                continue;
            }
            let Some(module) = installed_app_module_name_for_path(db, project, &env, entry.path())
                .or_else(|| module_name_for_path(db, project, entry.path()))
            else {
                continue;
            };
            push_python_module(&mut modules, module, entry.file());
        }
    }

    modules.sort_by(|left, right| left.module.cmp(&right.module));
    modules
}

fn push_python_module(modules: &mut Vec<PythonModule>, module: PyModuleName, file: File) {
    if modules
        .iter()
        .any(|existing: &PythonModule| existing.module() == &module)
    {
        return;
    }
    modules.push(PythonModule {
        module,
        file,
        origin: OriginSet::single(Origin::Convention { file }),
    });
}

fn installed_app_module_name_for_path(
    db: &dyn Db,
    project: Project,
    env: &DjangoEnvironmentId,
    path: &camino::Utf8Path,
) -> Option<PyModuleName> {
    for app in installed_apps(db, project, env.clone()) {
        let (root, base_module) = match app.resolution() {
            InstalledAppResolution::Package { module, file } => {
                (app_root_for_file(db, *file)?, module.clone())
            }
            InstalledAppResolution::AppConfig { config, file } => {
                let root = config
                    .path()
                    .map(camino::Utf8Path::to_owned)
                    .or_else(|| app_root_for_file(db, *file))?;
                let module = config
                    .name()
                    .and_then(|name| PyModuleName::parse(name).ok())
                    .or_else(|| config.module().parent())?;
                (root, module)
            }
            InstalledAppResolution::Unresolved(_) => continue,
        };
        if !path.starts_with(root.as_path()) {
            continue;
        }
        let relative = path.strip_prefix(root.as_path()).ok()?.with_extension("");
        let relative = relative
            .components()
            .map(|component| component.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let module = if relative.is_empty() || relative == "__init__" {
            base_module.as_str().to_string()
        } else {
            format!("{}.{}", base_module.as_str(), relative)
        };
        return PyModuleName::parse(&module).ok();
    }
    None
}

fn app_root_for_file(db: &dyn Db, file: File) -> Option<camino::Utf8PathBuf> {
    let path = file.path(db);
    let parent = path.parent()?;
    if path.file_name() == Some("__init__.py") || path.file_name() == Some("apps.py") {
        return Some(parent.to_owned());
    }
    parent.parent().map(camino::Utf8Path::to_owned)
}

fn is_model_module_candidate(path: &camino::Utf8Path, all_paths: &[camino::Utf8PathBuf]) -> bool {
    if path.file_name() == Some("models.py") {
        return true;
    }
    if path.extension() != Some("py") {
        return false;
    }
    let mut dir = path.parent();
    while let Some(parent) = dir {
        if parent.file_name() == Some("models") {
            return all_paths
                .iter()
                .any(|candidate| candidate == &parent.join("__init__.py"));
        }
        dir = parent.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8PathBuf;
    use djls_source::SourceFiles;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use super::*;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::testing::manage_py_path;
    use crate::testing::package_init_path;
    use crate::testing::project_discovery_set_for_test;
    use crate::testing::ready_source_inventory_with_roots_for_test;
    use crate::testing::settings_file_path;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        project: std::sync::OnceLock<Project>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, path: &camino::Utf8Path) -> std::io::Result<String> {
            self.fs.lock().unwrap().read_to_string(path)
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
                    crate::SourceFileInventory::Unavailable {
                        issue: crate::SourceFilesIssue::NotLoaded,
                    },
                    ProjectRootDiscovery::Absent,
                    crate::ProjectEnrichment::Absent,
                ))
                .expect("project should initialize once");
            db
        }

        fn set_file(&mut self, path: &str, content: &str) {
            self.fs
                .lock()
                .unwrap()
                .add_file(path.into(), content.to_string());
        }
    }

    #[test]
    fn python_module_inventory_classifies_workspace_modules() {
        let mut db = TestDb::with_project();
        let root = Utf8PathBuf::from("/workspace");
        db.set_file(
            "/workspace/config/settings.py",
            "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'OPTIONS': {'libraries': {'ui': 'blog.ui_tags'}}}]\n",
        );
        db.set_source_file_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone()],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
                package_init_path(&root, "blog"),
                root.join("blog/models.py"),
                root.join("blog/models/__init__.py"),
                root.join("blog/models/post.py"),
                root.join("blog/templatetags/__init__.py"),
                root.join("blog/templatetags/blog_tags.py"),
                root.join("blog/ui_tags.py"),
                root.join("blog/apps.py"),
                package_init_path(&root, "shop"),
                root.join("shop/templatetags/__init__.py"),
                root.join("shop/templatetags/unused.py"),
            ],
        ));
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(project_discovery_set_for_test(
            &db, root,
        )));

        let models = model_modules(&db, db.project());
        let template_tags = template_tag_modules(&db, db.project());

        assert!(models
            .iter()
            .any(|module| module.module().as_str() == "blog.models"));
        assert!(models
            .iter()
            .any(|module| module.module().as_str() == "blog.models.post"));
        assert!(template_tags
            .iter()
            .any(|module| module.module().as_str() == "blog.templatetags.blog_tags"));
        assert!(template_tags
            .iter()
            .any(|module| module.module().as_str() == "blog.ui_tags"));
        assert!(template_tags
            .iter()
            .all(|module| module.module().as_str() != "shop.templatetags.unused"));
    }
}
