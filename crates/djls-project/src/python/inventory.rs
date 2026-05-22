use djls_source::File;

use crate::installed_apps;
use crate::template_tag_libraries;
use crate::Db;
use crate::DjangoEnvironmentId;
use crate::InstalledAppResolution;
use crate::Origin;
use crate::OriginSet;
use crate::Project;
use crate::ProjectSourceInventory;
use crate::PyModuleName;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PythonModuleRole {
    Model,
    TemplateTag,
    AppConfig,
    Urls,
    Admin,
    Forms,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonModule {
    module: PyModuleName,
    file: File,
    roles: Vec<PythonModuleRole>,
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
    pub fn roles(&self) -> &[PythonModuleRole] {
        &self.roles
    }

    #[must_use]
    pub fn has_role(&self, role: PythonModuleRole) -> bool {
        self.roles.contains(&role)
    }

    #[must_use]
    pub fn origin(&self) -> &OriginSet {
        &self.origin
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PythonModuleInventory {
    modules: Vec<PythonModule>,
}

impl PythonModuleInventory {
    #[must_use]
    pub fn modules(&self) -> &[PythonModule] {
        &self.modules
    }
}

#[salsa::tracked(returns(ref))]
pub fn python_module_inventory(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> PythonModuleInventory {
    let ProjectSourceInventory::Ready(files) = project.source_inventory(db) else {
        return PythonModuleInventory::default();
    };

    let data = files.merged().data(db);
    let all_paths = data
        .files()
        .iter()
        .map(|entry| entry.path().to_owned())
        .collect::<Vec<_>>();
    let template_tag_files = template_tag_library_files(db, project, env.clone());
    let mut modules = data
        .files()
        .iter()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension() != Some("py") {
                return None;
            }
            let module = module_name_for_inventory_path(db, project, &env, path)?;
            let roles =
                roles_for_path(path, &all_paths, template_tag_files.contains(&entry.file()));
            if roles.is_empty() {
                return None;
            }
            Some(PythonModule {
                module,
                file: entry.file(),
                roles,
                origin: OriginSet::single(Origin::Convention { file: entry.file() }),
            })
        })
        .collect::<Vec<_>>();

    modules.sort_by(|left, right| left.module.cmp(&right.module));
    PythonModuleInventory { modules }
}

fn module_name_for_inventory_path(
    db: &dyn Db,
    project: Project,
    env: &DjangoEnvironmentId,
    path: &camino::Utf8Path,
) -> Option<PyModuleName> {
    installed_app_module_name_for_path(db, project, env, path)
        .or_else(|| crate::module_name_for_path(db, project, path))
        .and_then(|module| normalize_init_module(module, path))
}

fn normalize_init_module(module: PyModuleName, path: &camino::Utf8Path) -> Option<PyModuleName> {
    if path.file_name() != Some("__init__.py") {
        return Some(module);
    }
    let module = module.as_str();
    let stripped = module.strip_suffix(".__init__").unwrap_or(module);
    PyModuleName::parse(stripped).ok()
}

fn installed_app_module_name_for_path(
    db: &dyn Db,
    project: Project,
    env: &DjangoEnvironmentId,
    path: &camino::Utf8Path,
) -> Option<PyModuleName> {
    for app in installed_apps(db, project, env.clone()).iter() {
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
                    .or_else(|| parent_module(config.module()))?;
                (root, module)
            }
            InstalledAppResolution::Missing { .. }
            | InstalledAppResolution::Ambiguous { .. }
            | InstalledAppResolution::Deferred { .. } => continue,
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

fn parent_module(module: &PyModuleName) -> Option<PyModuleName> {
    let parent = module.as_str().rsplit_once('.')?.0;
    PyModuleName::parse(parent).ok()
}

fn template_tag_library_files(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> Vec<File> {
    template_tag_libraries(db, project, env)
        .libraries()
        .iter()
        .filter_map(|library| match library.resolution() {
            crate::TemplateTagLibraryResolution::Resolved { file } => Some(*file),
            crate::TemplateTagLibraryResolution::Builtin
            | crate::TemplateTagLibraryResolution::Unresolved { .. }
            | crate::TemplateTagLibraryResolution::Ambiguous { .. } => None,
        })
        .collect()
}

fn roles_for_path(
    path: &camino::Utf8Path,
    all_paths: &[camino::Utf8PathBuf],
    is_template_tag_file: bool,
) -> Vec<PythonModuleRole> {
    let mut roles = Vec::new();
    match path.file_name() {
        Some("models.py") => roles.push(PythonModuleRole::Model),
        Some("apps.py") => roles.push(PythonModuleRole::AppConfig),
        Some("urls.py") => roles.push(PythonModuleRole::Urls),
        Some("admin.py") => roles.push(PythonModuleRole::Admin),
        Some("forms.py") => roles.push(PythonModuleRole::Forms),
        Some("__init__.py") | None => {}
        Some(_) => {}
    }
    if is_model_package_file(path, all_paths) && !roles.contains(&PythonModuleRole::Model) {
        roles.push(PythonModuleRole::Model);
    }
    if is_template_tag_file && !roles.contains(&PythonModuleRole::TemplateTag) {
        roles.push(PythonModuleRole::TemplateTag);
    }
    roles
}

fn is_model_package_file(path: &camino::Utf8Path, all_paths: &[camino::Utf8PathBuf]) -> bool {
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
    use crate::testing::manage_py_path;
    use crate::testing::package_init_path;
    use crate::testing::project_discovery_set_for_test;
    use crate::testing::ready_source_inventory_with_roots_for_test;
    use crate::testing::settings_file_path;
    use crate::DjangoEnvironmentCandidatesOutcome;
    use crate::ProjectDiscovery;

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
                    crate::ProjectSourceInventory::Unavailable {
                        issue: crate::ProjectSourceFilesIssue::NotLoaded,
                    },
                    ProjectDiscovery::Absent,
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

    fn env(db: &TestDb) -> DjangoEnvironmentId {
        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
            crate::django_environment_candidates(db, db.project())
        else {
            panic!("environment candidates should be ready");
        };
        candidates[0].id().clone()
    }

    #[test]
    fn python_module_inventory_classifies_workspace_modules() {
        let mut db = TestDb::with_project();
        let root = Utf8PathBuf::from("/workspace");
        db.set_file(
            "/workspace/config/settings.py",
            "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'OPTIONS': {'libraries': {'ui': 'blog.ui_tags'}}}]\n",
        );
        db.set_project_source_inventory(ready_source_inventory_with_roots_for_test(
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
        db.set_project_discovery(ProjectDiscovery::Ready(project_discovery_set_for_test(
            &db, root,
        )));

        let inventory = python_module_inventory(&db, db.project(), env(&db));
        let models = inventory
            .modules()
            .iter()
            .find(|module| module.module().as_str() == "blog.models")
            .expect("models module should be indexed");
        let model_package = inventory
            .modules()
            .iter()
            .find(|module| module.module().as_str() == "blog.models.post")
            .expect("model package module should be indexed");
        let tags = inventory
            .modules()
            .iter()
            .find(|module| module.module().as_str() == "blog.templatetags.blog_tags")
            .expect("installed templatetag module should be indexed");
        let alias = inventory
            .modules()
            .iter()
            .find(|module| module.module().as_str() == "blog.ui_tags")
            .expect("settings library alias module should be indexed");

        assert!(models.has_role(PythonModuleRole::Model));
        assert!(model_package.has_role(PythonModuleRole::Model));
        assert!(tags.has_role(PythonModuleRole::TemplateTag));
        assert!(alias.has_role(PythonModuleRole::TemplateTag));
        assert!(inventory
            .modules()
            .iter()
            .all(|module| module.module().as_str() != "shop.templatetags.unused"));
    }
}
