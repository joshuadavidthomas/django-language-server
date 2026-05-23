use std::collections::BTreeSet;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;

use crate::apps::installed_apps;
use crate::apps::InstalledAppResolution;
use crate::project::Project;
use crate::resolver::resolve_module;
use crate::resolver::ModuleResolutionError;
use crate::resolver::ModuleResolutionOutcome;
use crate::settings::django_settings;
use crate::settings::SettingsIssue;
use crate::source_files::FileSetPartitionId;
use crate::source_files::SourceFileInventory;
use crate::source_files::SourceFilePartitionReadiness;
use crate::source_files::SourceFilesIssue;
use crate::Db;
use crate::DjangoEnvironmentId;
use crate::LibraryName;
use crate::PyModuleName;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateDirectory {
    path: Utf8PathBuf,
    source: TemplateDirectorySource,
}

impl TemplateDirectory {
    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        self.path.as_path()
    }

    #[must_use]
    pub fn source(&self) -> &TemplateDirectorySource {
        &self.source
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateDirectorySource {
    SettingsDirs,
    InstalledApp { entry: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateDirectoryEntry {
    Discovered(TemplateDirectory),
    UnknownSettingsDir {
        issue: SettingsIssue,
    },
    Deferred {
        directory: TemplateDirectory,
    },
    Unavailable {
        directory: TemplateDirectory,
        issue: SourceFilesIssue,
    },
    Stale {
        directory: TemplateDirectory,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateDirectoryInventory {
    entries: Vec<TemplateDirectoryEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectTemplate {
    path: Utf8PathBuf,
    name: String,
    file: File,
    directory: TemplateDirectory,
}

impl ProjectTemplate {
    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        self.path.as_path()
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }

    #[must_use]
    pub fn directory(&self) -> &TemplateDirectory {
        &self.directory
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateFileInventory {
    templates: Vec<ProjectTemplate>,
    directories: Vec<TemplateDirectoryEntry>,
}

impl TemplateFileInventory {
    #[must_use]
    pub fn templates(&self) -> &[ProjectTemplate] {
        &self.templates
    }

    #[must_use]
    pub fn directories(&self) -> &[TemplateDirectoryEntry] {
        &self.directories
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateTagLibrary {
    name: String,
    source: TemplateTagLibrarySource,
    resolution: TemplateTagLibraryResolution,
}

impl TemplateTagLibrary {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn resolution(&self) -> &TemplateTagLibraryResolution {
        &self.resolution
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateTagLibrarySource {
    DjangoBuiltin,
    InstalledApp,
    SettingsLibraries,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateTagLibraryResolution {
    Resolved { file: File },
    Builtin,
    Unresolved { issue: TemplateTagLibraryIssue },
    Ambiguous { issue: TemplateTagLibraryIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateTagLibraryIssue {
    NotFound { module: PyModuleName },
    Ambiguous { module: PyModuleName },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateTagLibraryInventory {
    libraries: Vec<TemplateTagLibrary>,
}

impl TemplateTagLibraryInventory {
    #[must_use]
    pub fn libraries(&self) -> &[TemplateTagLibrary] {
        &self.libraries
    }

    #[must_use]
    pub(crate) fn resolved_files(&self) -> Vec<File> {
        self.libraries
            .iter()
            .filter_map(|library| match library.resolution() {
                TemplateTagLibraryResolution::Resolved { file } => Some(*file),
                TemplateTagLibraryResolution::Builtin
                | TemplateTagLibraryResolution::Unresolved { .. }
                | TemplateTagLibraryResolution::Ambiguous { .. } => None,
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadableTemplateLibraryInventory {
    libraries: Vec<LoadableTemplateLibrary>,
}

impl LoadableTemplateLibraryInventory {
    #[must_use]
    pub fn libraries(&self) -> &[LoadableTemplateLibrary] {
        &self.libraries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadableTemplateLibrary {
    name: LibraryName,
    module: Option<PyModuleName>,
    source: LoadableTemplateLibrarySource,
}

impl LoadableTemplateLibrary {
    #[must_use]
    pub fn name(&self) -> &LibraryName {
        &self.name
    }

    #[must_use]
    pub fn module(&self) -> Option<&PyModuleName> {
        self.module.as_ref()
    }

    #[must_use]
    pub fn source(&self) -> &LoadableTemplateLibrarySource {
        &self.source
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadableTemplateLibrarySource {
    Static,
    Runtime,
}

#[salsa::tracked(returns(ref))]
pub fn template_directories(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> TemplateDirectoryInventory {
    TemplateDirectoryInventory {
        entries: template_directory_entries(db, project, env),
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_files(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> TemplateFileInventory {
    let directories = template_directory_entries(db, project, env);
    let mut templates = Vec::new();
    let SourceFileInventory::Ready(files) = project.source_inventory(db) else {
        return TemplateFileInventory {
            templates,
            directories: directories
                .into_iter()
                .map(defer_discovered_directory)
                .collect(),
        };
    };
    let data = files.merged().data(db);
    let loaded_roots = data
        .roots()
        .iter()
        .map(|entry| entry.root().path().to_owned())
        .collect::<Vec<_>>();
    let directories = directories
        .into_iter()
        .map(|entry| directory_entry_with_readiness(&files, &loaded_roots, entry))
        .collect::<Vec<_>>();

    for entry in &directories {
        let TemplateDirectoryEntry::Discovered(directory) = entry else {
            continue;
        };
        for file in data.files() {
            if file.path().starts_with(directory.path()) && is_template_file(file.path()) {
                let name = file
                    .path()
                    .strip_prefix(directory.path())
                    .unwrap_or(file.path())
                    .as_str()
                    .trim_start_matches('/')
                    .to_string();
                templates.push(ProjectTemplate {
                    path: file.path().to_owned(),
                    name,
                    file: file.file(),
                    directory: directory.clone(),
                });
            }
        }
    }

    TemplateFileInventory {
        templates,
        directories,
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_tag_libraries(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> TemplateTagLibraryInventory {
    let mut libraries = django_builtin_libraries();
    let SourceFileInventory::Ready(files) = project.source_inventory(db) else {
        return TemplateTagLibraryInventory { libraries };
    };
    for root in installed_app_roots(db, project, env.clone()) {
        let tag_root = root.join("templatetags");
        for file in files.merged().data(db).files() {
            if file.path().parent() == Some(tag_root.as_path())
                && file.path().extension() == Some("py")
                && file.path().file_name() != Some("__init__.py")
            {
                let name = file
                    .path()
                    .file_stem()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                libraries.push(TemplateTagLibrary {
                    name,
                    source: TemplateTagLibrarySource::InstalledApp,
                    resolution: TemplateTagLibraryResolution::Resolved { file: file.file() },
                });
            }
        }
    }
    let settings = django_settings(db, project, env);
    for backend in settings.templates().backends() {
        for alias in backend.libraries() {
            libraries.push(TemplateTagLibrary {
                name: alias.name().to_string(),
                source: TemplateTagLibrarySource::SettingsLibraries,
                resolution: resolve_template_library_module(db, project, alias.module().clone()),
            });
        }
    }
    TemplateTagLibraryInventory { libraries }
}

#[salsa::tracked(returns(ref))]
pub fn loadable_template_libraries(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> LoadableTemplateLibraryInventory {
    let static_inventory = template_tag_libraries(db, project, env);
    let mut libraries = Vec::new();
    let mut known_names = BTreeSet::new();

    for library in static_inventory.libraries() {
        if matches!(
            library.resolution(),
            TemplateTagLibraryResolution::Unresolved { .. }
                | TemplateTagLibraryResolution::Ambiguous { .. }
        ) {
            continue;
        }
        let Ok(name) = LibraryName::parse(library.name()) else {
            continue;
        };
        known_names.insert(name.clone());
        libraries.push(LoadableTemplateLibrary {
            name,
            module: None,
            source: LoadableTemplateLibrarySource::Static,
        });
    }

    let crate::ProjectEnrichment::Fresh(template_libraries) = project.enrichment(db) else {
        return LoadableTemplateLibraryInventory { libraries };
    };
    for (name, module) in template_libraries {
        if known_names.insert(name.clone()) {
            libraries.push(LoadableTemplateLibrary {
                name: name.clone(),
                module: Some(module.clone()),
                source: LoadableTemplateLibrarySource::Runtime,
            });
        }
    }

    LoadableTemplateLibraryInventory { libraries }
}

fn directory_entry_with_readiness(
    files: &crate::ReadySourceFiles,
    loaded_roots: &[Utf8PathBuf],
    entry: TemplateDirectoryEntry,
) -> TemplateDirectoryEntry {
    let TemplateDirectoryEntry::Discovered(directory) = entry else {
        return entry;
    };
    let partition_readiness = match directory.source() {
        TemplateDirectorySource::SettingsDirs => {
            files.root_readiness_for_partition(directory.path(), |partition| {
                matches!(
                    partition,
                    FileSetPartitionId::ConfiguredTemplateDirectory(_)
                )
            })
        }
        TemplateDirectorySource::InstalledApp { .. } => files
            .root_readiness_for_partition(directory.path(), |partition| {
                matches!(partition, FileSetPartitionId::InstalledApp(_))
            }),
    };
    match partition_readiness {
        Some(SourceFilePartitionReadiness::Ready { .. }) => {
            TemplateDirectoryEntry::Discovered(directory)
        }
        None if directory_fallback_loaded(files, &directory, loaded_roots) => {
            TemplateDirectoryEntry::Discovered(directory)
        }
        Some(
            SourceFilePartitionReadiness::Deferred { .. } | SourceFilePartitionReadiness::Loading,
        )
        | None => TemplateDirectoryEntry::Deferred { directory },
        Some(
            SourceFilePartitionReadiness::Unavailable { issue, .. }
            | SourceFilePartitionReadiness::Skipped { issue, .. },
        ) => TemplateDirectoryEntry::Unavailable { directory, issue },
        Some(SourceFilePartitionReadiness::Stale { .. }) => {
            TemplateDirectoryEntry::Stale { directory }
        }
    }
}

fn directory_fallback_loaded(
    files: &crate::ReadySourceFiles,
    directory: &TemplateDirectory,
    loaded_roots: &[Utf8PathBuf],
) -> bool {
    !files.has_partition_readiness()
        && matches!(directory.source(), TemplateDirectorySource::SettingsDirs)
        && loaded_roots.iter().any(|root| root == directory.path())
}

fn defer_discovered_directory(entry: TemplateDirectoryEntry) -> TemplateDirectoryEntry {
    match entry {
        TemplateDirectoryEntry::Discovered(directory) => {
            TemplateDirectoryEntry::Deferred { directory }
        }
        other => other,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn template_directory_entries(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> Vec<TemplateDirectoryEntry> {
    let settings = django_settings(db, project, env.clone());
    let mut entries = Vec::new();
    for backend in settings.templates().backends() {
        for segment in backend.dirs().segments() {
            match (segment.value(), segment.issue()) {
                (Some(path), None) => {
                    entries.push(TemplateDirectoryEntry::Discovered(TemplateDirectory {
                        path: Utf8PathBuf::from(path),
                        source: TemplateDirectorySource::SettingsDirs,
                    }));
                }
                (_, Some(issue)) => entries.push(TemplateDirectoryEntry::UnknownSettingsDir {
                    issue: issue.clone(),
                }),
                (None, None) => {}
            }
        }
        if backend.app_dirs() == Some(true) {
            for app in installed_apps(db, project, env.clone()) {
                if let Some(path) = installed_app_template_dir(db, app.resolution()) {
                    entries.push(TemplateDirectoryEntry::Discovered(TemplateDirectory {
                        path,
                        source: TemplateDirectorySource::InstalledApp {
                            entry: app.entry().to_string(),
                        },
                    }));
                }
            }
        }
    }
    entries
}

fn installed_app_roots(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> Vec<Utf8PathBuf> {
    installed_apps(db, project, env)
        .iter()
        .filter_map(|app| match app.resolution() {
            InstalledAppResolution::Package { file, .. } => app_root_for_file(db, *file),
            InstalledAppResolution::AppConfig { config, file } => config
                .path()
                .map(Utf8Path::to_owned)
                .or_else(|| app_root_for_file(db, *file)),
            InstalledAppResolution::Unresolved(_) => None,
        })
        .collect()
}

fn installed_app_template_dir(
    db: &dyn Db,
    resolution: &InstalledAppResolution,
) -> Option<Utf8PathBuf> {
    let root = match resolution {
        InstalledAppResolution::Package { file, .. } => app_root_for_file(db, *file)?,
        InstalledAppResolution::AppConfig { config, file } => config
            .path()
            .map(Utf8Path::to_owned)
            .or_else(|| app_root_for_file(db, *file))?,
        InstalledAppResolution::Unresolved(_) => return None,
    };
    Some(root.join("templates"))
}

fn app_root_for_file(db: &dyn Db, file: File) -> Option<Utf8PathBuf> {
    let path = file.path(db);
    let parent = path.parent()?;
    if path.file_name() == Some("__init__.py") || path.file_name() == Some("apps.py") {
        return Some(parent.to_owned());
    }
    parent.parent().map(Utf8Path::to_owned)
}

fn resolve_template_library_module(
    db: &dyn Db,
    project: Project,
    module: PyModuleName,
) -> TemplateTagLibraryResolution {
    match resolve_module(db, project, module.clone()).outcome() {
        ModuleResolutionOutcome::Resolved(resolved) => TemplateTagLibraryResolution::Resolved {
            file: resolved.location().file(),
        },
        ModuleResolutionOutcome::Unresolved(ModuleResolutionError::MultipleCandidates(_)) => {
            TemplateTagLibraryResolution::Ambiguous {
                issue: TemplateTagLibraryIssue::Ambiguous { module },
            }
        }
        ModuleResolutionOutcome::Unresolved(
            ModuleResolutionError::NoImportRoots
            | ModuleResolutionError::RootUnavailable(_)
            | ModuleResolutionError::NotFound
            | ModuleResolutionError::UnsupportedModuleName,
        ) => TemplateTagLibraryResolution::Unresolved {
            issue: TemplateTagLibraryIssue::NotFound { module },
        },
    }
}

fn django_builtin_libraries() -> Vec<TemplateTagLibrary> {
    ["cache", "i18n", "l10n", "static", "tz"]
        .into_iter()
        .map(|name| TemplateTagLibrary {
            name: name.to_string(),
            source: TemplateTagLibrarySource::DjangoBuiltin,
            resolution: TemplateTagLibraryResolution::Builtin,
        })
        .collect()
}

fn is_template_file(path: &Utf8Path) -> bool {
    matches!(
        path.extension(),
        Some("html" | "htm" | "txt" | "jinja" | "jinja2")
    )
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
    use rustc_hash::FxHashMap;
    use salsa::Setter;

    use super::*;
    use crate::django_environment_candidates;
    use crate::enrichment::ProjectEnrichment;
    use crate::root_discovery::DjangoSettingsModuleSeed;
    use crate::root_discovery::ProjectEnvVars;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::root_discovery::ProjectRootDiscoverySet;
    use crate::root_discovery::RootDiscoveryInput;
    use crate::source_files::ReadySourceFiles;
    use crate::source_files::SourceFilesIssue;
    use crate::DjangoEnvironmentCandidatesOutcome;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        sources: FxHashMap<Utf8PathBuf, String>,
        project: OnceLock<Project>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            Ok(self.sources.get(path).cloned().unwrap_or_default())
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
                    SourceFileInventory::Unavailable {
                        issue: SourceFilesIssue::NotLoaded,
                    },
                    ProjectRootDiscovery::Absent,
                    ProjectEnrichment::Absent,
                ))
                .expect("project should initialize once");
            db
        }

        fn set_file(&mut self, path: &str, source: &str) {
            self.sources
                .insert(Utf8PathBuf::from(path), source.to_string());
        }
    }

    fn ready_inventory(db: &TestDb, roots: &[&str], paths: &[&str]) -> SourceFileInventory {
        let roots = roots
            .iter()
            .map(|root| {
                let root_path = Utf8PathBuf::from(root);
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
            .iter()
            .map(|path| {
                let path = Utf8PathBuf::from(path);
                let root = roots
                    .iter()
                    .find(|root| path.starts_with(root.path()))
                    .expect("file should belong to a root");
                LoadedSourceFile::new(
                    path.clone(),
                    root.id().clone(),
                    db.get_or_create_file(&path),
                )
            })
            .collect::<Vec<_>>();
        SourceFileInventory::Ready(ReadySourceFiles::new(
            crate::source_files::SourceFileSetPartitions::default(),
            SourceFileSet::new(
                db,
                SourceFileSetData::new(root_entries, files).expect("test data should be valid"),
            ),
        ))
    }

    fn discovery(db: &TestDb) -> ProjectRootDiscovery {
        let root = RootDiscoveryInput::new(
            db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(DjangoSettingsModuleSeed::new("project.settings")),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        ProjectRootDiscovery::Ready(
            ProjectRootDiscoverySet::new(vec![root]).expect("root should create discovery"),
        )
    }

    fn env(db: &TestDb) -> DjangoEnvironmentId {
        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
            django_environment_candidates(db, db.project())
        else {
            panic!("environment should be ready");
        };
        candidates[0].id().clone()
    }

    #[test]
    fn template_inventory_configured_directory_is_deferred_until_loaded() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'DIRS': ['/workspace/emails']}]\n",
        );
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &["/workspace/project/settings.py"],
        ));
        let env = env(&db);

        let inventory = template_files(&db, db.project(), env);

        assert!(matches!(
            inventory.directories()[0],
            TemplateDirectoryEntry::Deferred { .. }
        ));
    }

    #[test]
    fn template_inventory_lists_loaded_configured_directory_files() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'DIRS': ['/workspace/emails']}]\n",
        );
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace", "/workspace/emails"],
            &[
                "/workspace/project/settings.py",
                "/workspace/emails/welcome.html",
            ],
        ));
        let env = env(&db);

        let inventory = template_files(&db, db.project(), env);

        assert_eq!(inventory.templates()[0].name(), "welcome.html");
    }

    #[test]
    fn template_inventory_preserves_unknown_settings_dir_segments() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'DIRS': [UNKNOWN]}]\n",
        );
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &["/workspace/project/settings.py"],
        ));
        let env = env(&db);

        let inventory = template_directories(&db, db.project(), env);

        assert!(matches!(
            inventory.entries[0],
            TemplateDirectoryEntry::UnknownSettingsDir { .. }
        ));
    }

    #[test]
    fn template_inventory_loaded_empty_directory_has_no_templates_but_is_not_deferred() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'DIRS': ['/workspace/partials']}]\n",
        );
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace", "/workspace/partials"],
            &["/workspace/project/settings.py"],
        ));
        let env = env(&db);

        let inventory = template_files(&db, db.project(), env);

        assert!(inventory.templates().is_empty());
        assert!(matches!(
            inventory.directories()[0],
            TemplateDirectoryEntry::Discovered(_)
        ));
    }

    #[test]
    fn template_inventory_includes_builtin_and_installed_tag_libraries() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'APP_DIRS': True}]\nINSTALLED_APPS = ['blog']\n",
        );
        db.set_file("/workspace/blog/__init__.py", "");
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &[
                "/workspace/project/settings.py",
                "/workspace/blog/__init__.py",
                "/workspace/blog/templatetags/blog_tags.py",
            ],
        ));
        let env = env(&db);

        let inventory = template_tag_libraries(&db, db.project(), env);
        let names = inventory
            .libraries()
            .iter()
            .map(TemplateTagLibrary::name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"static"));
        assert!(names.contains(&"blog_tags"));
    }

    #[test]
    fn template_inventory_includes_static_settings_libraries() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'OPTIONS': {'libraries': {'ui': 'blog.templatetags.ui'}}}]\n",
        );
        db.set_file("/workspace/blog/templatetags/ui.py", "");
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &[
                "/workspace/project/settings.py",
                "/workspace/blog/templatetags/ui.py",
            ],
        ));
        let env = env(&db);

        let inventory = template_tag_libraries(&db, db.project(), env);
        let library = inventory
            .libraries()
            .iter()
            .find(|library| {
                library.name() == "ui"
                    && matches!(&library.source, TemplateTagLibrarySource::SettingsLibraries)
            })
            .expect("settings library should be present");

        assert!(matches!(
            &library.source,
            TemplateTagLibrarySource::SettingsLibraries
        ));
        assert!(matches!(
            library.resolution(),
            TemplateTagLibraryResolution::Resolved { .. }
        ));
    }

    #[test]
    fn loadable_template_libraries_include_runtime_fallbacks() {
        let mut db = TestDb::with_project();
        db.set_file("/workspace/project/settings.py", "TEMPLATES = [{}]\n");
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &["/workspace/project/settings.py"],
        ));
        db.project()
            .set_enrichment(&mut db)
            .to(ProjectEnrichment::Fresh(std::collections::BTreeMap::from(
                [(
                    LibraryName::parse("runtime_ui").unwrap(),
                    PyModuleName::parse("blog.templatetags.runtime_ui").unwrap(),
                )],
            )));
        let env = env(&db);

        let inventory = loadable_template_libraries(&db, db.project(), env);
        let library = inventory
            .libraries()
            .iter()
            .find(|library| library.name().as_str() == "runtime_ui")
            .expect("runtime library should fill a static gap");

        assert_eq!(
            library.module().map(PyModuleName::as_str),
            Some("blog.templatetags.runtime_ui")
        );
        assert_eq!(library.source(), &LoadableTemplateLibrarySource::Runtime);
    }

    #[test]
    fn loadable_template_libraries_prefer_static_facts_over_runtime_hints() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'OPTIONS': {'libraries': {'ui': 'blog.templatetags.ui'}}}]\n",
        );
        db.set_file("/workspace/blog/templatetags/ui.py", "");
        db.set_project_root_discovery(discovery(&db));
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace"],
            &[
                "/workspace/project/settings.py",
                "/workspace/blog/templatetags/ui.py",
            ],
        ));
        db.project()
            .set_enrichment(&mut db)
            .to(ProjectEnrichment::Fresh(std::collections::BTreeMap::from(
                [(
                    LibraryName::parse("ui").unwrap(),
                    PyModuleName::parse("runtime.templatetags.ui").unwrap(),
                )],
            )));
        let env = env(&db);

        let inventory = loadable_template_libraries(&db, db.project(), env);
        let ui_libraries = inventory
            .libraries()
            .iter()
            .filter(|library| library.name().as_str() == "ui")
            .collect::<Vec<_>>();

        assert_eq!(ui_libraries.len(), 1);
        assert_eq!(ui_libraries[0].module(), None);
        assert_eq!(
            ui_libraries[0].source(),
            &LoadableTemplateLibrarySource::Static
        );
    }
}
