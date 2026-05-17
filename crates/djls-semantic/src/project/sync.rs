//! Synchronize external project state into Salsa inputs.
//!
//! This module is the imperative boundary for project data. It may ask
//! Django, Python, and the filesystem for facts, then writes changed facts to
//! the `Project` input. Pure semantic derivation stays in tracked queries.

use std::fmt::Write;
use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::Utf8PathClean;
use ignore::WalkBuilder;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use salsa::Setter;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use super::app_registry::resolve_app_registry;
use super::db::Db as ProjectDb;
use super::facts::Fact;
use super::facts::Field;
use super::facts::ModuleLocation;
use super::facts::Reason;
use super::facts::ReasonSource;
use super::input::Project;
use super::input::ProjectPythonIndex;
use super::input::ProjectPythonModule;
use super::input::ProjectTemplateFile;
use super::input::ProjectTemplateFiles;
use super::input::TemplateDirs;
use super::introspector::IntrospectionRequest;
use super::module_resolver::discover_module_search_paths as discover_static_module_search_paths;
use super::module_resolver::resolve_module as resolve_static_module;
use super::names::PyModuleName;
use super::python::Interpreter;
use super::resolve::build_search_paths;
use super::resolve::discover_model_files;
use super::resolve::find_site_packages;
use super::resolve::resolve_modules;
use super::resolve::ResolvedModule;
use super::settings_facts::extract_settings_facts_for_module;
use super::symbols::TemplateLibrarySnapshot;
use super::template_libraries::assemble_template_libraries;
use super::template_symbols::assemble_template_library_snapshot;
use super::template_symbols::assemble_template_symbols;
use crate::python::extract_model_graph;
use crate::python::extract_rules;
use crate::python::BlockSpecs;
use crate::python::ExtractionResult;
use crate::python::FilterArityMap;
use crate::python::ModelGraph;
use crate::python::ModulePath;
use crate::python::TagRuleMap;

/// Refresh all external project data.
///
/// This is the imperative boundary between the outside world and Salsa inputs:
/// it asks Django/Python/the filesystem for current facts, writes changed facts
/// into the `Project` input, then lets tracked semantic queries handle editor
/// file contents and downstream derivations.
pub fn refresh_external_data(db: &mut dyn ProjectDb) {
    let Some(project) = db.project() else {
        return;
    };

    refresh_project_external_data(db, project);
}

/// Populate template libraries from the filesystem cache, if available.
///
/// This is a fast, synchronous startup path. It gives completions and
/// diagnostics previously discovered library data while fresh project
/// introspection runs in the background.
pub fn load_template_library_cache(db: &mut dyn ProjectDb) -> bool {
    let Some(project) = db.project() else {
        return false;
    };

    load_project_template_library_cache(db, project)
}

fn refresh_project_external_data(db: &mut dyn ProjectDb, project: Project) {
    refresh_template_dirs(db, project);
    refresh_template_libraries(db, project);
    refresh_template_files(db, project);
    refresh_python_index(db, project);
    refresh_external_semantic_data(db, project);
}

#[derive(Serialize)]
struct TemplateDirsRequest;

#[derive(Deserialize)]
struct TemplateDirsResponse {
    dirs: Vec<Utf8PathBuf>,
}

impl IntrospectionRequest for TemplateDirsRequest {
    const NAME: &'static str = "template_dirs";
    type Response = TemplateDirsResponse;
}

fn refresh_template_dirs(db: &mut dyn ProjectDb, project: Project) {
    let Some(dirs) = fetch_template_dirs(db) else {
        return;
    };

    let next = TemplateDirs::Known(dirs);
    if project.template_dirs(db) != &next {
        project.set_template_dirs(db).to(next);
    }
}

fn fetch_template_dirs(db: &dyn ProjectDb) -> Option<Vec<Utf8PathBuf>> {
    tracing::debug!("Requesting template directories from project introspection");

    let response = db.project_introspector().query(db, &TemplateDirsRequest)?;

    let dir_count = response.dirs.len();
    tracing::info!(
        "Retrieved {} template directories from project introspection",
        dir_count
    );

    for (i, dir) in response.dirs.iter().enumerate() {
        tracing::debug!("  Template dir [{}]: {}", i, dir);
    }

    let missing_dirs: Vec<_> = response.dirs.iter().filter(|dir| !dir.exists()).collect();

    if !missing_dirs.is_empty() {
        tracing::warn!(
            "Found {} non-existent template directories: {:?}",
            missing_dirs.len(),
            missing_dirs
        );
    }

    Some(response.dirs)
}

#[derive(Serialize)]
struct TemplateLibrarySnapshotRequest;

impl IntrospectionRequest for TemplateLibrarySnapshotRequest {
    const NAME: &'static str = "template_libraries";
    type Response = TemplateLibrarySnapshot;
}

fn load_project_template_library_cache(db: &mut dyn ProjectDb, project: Project) -> bool {
    let Some(snapshot) = load_template_library_snapshot_cache(db, project) else {
        return false;
    };

    if apply_template_library_snapshot(db, project, snapshot) {
        refresh_templatetag_modules(db, project);
    }

    true
}

fn refresh_template_libraries(db: &mut dyn ProjectDb, project: Project) {
    if let Some(snapshot) = fetch_template_library_snapshot(db) {
        save_template_library_snapshot_cache(db, project, &snapshot);
        apply_template_library_snapshot(db, project, snapshot);
        return;
    }

    let snapshot = assemble_project_static_template_library_snapshot(db, project);
    apply_static_template_library_snapshot(db, project, snapshot);
}

fn apply_template_library_snapshot(
    db: &mut dyn ProjectDb,
    project: Project,
    snapshot: TemplateLibrarySnapshot,
) -> bool {
    let current = project.template_libraries(db).clone();
    let next = current.apply_active_snapshot(Some(snapshot));
    if project.template_libraries(db) == &next {
        return false;
    }

    project.set_template_libraries(db).to(next);
    true
}

fn fetch_template_library_snapshot(db: &dyn ProjectDb) -> Option<TemplateLibrarySnapshot> {
    db.project_introspector()
        .query(db, &TemplateLibrarySnapshotRequest)
}

fn assemble_project_static_template_library_snapshot(
    db: &dyn ProjectDb,
    project: Project,
) -> Fact<TemplateLibrarySnapshot> {
    let root = project.root(db).clone();
    let interpreter = project.interpreter(db).clone();
    let django_settings_module = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();
    let site_packages_paths = find_site_packages(&interpreter, &root)
        .into_iter()
        .collect::<Vec<_>>();

    assemble_static_template_library_snapshot(
        &root,
        django_settings_module.as_deref(),
        &pythonpath,
        &site_packages_paths,
    )
}

fn apply_static_template_library_snapshot(
    db: &mut dyn ProjectDb,
    project: Project,
    snapshot: Fact<TemplateLibrarySnapshot>,
) -> bool {
    let Some(snapshot) = usable_static_template_library_snapshot(snapshot) else {
        return false;
    };

    apply_template_library_snapshot(db, project, snapshot)
}

fn assemble_static_template_library_snapshot(
    root: &Utf8Path,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
    site_packages_paths: &[Utf8PathBuf],
) -> Fact<TemplateLibrarySnapshot> {
    let Some(django_settings_module) = django_settings_module else {
        return Fact::unknown(vec![Reason::new(
            Field::DjangoEnvironmentDiscovery,
            ReasonSource::Workspace(root.to_path_buf()),
            "django_settings_module is not configured; skipped static template library assembly",
        )]);
    };

    let django_settings_module = match PyModuleName::parse(django_settings_module) {
        Ok(module) => module,
        Err(error) => {
            return Fact::unknown(vec![Reason::new(
                Field::DjangoEnvironmentDiscovery,
                ReasonSource::Workspace(root.to_path_buf()),
                format!("django_settings_module is not a valid Python module path: {error}"),
            )]);
        }
    };

    let explicit_python_paths = pythonpath.iter().map(Utf8PathBuf::from).collect::<Vec<_>>();
    let module_search_paths =
        discover_static_module_search_paths(root, &explicit_python_paths, site_packages_paths);
    let mut static_reasons = module_search_paths.reasons().to_vec();
    let Some(search_paths) = module_search_paths.value() else {
        return Fact::unknown(module_search_paths.reasons().to_vec());
    };

    let settings_resolution =
        resolve_static_module(django_settings_module.clone(), search_paths, root);
    extend_unique_static_reasons(
        &mut static_reasons,
        settings_resolution.resolved.reasons().to_vec(),
    );
    let settings_file = match settings_resolution.resolved.value() {
        Some(resolved) => resolved.file.clone(),
        None => return Fact::unknown(settings_resolution.resolved.reasons().to_vec()),
    };

    let settings_facts = extract_settings_facts_for_module(
        &settings_file,
        &django_settings_module,
        root,
        search_paths,
    );
    let app_registry = resolve_app_registry(&settings_facts.installed_apps, root, search_paths);
    let template_libraries = assemble_template_libraries(
        &settings_facts.template_backends,
        &app_registry.app_registry,
    );
    let template_symbols =
        assemble_template_symbols(&template_libraries, &module_search_paths, root);
    let snapshot = assemble_template_library_snapshot(&template_libraries, &template_symbols);

    add_static_reasons(snapshot, static_reasons)
}

fn usable_static_template_library_snapshot(
    snapshot: Fact<TemplateLibrarySnapshot>,
) -> Option<TemplateLibrarySnapshot> {
    match snapshot {
        Fact::Known { value } => has_static_template_libraries(&value).then_some(value),
        Fact::Partial { .. } | Fact::Unknown { .. } | Fact::Ambiguous { .. } => None,
    }
}

fn has_static_template_libraries(snapshot: &TemplateLibrarySnapshot) -> bool {
    !snapshot.libraries.is_empty() || !snapshot.builtins.is_empty() || !snapshot.symbols.is_empty()
}

fn add_static_reasons<T>(fact: Fact<T>, new_reasons: impl IntoIterator<Item = Reason>) -> Fact<T> {
    let new_reasons = new_reasons.into_iter().collect::<Vec<_>>();
    if new_reasons.is_empty() {
        return fact;
    }

    match fact {
        Fact::Known { value } => Fact::Partial {
            value,
            reasons: new_reasons,
        },
        Fact::Partial { value, mut reasons } => {
            extend_unique_static_reasons(&mut reasons, new_reasons);
            Fact::Partial { value, reasons }
        }
        Fact::Unknown { mut reasons } => {
            extend_unique_static_reasons(&mut reasons, new_reasons);
            Fact::Unknown { reasons }
        }
        Fact::Ambiguous {
            candidates,
            mut reasons,
        } => {
            extend_unique_static_reasons(&mut reasons, new_reasons);
            Fact::Ambiguous {
                candidates,
                reasons,
            }
        }
    }
}

fn extend_unique_static_reasons(reasons: &mut Vec<Reason>, new_reasons: Vec<Reason>) {
    for reason in new_reasons {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CacheEnvelope {
    djls_version: String,
    response: TemplateLibrarySnapshot,
}

fn load_template_library_snapshot_cache(
    db: &dyn ProjectDb,
    project: Project,
) -> Option<TemplateLibrarySnapshot> {
    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let django_settings_module = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    load_cached_template_library_snapshot(
        &root,
        &interpreter,
        django_settings_module.as_deref(),
        &pythonpath,
    )
}

fn save_template_library_snapshot_cache(
    db: &dyn ProjectDb,
    project: Project,
    response: &TemplateLibrarySnapshot,
) {
    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let django_settings_module = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    save_template_library_snapshot(
        &root,
        &interpreter,
        django_settings_module.as_deref(),
        &pythonpath,
        response,
    );
}

fn cache_key(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(format!("{interpreter:?}").as_bytes());
    hasher.update(b"\0");
    hasher.update(django_settings_module.unwrap_or("").as_bytes());
    hasher.update(b"\0");
    for path in pythonpath {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
    }
    let digest = hasher.finalize();
    let mut key = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut key, "{byte:02x}").expect("writing to String cannot fail");
    }
    key
}

fn cache_dir(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
) -> Option<Utf8PathBuf> {
    let base = djls_conf::project_dirs()
        .and_then(|dirs| Utf8PathBuf::from_path_buf(dirs.cache_dir().to_path_buf()).ok())?;
    let key = cache_key(root, interpreter, django_settings_module, pythonpath);
    // Keep the legacy `inspector` directory for on-disk cache compatibility.
    Some(base.join("inspector").join(&key[..16]))
}

fn load_cached_template_library_snapshot(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
) -> Option<TemplateLibrarySnapshot> {
    let dir = cache_dir(root, interpreter, django_settings_module, pythonpath)?;
    // Keep the legacy filename for on-disk cache compatibility.
    let path = dir.join("inspector.json");

    let content = fs::read_to_string(path.as_std_path()).ok()?;
    let envelope: CacheEnvelope = serde_json::from_str(&content).ok()?;

    if envelope.djls_version != env!("CARGO_PKG_VERSION") {
        tracing::debug!(
            "Template library snapshot cache version mismatch: cached={}, current={}",
            envelope.djls_version,
            env!("CARGO_PKG_VERSION"),
        );
        return None;
    }

    tracing::info!("Loaded template library snapshot from cache: {}", path);
    Some(envelope.response)
}

fn save_template_library_snapshot(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
    response: &TemplateLibrarySnapshot,
) {
    let Some(dir) = cache_dir(root, interpreter, django_settings_module, pythonpath) else {
        return;
    };

    let Ok(response_value) = serde_json::to_value(response) else {
        tracing::warn!("Failed to serialize template library snapshot for cache");
        return;
    };
    let Ok(response_copy) = serde_json::from_value(response_value) else {
        tracing::warn!("Failed to roundtrip template library snapshot for cache");
        return;
    };
    let envelope = CacheEnvelope {
        djls_version: env!("CARGO_PKG_VERSION").to_string(),
        response: response_copy,
    };

    if let Err(e) = fs::create_dir_all(dir.as_std_path()) {
        tracing::warn!("Failed to create template library snapshot cache directory: {e}");
        return;
    }

    let path = dir.join("inspector.json");
    match serde_json::to_string(&envelope) {
        Ok(json) => {
            if let Err(e) = fs::write(path.as_std_path(), json) {
                tracing::warn!("Failed to write template library snapshot cache: {e}");
            } else {
                tracing::debug!("Saved template library snapshot to cache: {}", path);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to serialize template library snapshot cache: {e}");
        }
    }
}

fn refresh_template_files(db: &mut dyn ProjectDb, project: Project) {
    let next = match project.template_dirs(db).as_known() {
        Some(search_dirs) => discover_project_template_files(db, search_dirs),
        None => ProjectTemplateFiles::default(),
    };

    if project.template_files(db) != &next {
        project.set_template_files(db).to(next);
    }
}

fn discover_project_template_files(
    db: &dyn ProjectDb,
    search_dirs: &[Utf8PathBuf],
) -> ProjectTemplateFiles {
    let mut templates = Vec::new();

    for dir in search_dirs {
        if !dir.exists() {
            tracing::warn!("Template directory does not exist: {}", dir);
            continue;
        }

        let mut dir_templates = Vec::new();
        for entry in WalkBuilder::new(dir.as_std_path())
            .standard_filters(false)
            .build()
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file())
            })
        {
            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path().to_path_buf()) else {
                continue;
            };

            let name = match path.strip_prefix(dir) {
                Ok(rel) => rel.clean().to_string(),
                Err(_) => continue,
            };

            dir_templates.push((name, path));
        }

        dir_templates.sort_by(|(a_name, a_path), (b_name, b_path)| {
            a_name.cmp(b_name).then_with(|| a_path.cmp(b_path))
        });
        templates.extend(dir_templates.into_iter().map(|(name, path)| {
            ProjectTemplateFile::new(name, path.clone(), db.get_or_create_file(&path))
        }));
    }

    ProjectTemplateFiles::from_ordered(templates)
}

fn refresh_python_index(db: &mut dyn ProjectDb, project: Project) {
    let root = project.root(db).clone();
    let modules = discover_model_files(&root, FileRootKind::Project)
        .into_iter()
        .map(|(module_path, file_path)| {
            ProjectPythonModule::model(
                module_path,
                file_path.clone(),
                db.get_or_create_file(&file_path),
            )
        })
        .chain(templatetag_modules(db, project))
        .collect();

    let next = ProjectPythonIndex::new(modules);
    if project.python_index(db) != &next {
        project.set_python_index(db).to(next);
    }
}

fn refresh_templatetag_modules(db: &mut dyn ProjectDb, project: Project) {
    let modules = project
        .python_index(db)
        .models()
        .cloned()
        .chain(templatetag_modules(db, project))
        .collect();

    let next = ProjectPythonIndex::new(modules);
    if project.python_index(db) != &next {
        project.set_python_index(db).to(next);
    }
}

fn templatetag_modules(db: &dyn ProjectDb, project: Project) -> Vec<ProjectPythonModule> {
    let root = project.root(db).clone();
    let modules = project
        .template_libraries(db)
        .registration_modules()
        .into_iter()
        .collect::<Vec<_>>();

    if modules.is_empty() {
        return Vec::new();
    }

    let interpreter = project.interpreter(db).clone();
    let pythonpath = project.pythonpath(db).clone();
    let explicit_python_paths = pythonpath.iter().map(Utf8PathBuf::from).collect::<Vec<_>>();
    let site_packages_paths = find_site_packages(&interpreter, &root)
        .into_iter()
        .collect::<Vec<_>>();
    let module_search_paths =
        discover_static_module_search_paths(&root, &explicit_python_paths, &site_packages_paths);
    let Some(search_paths) = module_search_paths.value() else {
        return Vec::new();
    };

    modules
        .into_iter()
        .filter_map(|module| {
            let resolution = resolve_static_module(module, search_paths, &root);
            let resolved = resolution.resolved.value()?;
            if resolved.location != ModuleLocation::Workspace {
                return None;
            }

            Some(ProjectPythonModule::templatetag(
                ModulePath::new(resolved.module.as_str()),
                resolved.file.clone(),
                db.get_or_create_file(&resolved.file),
            ))
        })
        .collect()
}

fn refresh_external_semantic_data(db: &mut dyn ProjectDb, project: Project) {
    scan_external_rules(db, project);
    scan_external_models(db, project);
}

fn scan_external_models(db: &mut dyn ProjectDb, project: Project) {
    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();

    let new_models = match find_site_packages(&interpreter, &root) {
        Some(site_packages) => {
            let files = discover_model_files(&site_packages, FileRootKind::LibrarySearchPath);
            extract_models_from_files(&files)
        }
        None => FxHashMap::default(),
    };

    if project.extracted_external_models(db) != &new_models {
        project.set_extracted_external_models(db).to(new_models);
    }
}

fn scan_external_rules(db: &mut dyn ProjectDb, project: Project) {
    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    let modules: FxHashSet<String> = project
        .template_libraries(db)
        .registration_modules()
        .into_iter()
        .map(|m| m.as_str().to_string())
        .collect();

    let new_extraction = if modules.is_empty() {
        SplitExtractionResults::empty()
    } else {
        let search_paths = build_search_paths(&interpreter, &root, &pythonpath);
        let (_workspace, external_modules) =
            resolve_modules(modules.iter().map(String::as_str), &search_paths, &root);
        split_extraction_results(extract_rules_from_modules(external_modules))
    };

    if project.extracted_external_tag_rules(db) != &new_extraction.tag_rules {
        project
            .set_extracted_external_tag_rules(db)
            .to(new_extraction.tag_rules);
    }
    if project.extracted_external_filter_arities(db) != &new_extraction.filter_arities {
        project
            .set_extracted_external_filter_arities(db)
            .to(new_extraction.filter_arities);
    }
    if project.extracted_external_block_specs(db) != &new_extraction.block_specs {
        project
            .set_extracted_external_block_specs(db)
            .to(new_extraction.block_specs);
    }
}

fn extract_models_from_files(
    files: &[(ModulePath, Utf8PathBuf)],
) -> FxHashMap<ModulePath, ModelGraph> {
    let mut results = FxHashMap::default();

    for (module_path, file_path) in files {
        let source = match fs::read_to_string(file_path.as_std_path()) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("Failed to read model file {}: {}", file_path, e);
                continue;
            }
        };

        let graph = extract_model_graph(&source, module_path.as_str());
        if !graph.is_empty() {
            results.insert(module_path.clone(), graph);
        }
    }

    results
}

fn extract_rules_from_modules(modules: Vec<ResolvedModule>) -> FxHashMap<String, ExtractionResult> {
    let mut results = FxHashMap::default();

    for resolved in modules {
        match fs::read_to_string(resolved.file_path.as_std_path()) {
            Ok(source) => {
                let module_result = extract_rules(&source, &resolved.module_path);
                if !module_result.is_empty() {
                    results.insert(resolved.module_path, module_result);
                }
            }
            Err(e) => {
                tracing::debug!("Failed to read module file {}: {}", resolved.file_path, e);
            }
        }
    }

    results
}

struct SplitExtractionResults {
    tag_rules: FxHashMap<String, TagRuleMap>,
    filter_arities: FxHashMap<String, FilterArityMap>,
    block_specs: FxHashMap<String, BlockSpecs>,
}

impl SplitExtractionResults {
    fn empty() -> Self {
        Self {
            tag_rules: FxHashMap::default(),
            filter_arities: FxHashMap::default(),
            block_specs: FxHashMap::default(),
        }
    }
}

fn split_extraction_results(
    results: FxHashMap<String, ExtractionResult>,
) -> SplitExtractionResults {
    let mut split = SplitExtractionResults::empty();

    for (module_path, result) in results {
        if !result.tag_rules.is_empty() {
            split
                .tag_rules
                .insert(module_path.clone(), result.tag_rules);
        }
        if !result.filter_arities.is_empty() {
            split
                .filter_arities
                .insert(module_path.clone(), result.filter_arities);
        }
        if !result.block_specs.is_empty() {
            split.block_specs.insert(module_path, result.block_specs);
        }
    }

    split
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use djls_source::SourceFiles;
    use tempfile::TempDir;

    use super::*;
    use crate::project::introspector::ProjectIntrospector;
    use crate::project::symbols::InstalledSymbolOrigin;
    use crate::project::symbols::Knowledge;
    use crate::project::symbols::TemplateLibraries;
    use crate::project::symbols::TemplateSymbolKind;

    #[salsa::db]
    struct StaticSnapshotTestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        project: Option<Project>,
        introspector: Arc<ProjectIntrospector>,
    }

    impl StaticSnapshotTestDb {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                files: SourceFiles::default(),
                project: None,
                introspector: Arc::new(ProjectIntrospector::new()),
            }
        }

        fn with_project(
            root: Utf8PathBuf,
            django_settings_module: Option<String>,
        ) -> (Self, Project) {
            let mut db = Self::new();
            let project = Project::new(
                &db,
                root,
                Interpreter::discover(None),
                django_settings_module,
                Vec::new(),
                Vec::new(),
                TemplateDirs::Unknown,
                djls_conf::TagSpecDef::default(),
                TemplateLibraries::default(),
                ProjectTemplateFiles::default(),
                ProjectPythonIndex::default(),
                FxHashMap::default(),
                FxHashMap::default(),
                FxHashMap::default(),
                FxHashMap::default(),
            );
            db.project = Some(project);
            (db, project)
        }
    }

    #[salsa::db]
    impl salsa::Database for StaticSnapshotTestDb {}

    #[salsa::db]
    impl djls_source::Db for StaticSnapshotTestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            fs::read_to_string(path.as_std_path())
        }
    }

    #[salsa::db]
    impl ProjectDb for StaticSnapshotTestDb {
        fn project(&self) -> Option<Project> {
            self.project
        }

        fn project_introspector(&self) -> Arc<ProjectIntrospector> {
            self.introspector.clone()
        }
    }

    fn test_response() -> TemplateLibrarySnapshot {
        TemplateLibrarySnapshot {
            symbols: vec![],
            libraries: BTreeMap::from([(
                "i18n".to_string(),
                "django.templatetags.i18n".to_string(),
            )]),
            builtins: vec!["django.template.defaulttags".to_string()],
        }
    }

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn write_static_template_fixture(root: &Utf8Path) {
        write_file(&root.join("project/__init__.py"), "");
        write_file(
            &root.join("project/settings.py"),
            r#"
INSTALLED_APPS = ["blog"]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [],
        "APP_DIRS": True,
        "OPTIONS": {},
    }
]
"#,
        );
        write_file(&root.join("blog/__init__.py"), "");
        write_file(&root.join("blog/templatetags/__init__.py"), "");
        write_file(
            &root.join("blog/templatetags/blog_tags.py"),
            r"
from django import template
register = template.Library()

@register.simple_tag
def shout(value):
    return value.upper()

@register.filter
def emph(value):
    return value
",
        );
        write_file(&root.join("django/__init__.py"), "");
        write_file(&root.join("django/template/__init__.py"), "");
        write_file(&root.join("django/templatetags/__init__.py"), "");
        write_file(
            &root.join("django/template/defaulttags.py"),
            r#"
from django import template
register = template.Library()

@register.tag("if")
def do_if(parser, token):
    pass
"#,
        );
        write_file(
            &root.join("django/template/defaultfilters.py"),
            r"
from django import template
register = template.Library()

@register.filter
def lower(value):
    return value
",
        );
        write_file(
            &root.join("django/template/loader_tags.py"),
            r"
from django import template
register = template.Library()

@register.tag
def block(parser, token):
    pass
",
        );
        write_file(
            &root.join("django/templatetags/i18n.py"),
            r#"
from django import template
register = template.Library()

@register.simple_tag(name="trans")
def do_translate(value):
    return value
"#,
        );
        for library in ["cache", "l10n", "static", "tz"] {
            write_file(
                &root.join(format!("django/templatetags/{library}.py")),
                r"
from django import template
register = template.Library()

@register.simple_tag
def default_tag(value):
    return value
",
            );
        }
    }

    #[test]
    fn static_template_snapshot_populates_project_template_libraries() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_fixture(&root);

        let snapshot =
            assemble_static_template_library_snapshot(&root, Some("project.settings"), &[], &[]);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_static_template_library_snapshot(
            &mut db, project, snapshot
        ));

        let libraries = project.template_libraries(&db);
        assert_eq!(libraries.active_knowledge, Knowledge::Known);
        assert!(libraries
            .builtins
            .contains_key(&PyModuleName::parse("django.template.defaulttags").unwrap()));

        let builtin_tags = libraries.installed_symbol_candidates(TemplateSymbolKind::Tag);
        assert!(builtin_tags.iter().any(|candidate| {
            candidate.symbol.name() == "if"
                && matches!(candidate.origin, InstalledSymbolOrigin::Builtin { .. })
        }));

        let blog_tags = libraries
            .best_loadable_library_str("blog_tags")
            .expect("blog_tags should be loadable");
        assert!(blog_tags.is_active());
        assert_eq!(blog_tags.module().as_str(), "blog.templatetags.blog_tags");
        assert!(blog_tags
            .symbols
            .iter()
            .any(|symbol| { symbol.kind == TemplateSymbolKind::Tag && symbol.name() == "shout" }));
        assert!(blog_tags.symbols.iter().any(|symbol| {
            symbol.kind == TemplateSymbolKind::Filter && symbol.name() == "emph"
        }));

        let modules = libraries.registration_modules();
        assert!(modules.contains(&PyModuleName::parse("django.template.defaulttags").unwrap()));
        assert!(modules.contains(&PyModuleName::parse("blog.templatetags.blog_tags").unwrap()));
    }

    #[test]
    fn template_library_snapshot_routing_prefers_inspector_snapshot() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_fixture(&root);
        let inspector_snapshot = TemplateLibrarySnapshot {
            symbols: Vec::new(),
            libraries: BTreeMap::from([(
                "inspector_only".to_string(),
                "inspector.templatetags.only".to_string(),
            )]),
            builtins: Vec::new(),
        };
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_template_library_snapshot(
            &mut db,
            project,
            inspector_snapshot,
        ));

        let libraries = project.template_libraries(&db);
        assert!(libraries.is_enabled_library_str("inspector_only"));
        assert!(!libraries.is_enabled_library_str("blog_tags"));
    }

    #[test]
    fn template_library_snapshot_routing_uses_static_when_inspector_is_missing() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_fixture(&root);
        let static_snapshot =
            assemble_static_template_library_snapshot(&root, Some("project.settings"), &[], &[]);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_static_template_library_snapshot(
            &mut db,
            project,
            static_snapshot,
        ));

        let libraries = project.template_libraries(&db);
        assert!(libraries.is_enabled_library_str("blog_tags"));
    }

    #[test]
    fn static_templatetag_modules_use_static_resolver_src_root() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_fixture(&root.join("src"));
        let static_snapshot =
            assemble_static_template_library_snapshot(&root, Some("project.settings"), &[], &[]);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_static_template_library_snapshot(
            &mut db,
            project,
            static_snapshot,
        ));
        refresh_python_index(&mut db, project);

        let module_paths = project
            .python_index(&db)
            .templatetags()
            .map(|module| module.module_path().as_str().to_string())
            .collect::<Vec<_>>();
        assert!(module_paths.contains(&"blog.templatetags.blog_tags".to_string()));
    }

    #[test]
    fn static_template_snapshot_preserves_partial_known_data() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_fixture(&root);
        fs::remove_file(root.join("django/templatetags/i18n.py")).unwrap();

        let snapshot =
            assemble_static_template_library_snapshot(&root, Some("project.settings"), &[], &[]);
        let Fact::Partial { value, reasons } = &snapshot else {
            panic!("expected partial snapshot when a default library is unresolved");
        };
        assert!(!reasons.is_empty());
        assert!(value.libraries.contains_key("blog_tags"));
        assert!(value.symbols.iter().any(|symbol| symbol.name == "shout"));
        assert!(usable_static_template_library_snapshot(snapshot).is_none());
    }

    #[test]
    fn static_template_snapshot_declines_empty_templates() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(
            &root.join("project/settings.py"),
            r"
INSTALLED_APPS = []
TEMPLATES = []
",
        );

        let snapshot =
            assemble_static_template_library_snapshot(&root, Some("project.settings"), &[], &[]);

        assert!(usable_static_template_library_snapshot(snapshot).is_none());
    }

    #[test]
    fn static_template_snapshot_accepts_libraries_without_symbols() {
        let snapshot = Fact::known(TemplateLibrarySnapshot {
            symbols: Vec::new(),
            libraries: BTreeMap::from([(
                "custom".to_string(),
                "project.templatetags.custom".to_string(),
            )]),
            builtins: Vec::new(),
        });

        assert!(usable_static_template_library_snapshot(snapshot).is_some());
    }

    #[test]
    fn static_template_snapshot_declines_missing_django_settings_module() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let snapshot = assemble_static_template_library_snapshot(&root, None, &[], &[]);

        assert!(matches!(snapshot, Fact::Unknown { .. }));
        assert!(usable_static_template_library_snapshot(snapshot).is_none());
    }

    #[test]
    fn cache_key_deterministic() {
        let root = Utf8Path::new("/project");
        let interpreter = Interpreter::VenvPath("/project/.venv".to_string());
        let dsm = Some("myproject.settings");
        let pythonpath = vec!["/extra".to_string()];

        let key1 = cache_key(root, &interpreter, dsm, &pythonpath);
        let key2 = cache_key(root, &interpreter, dsm, &pythonpath);
        assert_eq!(key1, key2);
    }

    #[test]
    fn cache_key_varies_with_inputs() {
        let interpreter = Interpreter::VenvPath("/project/.venv".to_string());
        let pythonpath: Vec<String> = vec![];

        let key1 = cache_key(Utf8Path::new("/project-a"), &interpreter, None, &pythonpath);
        let key2 = cache_key(Utf8Path::new("/project-b"), &interpreter, None, &pythonpath);
        assert_ne!(key1, key2);
    }

    #[test]
    fn roundtrip_through_filesystem() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let interpreter = Interpreter::VenvPath("/test/.venv".to_string());

        let response = test_response();

        save_template_library_snapshot(&root, &interpreter, None, &[], &response);
        let Some(cache_file) =
            cache_dir(&root, &interpreter, None, &[]).map(|dir| dir.join("inspector.json"))
        else {
            return;
        };
        if !cache_file.is_file() {
            return;
        }

        let loaded = load_cached_template_library_snapshot(&root, &interpreter, None, &[]);

        // Cache reads from the XDG dir, not from the project root — so this
        // only works if the cache file can be written. If it can't (CI or
        // sandboxed tests), the save is a no-op and load returns None.
        let loaded = loaded.expect("should load cached response");
        assert_eq!(loaded.libraries.len(), 1);
        assert_eq!(loaded.builtins.len(), 1);
    }

    #[test]
    fn extract_models_finds_models() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("myapp");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("models.py"),
            r"
from django.db import models

class Article(models.Model):
    title = models.CharField(max_length=200)
    author = models.ForeignKey('auth.User', on_delete=models.CASCADE)
",
        )
        .unwrap();

        let files = discover_model_files(&root, FileRootKind::LibrarySearchPath);
        let results = extract_models_from_files(&files);
        assert_eq!(results.len(), 1);
        assert!(results.contains_key("myapp.models"));
        assert!(results["myapp.models"].get("Article").is_some());
    }

    #[test]
    fn extract_models_skips_empty() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("emptyapp");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(app_dir.join("models.py"), "# no models here\n").unwrap();

        let files = discover_model_files(&root, FileRootKind::LibrarySearchPath);
        let results = extract_models_from_files(&files);
        assert!(results.is_empty());
    }

    #[test]
    fn extract_models_nested_apps() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        for app in &["blog", "accounts"] {
            let app_dir = root.join(app);
            fs::create_dir_all(&app_dir).unwrap();
            fs::write(
                app_dir.join("models.py"),
                format!(
                    "from django.db import models\nclass {name}Model(models.Model):\n    pass\n",
                    name = app.chars().next().unwrap().to_uppercase().to_string() + &app[1..]
                ),
            )
            .unwrap();
        }

        let files = discover_model_files(&root, FileRootKind::LibrarySearchPath);
        let results = extract_models_from_files(&files);
        assert_eq!(results.len(), 2);
        assert!(results.contains_key("blog.models"));
        assert!(results.contains_key("accounts.models"));
    }

    #[test]
    fn extract_models_package() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let models_dir = root.join("myapp/models");
        fs::create_dir_all(&models_dir).unwrap();
        fs::write(
            models_dir.join("__init__.py"),
            "from .user import User\nfrom .order import Order\n",
        )
        .unwrap();
        fs::write(
            models_dir.join("user.py"),
            "from django.db import models\nclass User(models.Model):\n    pass\n",
        )
        .unwrap();
        fs::write(
            models_dir.join("order.py"),
            "from django.db import models\nclass Order(models.Model):\n    user = models.ForeignKey(User, on_delete=models.CASCADE)\n",
        )
        .unwrap();

        let files = discover_model_files(&root, FileRootKind::LibrarySearchPath);
        let results = extract_models_from_files(&files);
        // __init__.py has no model defs, so only the two submodules
        assert_eq!(results.len(), 2);
        assert!(results.contains_key("myapp.models.user"));
        assert!(results.contains_key("myapp.models.order"));
        assert!(results["myapp.models.user"].get("User").is_some());
        assert!(results["myapp.models.order"].get("Order").is_some());
    }

    #[test]
    fn extract_models_nested_package() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let base_dir = root.join("myapp/models/base");
        fs::create_dir_all(&base_dir).unwrap();
        fs::write(root.join("myapp/models/__init__.py"), "").unwrap();
        fs::write(base_dir.join("__init__.py"), "").unwrap();
        fs::write(
            base_dir.join("abstract.py"),
            "from django.db import models\nclass BaseModel(models.Model):\n    class Meta:\n        abstract = True\n",
        )
        .unwrap();

        let files = discover_model_files(&root, FileRootKind::LibrarySearchPath);
        let results = extract_models_from_files(&files);
        assert!(
            results.contains_key("myapp.models.base.abstract"),
            "should extract from nested model files: got {:?}",
            results.keys().collect::<Vec<_>>()
        );
        assert!(results["myapp.models.base.abstract"]
            .get("BaseModel")
            .is_some());
    }
}
