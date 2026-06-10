//! Module path → file path resolution using Python search paths.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use rustc_hash::FxHashMap;

use crate::project::Interpreter;
use crate::project::db::Db as ProjectDb;
use crate::project::input::Project;
use crate::project::input::PythonModule;
use crate::python::ModulePath;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchPath {
    FirstParty(Utf8PathBuf),
    Extra(Utf8PathBuf),
    SitePackages(Utf8PathBuf),
}

impl SearchPath {
    fn first_party(path: Utf8PathBuf) -> Self {
        Self::FirstParty(path)
    }

    fn extra(path: Utf8PathBuf) -> Self {
        Self::Extra(path)
    }

    fn site_packages(path: Utf8PathBuf) -> Self {
        Self::SitePackages(path)
    }

    fn from_pythonpath(
        root: &Utf8Path,
        discovered_site_packages: Option<&Utf8Path>,
        path: Utf8PathBuf,
    ) -> Self {
        if discovered_site_packages.is_some_and(|site_packages| site_packages == path)
            || has_installed_package_component(&path)
        {
            Self::site_packages(path)
        } else if path.starts_with(root) {
            Self::first_party(path)
        } else {
            Self::extra(path)
        }
    }

    #[must_use]
    pub(crate) fn path(&self) -> &Utf8Path {
        match self {
            Self::FirstParty(path) | Self::Extra(path) | Self::SitePackages(path) => path,
        }
    }

    #[must_use]
    pub(crate) fn is_first_party(&self) -> bool {
        matches!(self, Self::FirstParty(_))
    }

    fn root_kind(&self) -> FileRootKind {
        match self {
            // Extra pythonpath entries are user-edited code, so they get the
            // same low-durability treatment as project files.
            Self::FirstParty(_) | Self::Extra(_) => FileRootKind::Project,
            Self::SitePackages(_) => FileRootKind::SearchPath,
        }
    }
}

fn has_installed_package_component(path: &Utf8Path) -> bool {
    path.components()
        .any(|component| matches!(component.as_str(), "site-packages" | "dist-packages"))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPaths {
    paths: Vec<SearchPath>,
}

impl SearchPaths {
    #[must_use]
    pub(crate) fn from_project_settings(
        fs: &dyn FileSystem,
        root: &Utf8Path,
        interpreter: &Interpreter,
        pythonpath: &[String],
    ) -> Self {
        let mut search_paths = Self::default();
        search_paths.push(SearchPath::first_party(root.to_path_buf()));
        let discovered_site_packages = interpreter.site_packages_path(fs, root);

        for path in pythonpath {
            let path = Utf8PathBuf::from(path);
            if !fs.is_dir(&path) || search_paths.contains_path(&path) {
                continue;
            }

            search_paths.push(SearchPath::from_pythonpath(
                root,
                discovered_site_packages.as_deref(),
                path,
            ));
        }

        if let Some(site_packages) = discovered_site_packages
            && !search_paths.contains_path(&site_packages)
        {
            search_paths.push(SearchPath::site_packages(site_packages));
        }

        search_paths
    }

    pub(crate) fn register_roots(&self, db: &dyn ProjectDb) {
        let mut roots = Vec::new();
        for search_path in self.iter() {
            if search_path.is_first_party()
                && roots.iter().any(|(path, kind)| {
                    *kind == FileRootKind::Project && search_path.path().starts_with(path)
                })
            {
                continue;
            }

            roots.push((search_path.path().to_path_buf(), search_path.root_kind()));
        }

        db.files().replace_roots(db, roots);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &SearchPath> {
        self.paths.iter()
    }

    fn push(&mut self, search_path: SearchPath) {
        self.paths.push(search_path);
    }

    fn contains_path(&self, path: &Utf8Path) -> bool {
        self.iter().any(|search_path| search_path.path() == path)
    }
}

#[salsa::tracked(returns(ref))]
pub(crate) fn model_modules(db: &dyn ProjectDb, project: Project) -> Vec<PythonModule> {
    let search_paths = project.search_paths(db);
    let mut modules_by_path: Vec<(usize, PythonModule)> = Vec::new();
    let mut path_indexes: FxHashMap<Utf8PathBuf, usize> = FxHashMap::default();

    for search_path in search_paths.iter() {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        } else {
            tracing::warn!(
                "Search path has no registered source root: {}",
                search_path.path()
            );
        }

        let excluded_paths: Vec<_> = if search_path.is_first_party() {
            search_paths
                .iter()
                .filter(|other| {
                    !other.is_first_party() && other.path().starts_with(search_path.path())
                })
                .map(|other| other.path().to_path_buf())
                .collect()
        } else {
            Vec::new()
        };

        let search_path_len = search_path.path().as_str().len();
        for (module_path, file_path) in discover_model_files_excluding(
            db.file_system(),
            search_path.path(),
            search_path.root_kind(),
            &excluded_paths,
        ) {
            let module = PythonModule::new(
                module_path,
                file_path.clone(),
                db.get_or_create_file(&file_path),
            );
            if let Some(index) = path_indexes.get(&file_path).copied() {
                let (existing_search_path_len, existing) = &mut modules_by_path[index];
                if search_path_len > *existing_search_path_len {
                    *existing_search_path_len = search_path_len;
                    *existing = module;
                }
            } else {
                path_indexes.insert(file_path, modules_by_path.len());
                modules_by_path.push((search_path_len, module));
            }
        }
    }

    modules_by_path
        .into_iter()
        .map(|(_search_path_len, module)| module)
        .collect()
}

pub(crate) fn templatetag_module_file(
    fs: &dyn FileSystem,
    module_path: &str,
    search_path: &Utf8Path,
) -> Option<Utf8PathBuf> {
    let mut candidate = search_path.to_path_buf();
    for part in module_path.split('.') {
        candidate.push(part);
    }

    let py_file = candidate.with_extension("py");
    if fs.is_file(&py_file) {
        return Some(py_file);
    }

    let init_file = candidate.join("__init__.py");
    fs.is_file(&init_file).then_some(init_file)
}

#[salsa::tracked(returns(ref))]
pub(crate) fn templatetag_modules(db: &dyn ProjectDb, project: Project) -> Vec<PythonModule> {
    let search_paths: Vec<_> = project.search_paths(db).iter().collect();
    for search_path in &search_paths {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        } else {
            tracing::warn!(
                "Search path has no registered source root: {}",
                search_path.path()
            );
        }
    }

    let mut modules = Vec::new();

    for (module_index, module_path) in project
        .template_libraries(db)
        .registration_modules()
        .into_iter()
        .enumerate()
    {
        for search_path in &search_paths {
            let Some(file_path) =
                templatetag_module_file(db.file_system(), module_path.as_str(), search_path.path())
            else {
                continue;
            };

            modules.push((
                module_index,
                PythonModule::new(
                    ModulePath::new(module_path.as_str().to_string()),
                    file_path.clone(),
                    db.get_or_create_file(&file_path),
                ),
            ));
            break;
        }
    }

    modules.sort_by(|(left_index, left), (right_index, right)| {
        left_index
            .cmp(right_index)
            .then_with(|| left.module_path().cmp(right.module_path()))
            .then_with(|| left.path().cmp(right.path()))
    });

    modules.into_iter().map(|(_index, module)| module).collect()
}

/// Discover Django model source files and return their resolved module paths.
///
/// Finds `models.py` files and `.py` files inside `models/` packages
/// (directories with `__init__.py`) without reading file contents.
#[cfg(test)]
#[must_use]
pub(crate) fn discover_model_files(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
) -> Vec<(ModulePath, Utf8PathBuf)> {
    discover_model_files_excluding(fs, base_dir, root_kind, &[])
}

fn discover_model_files_excluding(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
) -> Vec<(ModulePath, Utf8PathBuf)> {
    let options = match root_kind {
        FileRootKind::Project => WalkOptions::project(),
        FileRootKind::SearchPath => WalkOptions::library_search_path(),
    };

    let mut results = Vec::new();

    let entries = match fs.walk_entries(base_dir, &options) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Failed to walk Python source root {}: {}", base_dir, err);
            return results;
        }
    };

    for entry in entries {
        if entry.kind != WalkEntryKind::File {
            continue;
        }
        let path = entry.path;

        let is_django_model_source = if path.file_name() == Some("models.py") {
            true
        } else if path.extension() == Some("py") {
            let mut dir = path.parent();
            let mut in_models_package = false;
            while let Some(parent) = dir {
                if parent.file_name() == Some("models") {
                    in_models_package = fs.exists(&parent.join("__init__.py"));
                    break;
                }
                dir = parent.parent();
            }
            in_models_package
        } else {
            false
        };

        if !is_django_model_source {
            continue;
        }

        if excluded_roots
            .iter()
            .any(|excluded| path.starts_with(excluded))
        {
            continue;
        }

        let Some(rel) = path.strip_prefix(base_dir).ok() else {
            continue;
        };

        results.push((ModulePath::from_relative_path(rel), path));
    }

    results.sort_by(|(a, _), (b, _)| a.cmp(b));
    results
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use djls_conf::Settings;
    use djls_source::Db as _;
    use djls_source::InMemoryFileSystem;

    use super::*;
    use crate::compute_filter_arity_specs;
    use crate::project::ProjectTemplateFiles;
    use crate::project::TemplateDirs;
    use crate::project::TemplateLibraries;
    use crate::project::TemplateLibrarySnapshot;
    use crate::project::refresh_external_data;
    use crate::python::compute_model_graph;
    use crate::testing::TestDatabase;

    fn template_libraries_for_module(module: &str) -> TemplateLibraries {
        TemplateLibraries::default().apply_active_snapshot(Some(TemplateLibrarySnapshot {
            symbols: Vec::new(),
            libraries: BTreeMap::new(),
            builtins: vec![module.to_string()],
        }))
    }

    fn project_for_search_paths(
        db: &TestDatabase,
        root: &str,
        search_paths: SearchPaths,
        template_libraries: TemplateLibraries,
    ) -> Project {
        let settings = Settings::default();
        Project::new(
            db,
            root.into(),
            search_paths,
            Interpreter::discover(settings.venv_path()),
            None,
            Vec::new(),
            Vec::new(),
            TemplateDirs::Unknown,
            settings.tagspecs().clone(),
            template_libraries,
            ProjectTemplateFiles::default(),
        )
    }

    #[test]
    fn search_paths_keep_site_packages_external_inside_project_root() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file("/project/src/app/__init__.py".into(), String::new());
        fs.add_file("/outside/pkg/__init__.py".into(), String::new());
        fs.add_file(
            "/project/.venv/lib/python3.12/site-packages/django/__init__.py".into(),
            String::new(),
        );

        let pythonpath = vec![
            "/project/src".to_string(),
            "/outside".to_string(),
            "/project/.venv/lib/python3.12/site-packages".to_string(),
        ];
        let search_paths = SearchPaths::from_project_settings(
            &fs,
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &pythonpath,
        );

        let paths: Vec<_> = search_paths.iter().map(SearchPath::path).collect();
        assert_eq!(
            paths,
            vec![
                Utf8Path::new("/project"),
                Utf8Path::new("/project/src"),
                Utf8Path::new("/outside"),
                Utf8Path::new("/project/.venv/lib/python3.12/site-packages"),
            ]
        );

        let external_paths: Vec<_> = search_paths
            .iter()
            .filter(|search_path| !search_path.is_first_party())
            .map(SearchPath::path)
            .collect();
        assert_eq!(
            external_paths,
            vec![
                Utf8Path::new("/outside"),
                Utf8Path::new("/project/.venv/lib/python3.12/site-packages"),
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn search_paths_find_windows_style_venv_site_packages() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            "/project/.venv/Lib/site-packages/django/__init__.py".into(),
            String::new(),
        );

        let search_paths = SearchPaths::from_project_settings(
            &fs,
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );

        let paths: Vec<_> = search_paths.iter().map(SearchPath::path).collect();
        assert_eq!(
            paths,
            vec![
                Utf8Path::new("/project"),
                Utf8Path::new("/project/.venv/Lib/site-packages"),
            ]
        );
    }

    #[test]
    fn model_modules_use_first_party_search_path_relative_names() {
        let db = TestDatabase::new();
        db.add_file(
            "/project/src/blog/models.py",
            "from django.db import models\nclass Article(models.Model):\n    pass\n",
        );

        let pythonpath = vec!["/project/src".to_string()];
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &pythonpath,
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());

        let modules = model_modules(&db, project);
        assert!(
            modules
                .iter()
                .any(|module| module.module_path().as_str() == "blog.models")
        );
        assert!(
            !modules
                .iter()
                .any(|module| module.module_path().as_str() == "src.blog.models")
        );
    }

    #[test]
    fn registering_search_paths_removes_obsolete_external_roots() {
        let db = TestDatabase::new();
        db.add_file("/external/pkg/models.py", "");

        let pythonpath = vec!["/external".to_string()];
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &pythonpath,
        );
        search_paths.register_roots(&db);
        let external_root = db
            .files()
            .expect_root(&db, Utf8Path::new("/external/pkg/models.py"));
        assert_eq!(external_root.kind(&db), FileRootKind::Project);

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        assert!(
            db.files()
                .root(&db, Utf8Path::new("/external/pkg/models.py"))
                .is_none()
        );
    }

    #[test]
    fn model_modules_tolerate_unregistered_search_paths() {
        let db = TestDatabase::new();
        db.add_file(
            "/shared/blog/models.py",
            "from django.db import models\nclass SharedArticle(models.Model):\n    pass\n",
        );

        let pythonpath = vec!["/shared".to_string()];
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &pythonpath,
        );
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());

        let modules = model_modules(&db, project);
        assert!(
            modules
                .iter()
                .any(|module| module.module_path().as_str() == "blog.models")
        );
    }

    #[test]
    fn templatetag_modules_tolerate_unregistered_search_paths() {
        let db = TestDatabase::new();
        db.add_file(
            "/project/django/templatetags/i18n.py",
            "from django import template\nregister = template.Library()\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        let project = project_for_search_paths(
            &db,
            "/project",
            search_paths,
            template_libraries_for_module("django.templatetags.i18n"),
        );

        let modules = templatetag_modules(&db, project);
        assert_eq!(modules.len(), 1);
        assert_eq!(
            modules[0].module_path().as_str(),
            "django.templatetags.i18n"
        );
    }

    #[test]
    fn templatetag_resolution_uses_project_venv_site_packages_root() {
        let db = TestDatabase::new();
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/django/templatetags/i18n.py",
            "from django import template\nregister = template.Library()\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project = project_for_search_paths(
            &db,
            "/project",
            search_paths,
            template_libraries_for_module("django.templatetags.i18n"),
        );

        let modules = templatetag_modules(&db, project);
        assert_eq!(modules.len(), 1);
        assert_eq!(
            modules[0].module_path().as_str(),
            "django.templatetags.i18n"
        );
        let root = db.files().expect_root(&db, modules[0].path());
        assert_eq!(root.kind(&db), FileRootKind::SearchPath);
    }

    #[test]
    fn templatetag_resolution_prefers_first_party_module_shadowing_dependency() {
        let db = TestDatabase::new();
        db.add_file(
            "/project/django/templatetags/i18n.py",
            "from django import template\nregister = template.Library()\n",
        );
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/django/templatetags/i18n.py",
            "from django import template\nregister = template.Library()\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project = project_for_search_paths(
            &db,
            "/project",
            search_paths,
            template_libraries_for_module("django.templatetags.i18n"),
        );

        let modules = templatetag_modules(&db, project);
        assert_eq!(modules.len(), 1);
        assert_eq!(
            modules[0].path(),
            Utf8Path::new("/project/django/templatetags/i18n.py")
        );
    }

    #[test]
    fn templatetag_modules_preserve_registration_order_across_roots() {
        let db = TestDatabase::new();
        db.add_file(
            "/project/a/templatetags/tags.py",
            "from django import template\nregister = template.Library()\n",
        );
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/z/templatetags/tags.py",
            "from django import template\nregister = template.Library()\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project = project_for_search_paths(
            &db,
            "/project",
            search_paths,
            TemplateLibraries::default().apply_active_snapshot(Some(TemplateLibrarySnapshot {
                symbols: Vec::new(),
                libraries: BTreeMap::new(),
                builtins: vec![
                    "a.templatetags.tags".to_string(),
                    "z.templatetags.tags".to_string(),
                ],
            })),
        );

        let modules = templatetag_modules(&db, project);
        let module_paths: Vec<_> = modules
            .iter()
            .map(|module| module.module_path().as_str())
            .collect();

        assert_eq!(
            module_paths,
            vec!["a.templatetags.tags", "z.templatetags.tags"]
        );
    }

    #[test]
    fn filter_arity_specs_preserve_builtin_registration_order_across_roots() {
        let db = TestDatabase::new();
        db.add_file(
            "/project/z_first.py",
            r"
from django import template
register = template.Library()

@register.filter
def duplicate(value):
    return value
",
        );
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/a_second.py",
            r"
from django import template
register = template.Library()

@register.filter
def duplicate(value, arg):
    return value
",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project = project_for_search_paths(
            &db,
            "/project",
            search_paths,
            TemplateLibraries::default().apply_active_snapshot(Some(TemplateLibrarySnapshot {
                symbols: Vec::new(),
                libraries: BTreeMap::new(),
                builtins: vec!["z_first".to_string(), "a_second".to_string()],
            })),
        );

        let specs = compute_filter_arity_specs(&db, project);
        let arity = specs.get("duplicate").expect("filter should be extracted");
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn project_model_graph_refresh_reads_changed_project_file() {
        let mut db = TestDatabase::new();
        db.add_file(
            "/project/blog/models.py",
            "from django.db import models\nclass Article(models.Model):\n    pass\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());
        db.set_project(project);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_none());

        db.add_file(
            "/project/blog/models.py",
            "from django.db import models\nclass Comment(models.Model):\n    pass\n",
        );
        refresh_external_data(&mut db);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_none());
        assert!(graph.get("Comment").is_some());
    }

    #[test]
    fn project_model_discovery_refreshes_through_project_refresh() {
        let mut db = TestDatabase::new();
        db.add_file(
            "/project/blog/models.py",
            "from django.db import models\nclass Article(models.Model):\n    pass\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());
        db.set_project(project);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_none());

        db.add_file(
            "/project/comments/models.py",
            "from django.db import models\nclass Comment(models.Model):\n    pass\n",
        );
        refresh_external_data(&mut db);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_some());
    }

    #[test]
    fn external_model_graph_refresh_reads_changed_site_packages_file() {
        let mut db = TestDatabase::new();
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/blog/models.py",
            "from django.db import models\nclass Article(models.Model):\n    pass\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());
        db.set_project(project);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_none());

        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/blog/models.py",
            "from django.db import models\nclass Comment(models.Model):\n    pass\n",
        );
        refresh_external_data(&mut db);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_none());
        assert!(graph.get("Comment").is_some());
    }

    #[test]
    fn external_model_graph_preserves_pythonpath_precedence() {
        let db = TestDatabase::new();
        db.add_file(
            "/zfirst/zapp/models.py",
            "from django.db import models\nclass Duplicate(models.Model):\n    pass\n",
        );
        db.add_file(
            "/afallback/aapp/models.py",
            "from django.db import models\nclass Duplicate(models.Model):\n    pass\n",
        );

        let pythonpath = vec!["/zfirst".to_string(), "/afallback".to_string()];
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &pythonpath,
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());

        let graph = compute_model_graph(&db, project);
        let model = graph.get("Duplicate").expect("model should be discovered");
        assert_eq!(model.module_path.as_str(), "zapp.models");
    }

    #[test]
    fn external_model_discovery_refreshes_through_project_refresh() {
        let mut db = TestDatabase::new();
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/blog/models.py",
            "from django.db import models\nclass Article(models.Model):\n    pass\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());
        db.set_project(project);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_none());

        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/comments/models.py",
            "from django.db import models\nclass Comment(models.Model):\n    pass\n",
        );
        refresh_external_data(&mut db);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_some());
    }

    #[test]
    fn external_model_discovery_removes_deleted_models_through_project_refresh() {
        let mut db = TestDatabase::new();
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/blog/models.py",
            "from django.db import models\nclass Article(models.Model):\n    pass\n",
        );
        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/comments/models.py",
            "from django.db import models\nclass Comment(models.Model):\n    pass\n",
        );

        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());
        db.set_project(project);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_some());

        db.remove_file("/project/.venv/lib/python3.12/site-packages/comments/models.py");
        refresh_external_data(&mut db);

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("Article").is_some());
        assert!(graph.get("Comment").is_none());
    }

    #[test]
    fn external_model_graph_reads_extra_pythonpath_models() {
        let db = TestDatabase::new();
        db.add_file(
            "/shared/blog/models.py",
            "from django.db import models\nclass SharedArticle(models.Model):\n    pass\n",
        );

        let pythonpath = vec!["/shared".to_string()];
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &pythonpath,
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());

        let graph = compute_model_graph(&db, project);
        assert!(graph.get("SharedArticle").is_some());
    }

    #[test]
    fn refresh_external_data_discovers_site_packages_created_after_bootstrap() {
        let mut db = TestDatabase::new();
        let settings = Settings::default();
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::Auto,
            &[],
        );
        search_paths.register_roots(&db);
        let project = Project::new(
            &db,
            "/project".into(),
            search_paths,
            Interpreter::Auto,
            None,
            Vec::new(),
            Vec::new(),
            TemplateDirs::Unknown,
            settings.tagspecs().clone(),
            TemplateLibraries::default(),
            ProjectTemplateFiles::default(),
        );
        db.set_project(project);

        assert!(
            project
                .search_paths(&db)
                .iter()
                .all(|search_path| search_path.path()
                    != Utf8Path::new("/project/.venv/lib/python3.12/site-packages"))
        );

        db.add_file(
            "/project/.venv/lib/python3.12/site-packages/blog/models.py",
            "from django.db import models\nclass VenvArticle(models.Model):\n    pass\n",
        );

        refresh_external_data(&mut db);

        assert!(project.search_paths(&db).iter().any(|search_path| {
            search_path.path() == Utf8Path::new("/project/.venv/lib/python3.12/site-packages")
        }));
        let graph = compute_model_graph(&db, project);
        assert!(graph.get("VenvArticle").is_some());
    }

    #[test]
    fn discover_external_model_files_finds_models() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("myapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("models.py"),
            r"
from django.db import models

class Article(models.Model):
    title = models.CharField(max_length=200)
    author = models.ForeignKey('auth.User', on_delete=models.CASCADE)
",
        )
        .unwrap();

        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "myapp.models");
        assert!(results[0].1.ends_with("models.py"));
    }

    #[test]
    fn discover_external_model_files_finds_files_without_inspecting_contents() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("emptyapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("models.py"), "# no models here\n").unwrap();

        // Discovery finds the file (it doesn't inspect contents)
        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "emptyapp.models");
    }

    #[test]
    fn discover_external_model_files_nested_apps() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        for app in &["blog", "accounts"] {
            let app_dir = root.join(app);
            std::fs::create_dir_all(&app_dir).unwrap();
            std::fs::write(
                app_dir.join("models.py"),
                format!(
                    "from django.db import models\nclass {name}Model(models.Model):\n    pass\n",
                    name = app.chars().next().unwrap().to_uppercase().to_string() + &app[1..]
                ),
            )
            .unwrap();
        }

        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
        assert_eq!(results.len(), 2);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(module_paths.contains(&"blog.models"));
        assert!(module_paths.contains(&"accounts.models"));
    }

    #[test]
    fn discover_model_files_workspace_finds_models() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("myapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("models.py"),
            "from django.db import models\nclass Foo(models.Model): pass\n",
        )
        .unwrap();

        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::Project);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "myapp.models");
        assert!(results[0].1.ends_with("models.py"));
    }

    #[test]
    fn discover_external_model_files_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let models_dir = root.join("myapp/models");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(
            models_dir.join("__init__.py"),
            "from .user import User\nfrom .order import Order\n",
        )
        .unwrap();
        std::fs::write(
            models_dir.join("user.py"),
            "from django.db import models\nclass User(models.Model):\n    pass\n",
        )
        .unwrap();
        std::fs::write(
            models_dir.join("order.py"),
            "from django.db import models\nclass Order(models.Model):\n    user = models.ForeignKey(User, on_delete=models.CASCADE)\n",
        )
        .unwrap();

        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
        // Discovers all three files (including __init__.py)
        assert_eq!(results.len(), 3);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(module_paths.contains(&"myapp.models"));
        assert!(module_paths.contains(&"myapp.models.user"));
        assert!(module_paths.contains(&"myapp.models.order"));
    }

    #[test]
    fn discover_workspace_models_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let models_dir = root.join("myapp/models");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(models_dir.join("__init__.py"), "").unwrap();
        std::fs::write(
            models_dir.join("user.py"),
            "from django.db import models\nclass User(models.Model): pass\n",
        )
        .unwrap();

        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::Project);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(
            module_paths.contains(&"myapp.models"),
            "should discover __init__.py as myapp.models"
        );
        assert!(
            module_paths.contains(&"myapp.models.user"),
            "should discover user.py as myapp.models.user"
        );
    }

    #[test]
    fn module_path_from_init_file() {
        let path = Utf8Path::new("myapp/models/__init__.py");
        assert_eq!(
            ModulePath::from_relative_path(path).as_str(),
            "myapp.models"
        );
    }

    #[test]
    fn module_path_from_submodule() {
        let path = Utf8Path::new("myapp/models/user.py");
        assert_eq!(
            ModulePath::from_relative_path(path).as_str(),
            "myapp.models.user"
        );
    }

    #[test]
    fn discover_workspace_models_nested_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let base_dir = root.join("myapp/models/base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(root.join("myapp/models/__init__.py"), "").unwrap();
        std::fs::write(base_dir.join("__init__.py"), "").unwrap();
        std::fs::write(
            base_dir.join("abstract.py"),
            "from django.db import models\nclass BaseModel(models.Model):\n    class Meta:\n        abstract = True\n",
        )
        .unwrap();

        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::Project);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(
            module_paths.contains(&"myapp.models.base.abstract"),
            "should discover nested model files: got {module_paths:?}"
        );
    }

    #[test]
    fn discover_external_model_files_nested_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let base_dir = root.join("myapp/models/base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(root.join("myapp/models/__init__.py"), "").unwrap();
        std::fs::write(base_dir.join("__init__.py"), "").unwrap();
        std::fs::write(
            base_dir.join("abstract.py"),
            "from django.db import models\nclass BaseModel(models.Model):\n    class Meta:\n        abstract = True\n",
        )
        .unwrap();

        let results =
            discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(
            module_paths.contains(&"myapp.models.base.abstract"),
            "should discover nested model files: got {module_paths:?}"
        );
    }

    #[test]
    fn project_model_discovery_skips_registered_non_first_party_paths() {
        let db = TestDatabase::new();
        db.add_file(
            "/project/app/models.py",
            "from django.db import models\nclass App(models.Model): pass\n",
        );
        db.add_file(
            "/project/venv/lib/python3.12/site-packages/somelib/models.py",
            "from django.db import models\nclass Lib(models.Model): pass\n",
        );

        let pythonpath = vec!["/project/venv/lib/python3.12/site-packages".to_string()];
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            Utf8Path::new("/project"),
            &Interpreter::InterpreterPath("/usr/bin/python".to_string()),
            &pythonpath,
        );
        search_paths.register_roots(&db);
        let project =
            project_for_search_paths(&db, "/project", search_paths, TemplateLibraries::default());

        let modules = model_modules(&db, project);
        let module_paths: Vec<_> = modules
            .iter()
            .map(|module| module.module_path().as_str())
            .collect();

        assert!(module_paths.contains(&"app.models"));
        assert!(module_paths.contains(&"somelib.models"));
        assert!(!module_paths.contains(&"venv.lib.python3.12.site-packages.somelib.models"));
    }
}
