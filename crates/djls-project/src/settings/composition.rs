use crate::django_environment_candidates;
use crate::project::Project;
use crate::python::python_source_model;
use crate::python::AssignmentKind;
use crate::python::ImportStatement;
use crate::python::PythonSourceOperation;
use crate::python::PythonSourceParseStatus;
use crate::python::QualifiedName;
use crate::python::StaticValue;
use crate::resolver::resolve_module;
use crate::resolver::ModuleResolutionError;
use crate::resolver::ModuleResolutionOutcome;
use crate::Db;
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::DjangoEnvironmentId;
use crate::PyModuleName;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DjangoSettings {
    installed_apps: PartialList<String>,
    templates: TemplateSettingsResolution,
}

impl DjangoSettings {
    #[must_use]
    pub fn installed_app_entries(&self) -> &PartialList<String> {
        &self.installed_apps
    }

    #[must_use]
    pub fn templates(&self) -> &TemplateSettingsResolution {
        &self.templates
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartialList<T> {
    segments: Vec<PartialListSegment<T>>,
}

impl<T> PartialList<T> {
    #[must_use]
    pub fn segments(&self) -> &[PartialListSegment<T>] {
        &self.segments
    }

    fn replace(&mut self, segments: Vec<PartialListSegment<T>>) {
        self.segments = segments;
    }

    fn extend(&mut self, segments: Vec<PartialListSegment<T>>) {
        self.segments.extend(segments);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PartialListSegment<T> {
    Known(T),
    Unknown,
}

impl<T> PartialListSegment<T> {
    fn known(value: T) -> Self {
        Self::Known(value)
    }

    fn unknown() -> Self {
        Self::Unknown
    }

    #[must_use]
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown => None,
        }
    }

    #[must_use]
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateSettingsResolution {
    backends: Vec<TemplateBackend>,
    has_unknown: bool,
}

impl TemplateSettingsResolution {
    #[must_use]
    pub fn backends(&self) -> &[TemplateBackend] {
        &self.backends
    }

    #[must_use]
    pub fn has_unknown(&self) -> bool {
        self.has_unknown
    }

    fn merge(&mut self, other: Self) {
        self.backends.extend(other.backends);
        self.has_unknown |= other.has_unknown;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateBackend {
    backend: Option<String>,
    dirs: PartialList<String>,
    app_dirs: Option<bool>,
    libraries: Vec<TemplateLibraryAlias>,
}

impl TemplateBackend {
    #[must_use]
    pub fn dirs(&self) -> &PartialList<String> {
        &self.dirs
    }

    #[must_use]
    pub fn app_dirs(&self) -> Option<bool> {
        self.app_dirs
    }

    #[must_use]
    pub fn libraries(&self) -> &[TemplateLibraryAlias] {
        &self.libraries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateLibraryAlias {
    name: String,
    module: PyModuleName,
}

impl TemplateLibraryAlias {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn module(&self) -> &PyModuleName {
        &self.module
    }
}

#[salsa::tracked(returns(ref))]
#[allow(clippy::needless_pass_by_value)]
pub fn django_settings(db: &dyn Db, project: Project, env: DjangoEnvironmentId) -> DjangoSettings {
    let Some(settings_module) = settings_module_for_env(db, project, &env) else {
        return DjangoSettings::default();
    };
    let mut visited = Vec::new();
    django_settings_for_module(db, project, &settings_module, &mut visited)
}

fn settings_module_for_env(
    db: &dyn Db,
    project: Project,
    env: &DjangoEnvironmentId,
) -> Option<PyModuleName> {
    let candidates = match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Ready(candidates) => candidates,
        DjangoEnvironmentCandidatesOutcome::Deferred => return None,
    };
    candidates
        .iter()
        .find(|candidate| candidate.id() == env)
        .map(|candidate| candidate.settings().clone())
}

fn django_settings_for_module(
    db: &dyn Db,
    project: Project,
    module: &PyModuleName,
    visited: &mut Vec<PyModuleName>,
) -> DjangoSettings {
    if visited.contains(module) {
        return DjangoSettings::default();
    }
    visited.push(module.clone());

    let file = match resolve_module(db, project, module.clone()).outcome() {
        ModuleResolutionOutcome::Resolved(resolved) => resolved.location().file(),
        ModuleResolutionOutcome::Unresolved(
            ModuleResolutionError::MultipleCandidates(_)
            | ModuleResolutionError::RootUnavailable(_)
            | ModuleResolutionError::NoImportRoots
            | ModuleResolutionError::NotFound
            | ModuleResolutionError::UnsupportedModuleName,
        ) => return DjangoSettings::default(),
    };

    let model = python_source_model(db, file);
    if !matches!(model.parse_status(), PythonSourceParseStatus::Parsed) {
        return DjangoSettings::default();
    }

    let mut settings = DjangoSettings::default();
    apply_settings_operations(
        db,
        project,
        module,
        &mut settings,
        model.operations(),
        visited,
    );
    settings
}

impl DjangoSettings {
    fn merge(&mut self, other: DjangoSettings) {
        self.installed_apps.extend(other.installed_apps.segments);
        self.templates.merge(other.templates);
    }
}

fn imported_settings_module(
    current_module: &PyModuleName,
    import: &ImportStatement,
) -> Option<PyModuleName> {
    match import {
        ImportStatement::Import { module, .. } => PyModuleName::parse(&module.as_dotted()).ok(),
        ImportStatement::ImportFrom {
            module,
            name,
            level,
            ..
        } if name == "*" => import_from_module_name(current_module, module.as_ref(), *level),
        ImportStatement::ImportFrom { .. } => None,
    }
}

fn import_from_module_name(
    current_module: &PyModuleName,
    module: Option<&QualifiedName>,
    level: u32,
) -> Option<PyModuleName> {
    if level == 0 {
        return PyModuleName::parse(&module?.as_dotted()).ok();
    }
    let mut parts = current_module.as_str().split('.').collect::<Vec<_>>();
    parts.pop();
    for _ in 1..level {
        parts.pop()?;
    }
    if let Some(module) = module {
        for part in module.parts() {
            parts.push(part);
        }
    }
    PyModuleName::parse(&parts.join(".")).ok()
}

fn apply_settings_operations(
    db: &dyn Db,
    project: Project,
    current_module: &PyModuleName,
    settings: &mut DjangoSettings,
    operations: &[PythonSourceOperation],
    visited: &mut Vec<PyModuleName>,
) {
    for operation in operations {
        match operation {
            PythonSourceOperation::Import(import) => {
                if let Some(imported) = imported_settings_module(current_module, import) {
                    let imported_settings =
                        django_settings_for_module(db, project, &imported, visited);
                    settings.merge(imported_settings);
                }
            }
            PythonSourceOperation::Assignment(assignment) => {
                let target_names = assignment
                    .targets()
                    .iter()
                    .map(|target| target.name().as_dotted())
                    .collect::<Vec<_>>();
                if target_names.iter().any(|target| target == "INSTALLED_APPS") {
                    let segments = partial_string_list_from_value(assignment.value());
                    match assignment.kind() {
                        AssignmentKind::Assign => settings.installed_apps.replace(segments),
                        AssignmentKind::AugAdd => settings.installed_apps.extend(segments),
                    }
                } else if target_names.iter().any(|target| target == "TEMPLATES") {
                    settings.templates = template_settings_from_value(assignment.value());
                }
            }
            PythonSourceOperation::Call(call) => {
                let Some(callee) = call.callee().map(crate::python::QualifiedName::as_dotted)
                else {
                    continue;
                };
                match callee.as_str() {
                    "INSTALLED_APPS.append" => {
                        if let Some(StaticValue::String(value)) = call.arguments().first() {
                            settings
                                .installed_apps
                                .extend(vec![PartialListSegment::known(value.clone())]);
                        }
                    }
                    "INSTALLED_APPS.extend" => {
                        if let Some(value) = call.arguments().first() {
                            settings
                                .installed_apps
                                .extend(partial_string_list_from_value(value));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn partial_string_list_from_value(value: &StaticValue) -> Vec<PartialListSegment<String>> {
    match value {
        StaticValue::StringList(segments) => segments
            .iter()
            .map(|segment| match segment.value() {
                Some(value) => PartialListSegment::known(value.clone()),
                None => PartialListSegment::unknown(),
            })
            .collect(),
        _ => vec![PartialListSegment::unknown()],
    }
}

fn template_settings_from_value(value: &StaticValue) -> TemplateSettingsResolution {
    let mut resolution = TemplateSettingsResolution::default();
    match value {
        StaticValue::List(backends) => {
            for backend in backends {
                match backend {
                    StaticValue::Dict(entries) => {
                        resolution
                            .backends
                            .push(template_backend_from_dict(entries));
                    }
                    _ => resolution.has_unknown = true,
                }
            }
        }
        _ => resolution.has_unknown = true,
    }
    resolution
}

fn template_backend_from_dict(entries: &[(String, StaticValue)]) -> TemplateBackend {
    let mut backend = TemplateBackend {
        backend: None,
        dirs: PartialList::default(),
        app_dirs: None,
        libraries: Vec::new(),
    };
    for (key, value) in entries {
        match (key.as_str(), value) {
            ("BACKEND", StaticValue::String(name)) => backend.backend = Some(name.clone()),
            ("DIRS", value) => {
                backend.dirs = PartialList {
                    segments: partial_string_list_from_value(value),
                }
            }
            ("APP_DIRS", StaticValue::Bool(value)) => backend.app_dirs = Some(*value),
            ("OPTIONS", StaticValue::Dict(options)) => {
                backend.libraries = template_libraries_from_options(options);
            }
            _ => {}
        }
    }
    backend
}

fn template_libraries_from_options(options: &[(String, StaticValue)]) -> Vec<TemplateLibraryAlias> {
    options
        .iter()
        .find_map(|(key, value)| match (key.as_str(), value) {
            ("libraries", StaticValue::Dict(libraries)) => Some(libraries),
            _ => None,
        })
        .into_iter()
        .flat_map(|libraries| libraries.iter())
        .filter_map(|(name, value)| match value {
            StaticValue::String(module) => Some(TemplateLibraryAlias {
                name: name.clone(),
                module: PyModuleName::parse(module).ok()?,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

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
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::enrichment::ProjectEnrichment;
    use crate::root_discovery::ProjectEnvVars;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::root_discovery::ProjectRootDiscoverySet;
    use crate::root_discovery::RootDiscoveryInput;
    use crate::source_files::ReadySourceFiles;
    use crate::source_files::SourceFileInventory;
    use crate::source_files::SourceFilesIssue;

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

        fn set_file(&mut self, path: &str, source: &str) -> File {
            let path = Utf8PathBuf::from(path);
            self.sources.insert(path.clone(), source.to_string());
            self.get_or_create_file(path.as_path())
        }
    }

    fn ready_inventory(db: &TestDb, paths: &[&str]) -> SourceFileInventory {
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path, FileRootKind::Project);
        let roots = vec![SourceRootEntry::new(root)];
        let files = paths
            .iter()
            .map(|path| {
                let path = Utf8PathBuf::from(path);
                LoadedSourceFile::new(path.clone(), root_id.clone(), db.get_or_create_file(&path))
            })
            .collect::<Vec<_>>();
        let data = SourceFileSetData::new(roots, files).expect("test data should be valid");
        SourceFileInventory::Ready(ReadySourceFiles::new(
            crate::source_files::SourceFileSetPartitions::default(),
            SourceFileSet::new(db, data),
        ))
    }

    fn discovery(db: &TestDb, settings_module: &str) -> ProjectRootDiscovery {
        let root = RootDiscoveryInput::new(
            db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(settings_module.to_string()),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        ProjectRootDiscovery::Ready(
            ProjectRootDiscoverySet::new(vec![root]).expect("root should create discovery"),
        )
    }

    fn single_env_id(db: &TestDb) -> DjangoEnvironmentId {
        let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
            django_environment_candidates(db, db.project())
        else {
            panic!("single candidate should be ready");
        };
        candidates[0].id().clone()
    }

    #[test]
    fn django_settings_preserve_installed_apps_order_with_unknown_gaps() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = ['django.contrib.auth', UNKNOWN, 'blog']\n",
        );
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/project/settings.py"]));
        db.set_project_root_discovery(discovery(&db, "project.settings"));
        let env = single_env_id(&db);

        let settings = django_settings(&db, db.project(), env);
        let segments = settings.installed_app_entries().segments();

        assert_eq!(
            segments[0].value(),
            Some(&"django.contrib.auth".to_string())
        );
        assert!(segments[1].is_unknown());
        assert_eq!(segments[2].value(), Some(&"blog".to_string()));
    }

    #[test]
    fn django_settings_extract_template_backend_settings() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates'], 'APP_DIRS': True}]\n",
        );
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/project/settings.py"]));
        db.set_project_root_discovery(discovery(&db, "project.settings"));
        let env = single_env_id(&db);

        let settings = django_settings(&db, db.project(), env);
        let backend = &settings.templates().backends()[0];

        assert_eq!(
            backend.backend.as_deref(),
            Some("django.template.backends.django.DjangoTemplates")
        );
        assert_eq!(
            backend.dirs().segments()[0].value(),
            Some(&"templates".to_string())
        );
        assert_eq!(backend.app_dirs(), Some(true));
    }

    #[test]
    fn django_settings_apply_operations_in_source_order() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = []\nINSTALLED_APPS.append('temporary')\nINSTALLED_APPS = ['final']\n",
        );
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/project/settings.py"]));
        db.set_project_root_discovery(discovery(&db, "project.settings"));
        let env = single_env_id(&db);

        let settings = django_settings(&db, db.project(), env);
        let values = settings
            .installed_app_entries()
            .segments()
            .iter()
            .filter_map(PartialListSegment::value)
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["final"]);
    }

    #[test]
    fn django_settings_support_relative_star_imports() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/base.py",
            "INSTALLED_APPS = ['base_app']\n",
        );
        db.set_file(
            "/workspace/project/settings.py",
            "from .base import *\nINSTALLED_APPS += ['local_app']\n",
        );
        db.set_source_file_inventory(ready_inventory(
            &db,
            &[
                "/workspace/project/base.py",
                "/workspace/project/settings.py",
            ],
        ));
        db.set_project_root_discovery(discovery(&db, "project.settings"));
        let env = single_env_id(&db);

        let settings = django_settings(&db, db.project(), env);
        let values = settings
            .installed_app_entries()
            .segments()
            .iter()
            .filter_map(PartialListSegment::value)
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["base_app", "local_app"]);
    }

    #[test]
    fn django_settings_preserve_known_concat_tail_after_unknown_prefix() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = BASE_APPS + ['local_app']\n",
        );
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/project/settings.py"]));
        db.set_project_root_discovery(discovery(&db, "project.settings"));
        let env = single_env_id(&db);

        let settings = django_settings(&db, db.project(), env);
        let segments = settings.installed_app_entries().segments();

        assert!(segments[0].is_unknown());
        assert_eq!(segments[1].value(), Some(&"local_app".to_string()));
    }

    #[test]
    fn django_settings_imports_apply_before_current_file_appends() {
        let mut db = TestDb::with_project();
        db.set_file("/workspace/base.py", "INSTALLED_APPS = ['base_app']\n");
        db.set_file(
            "/workspace/project/settings.py",
            "import base\nINSTALLED_APPS = ['local_app'] + ['concat_app']\nINSTALLED_APPS += ['aug_app']\nINSTALLED_APPS.append('tail_app')\n",
        );
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace/base.py", "/workspace/project/settings.py"],
        ));
        db.set_project_root_discovery(discovery(&db, "project.settings"));
        let env = single_env_id(&db);

        let settings = django_settings(&db, db.project(), env);
        let values = settings
            .installed_app_entries()
            .segments()
            .iter()
            .filter_map(PartialListSegment::value)
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(
            values,
            vec!["local_app", "concat_app", "aug_app", "tail_app"]
        );
    }
}
