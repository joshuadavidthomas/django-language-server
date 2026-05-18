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

use crate::project::app_registry::resolve_app_registry;
use crate::project::app_registry::AppRegistryFacts;
use crate::project::db::Db as ProjectDb;
use crate::project::facts::Fact;
use crate::project::facts::Field;
use crate::project::facts::ModuleLocation;
use crate::project::facts::ModuleSearchPathEntry;
use crate::project::facts::Reason;
use crate::project::facts::ReasonSource;
use crate::project::facts::SettingsFacts;
use crate::project::input::Project;
use crate::project::input::ProjectPythonIndex;
use crate::project::input::ProjectPythonModule;
use crate::project::input::ProjectTemplateFile;
use crate::project::input::ProjectTemplateFiles;
use crate::project::input::TemplateDirs;
use crate::project::introspector::IntrospectionRequest;
use crate::project::module_resolver::discover_module_search_paths as discover_static_module_search_paths;
use crate::project::module_resolver::resolve_module as resolve_static_module;
use crate::project::names::PyModuleName;
use crate::project::python::Interpreter;
use crate::project::resolve::build_search_paths;
use crate::project::resolve::discover_model_files;
use crate::project::resolve::find_site_packages;
use crate::project::resolve::resolve_modules;
use crate::project::resolve::ResolvedModule;
use crate::project::settings_facts::extract_settings_facts_for_module;
use crate::project::symbols::Knowledge;
use crate::project::symbols::TemplateLibrarySnapshot;
use crate::project::template_dirs::assemble_template_dirs;
use crate::project::template_libraries::assemble_template_libraries;
use crate::project::template_symbols::assemble_template_library_snapshot;
use crate::project::template_symbols::assemble_template_symbols;
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
    let inspector_dirs = fetch_template_dirs(db);
    let static_dirs = inspector_dirs
        .is_none()
        .then(|| assemble_project_static_template_dirs(db, project));
    apply_template_dirs_or_static_fallback(db, project, inspector_dirs, static_dirs);
}

fn apply_template_dirs(db: &mut dyn ProjectDb, project: Project, dirs: Vec<Utf8PathBuf>) -> bool {
    let next = TemplateDirs::Known(dirs);
    if project.template_dirs(db) != &next {
        project.set_template_dirs(db).to(next);
        return true;
    }

    false
}

fn apply_template_dirs_or_static_fallback(
    db: &mut dyn ProjectDb,
    project: Project,
    inspector_dirs: Option<Vec<Utf8PathBuf>>,
    static_dirs: Option<Fact<Vec<Utf8PathBuf>>>,
) -> bool {
    if let Some(dirs) = inspector_dirs {
        return apply_template_dirs(db, project, dirs);
    }

    let Some(dirs) = static_dirs else {
        return false;
    };
    apply_static_template_dirs(db, project, dirs)
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

fn assemble_project_static_template_dirs(
    db: &dyn ProjectDb,
    project: Project,
) -> Fact<Vec<Utf8PathBuf>> {
    let root = project.root(db).clone();
    let interpreter = project.interpreter(db).clone();
    let django_settings_module = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();
    let site_packages_paths = find_site_packages(&interpreter, &root)
        .into_iter()
        .collect::<Vec<_>>();

    assemble_static_template_dirs(
        &root,
        django_settings_module.as_deref(),
        &pythonpath,
        &site_packages_paths,
    )
}

fn apply_static_template_dirs(
    db: &mut dyn ProjectDb,
    project: Project,
    dirs: Fact<Vec<Utf8PathBuf>>,
) -> bool {
    let Some(dirs) = usable_static_template_dirs(dirs) else {
        if project.template_dirs(db) != &TemplateDirs::Unknown {
            project.set_template_dirs(db).to(TemplateDirs::Unknown);
        }
        return false;
    };

    apply_template_dirs(db, project, dirs)
}

fn assemble_static_template_dirs(
    root: &Utf8Path,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
    site_packages_paths: &[Utf8PathBuf],
) -> Fact<Vec<Utf8PathBuf>> {
    let context = match assemble_static_project_context(
        root,
        django_settings_module,
        pythonpath,
        site_packages_paths,
    ) {
        Ok(context) => context,
        Err(reasons) => return Fact::unknown(reasons),
    };

    let dirs = assemble_template_dirs(
        &context.settings_facts.template_backends,
        &context.app_registry.app_registry,
    )
    .map(|dirs| dirs.into_iter().map(|dir| dir.path).collect::<Vec<_>>());

    add_static_reasons(dirs, context.reasons)
}

fn usable_static_template_dirs(dirs: Fact<Vec<Utf8PathBuf>>) -> Option<Vec<Utf8PathBuf>> {
    match dirs {
        Fact::Known { value } => Some(value),
        Fact::Partial { .. } | Fact::Unknown { .. } | Fact::Ambiguous { .. } => None,
    }
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
    apply_template_library_snapshot_with_knowledge(db, project, snapshot, Knowledge::Known)
}

fn apply_template_library_snapshot_with_knowledge(
    db: &mut dyn ProjectDb,
    project: Project,
    snapshot: TemplateLibrarySnapshot,
    knowledge: Knowledge,
) -> bool {
    let current = project.template_libraries(db).clone();
    let next = match knowledge {
        Knowledge::Known => current.apply_active_snapshot(Some(snapshot)),
        Knowledge::Partial => current.apply_partial_active_snapshot(Some(snapshot)),
        Knowledge::Unknown => current.apply_active_snapshot(None),
    };
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
    let Some((snapshot, knowledge)) = usable_static_template_library_snapshot(snapshot) else {
        return false;
    };

    apply_template_library_snapshot_with_knowledge(db, project, snapshot, knowledge)
}

fn assemble_static_template_library_snapshot(
    root: &Utf8Path,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
    site_packages_paths: &[Utf8PathBuf],
) -> Fact<TemplateLibrarySnapshot> {
    let context = match assemble_static_project_context(
        root,
        django_settings_module,
        pythonpath,
        site_packages_paths,
    ) {
        Ok(context) => context,
        Err(reasons) => return Fact::unknown(reasons),
    };

    let template_libraries = assemble_template_libraries(
        &context.settings_facts.template_backends,
        &context.app_registry.app_registry,
    );
    let template_symbols =
        assemble_template_symbols(&template_libraries, &context.module_search_paths, root);
    let snapshot = assemble_template_library_snapshot(&template_libraries, &template_symbols);

    add_static_reasons(snapshot, context.reasons)
}

struct StaticProjectContext {
    module_search_paths: Fact<Vec<ModuleSearchPathEntry>>,
    settings_facts: SettingsFacts,
    app_registry: AppRegistryFacts,
    reasons: Vec<Reason>,
}

fn assemble_static_project_context(
    root: &Utf8Path,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
    site_packages_paths: &[Utf8PathBuf],
) -> Result<StaticProjectContext, Vec<Reason>> {
    let Some(django_settings_module) = django_settings_module else {
        return Err(vec![Reason::new(
            Field::DjangoEnvironmentDiscovery,
            ReasonSource::Workspace(root.to_path_buf()),
            "django_settings_module is not configured; skipped static project model assembly",
        )]);
    };

    let django_settings_module = PyModuleName::parse(django_settings_module).map_err(|error| {
        vec![Reason::new(
            Field::DjangoEnvironmentDiscovery,
            ReasonSource::Workspace(root.to_path_buf()),
            format!("django_settings_module is not a valid Python module path: {error}"),
        )]
    })?;

    let explicit_python_paths = pythonpath.iter().map(Utf8PathBuf::from).collect::<Vec<_>>();
    let module_search_paths =
        discover_static_module_search_paths(root, &explicit_python_paths, site_packages_paths);
    let mut static_reasons = module_search_paths.reasons().to_vec();
    let Some(search_paths) = module_search_paths.value() else {
        return Err(module_search_paths.reasons().to_vec());
    };

    let settings_resolution =
        resolve_static_module(django_settings_module.clone(), search_paths, root);
    extend_unique_static_reasons(
        &mut static_reasons,
        settings_resolution.resolved.reasons().to_vec(),
    );
    let settings_file = match settings_resolution.resolved.value() {
        Some(resolved) => resolved.file.clone(),
        None => return Err(settings_resolution.resolved.reasons().to_vec()),
    };

    let settings_facts = extract_settings_facts_for_module(
        &settings_file,
        &django_settings_module,
        root,
        search_paths,
    );
    let app_registry = resolve_app_registry(&settings_facts.installed_apps, root, search_paths);

    Ok(StaticProjectContext {
        module_search_paths,
        settings_facts,
        app_registry,
        reasons: static_reasons,
    })
}

fn usable_static_template_library_snapshot(
    snapshot: Fact<TemplateLibrarySnapshot>,
) -> Option<(TemplateLibrarySnapshot, Knowledge)> {
    match snapshot {
        Fact::Known { value } => {
            has_static_template_libraries(&value).then_some((value, Knowledge::Known))
        }
        Fact::Partial { value, .. } => {
            has_static_template_libraries(&value).then_some((value, Knowledge::Partial))
        }
        Fact::Unknown { .. } | Fact::Ambiguous { .. } => None,
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
    use std::process::Command;
    use std::sync::Arc;

    use djls_source::SourceFiles;
    use tempfile::TempDir;

    use super::*;
    use crate::project::introspector::ProjectIntrospector;
    use crate::project::names::LibraryName;
    use crate::project::names::PyModuleName;
    use crate::project::symbols::InstalledSymbolOrigin;
    use crate::project::symbols::Knowledge;
    use crate::project::symbols::TemplateLibraries;
    use crate::project::symbols::TemplateSymbolKind;
    use crate::project::symbols::TemplateSymbolSnapshot;
    use crate::testing::collect_errors;
    use crate::testing::TestDatabase;
    use crate::ValidationError;

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
            Self::with_project_options(
                root,
                Interpreter::discover(None),
                django_settings_module,
                Vec::new(),
            )
        }

        fn with_project_options(
            root: Utf8PathBuf,
            interpreter: Interpreter,
            django_settings_module: Option<String>,
            pythonpath: Vec<String>,
        ) -> (Self, Project) {
            let mut db = Self::new();
            let project = Project::new(
                &db,
                root,
                interpreter,
                django_settings_module,
                pythonpath,
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

    fn write_static_template_dirs_fixture(root: &Utf8Path) {
        write_file(&root.join("project/__init__.py"), "");
        write_file(
            &root.join("project/settings.py"),
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = ["blog"]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates"],
        "APP_DIRS": True,
        "OPTIONS": {},
    }
]
"#,
        );
        write_file(&root.join("blog/__init__.py"), "");
        write_file(&root.join("templates/base.html"), "");
        write_file(&root.join("blog/templates/blog/detail.html"), "");
    }

    fn write_multisite_runtime_template_fixture(root: &Utf8Path) {
        for app in ["app1", "app2", "app3"] {
            write_file(&root.join(format!("apps/clientname/{app}/__init__.py")), "");
            write_file(&root.join(format!("apps/clientname/{app}/apps.py")), "");
            write_file(
                &root.join(format!("apps/clientname/{app}/templatetags/__init__.py")),
                "",
            );
            write_file(
                &root.join(format!("apps/clientname/{app}/templatetags/{app}_tags.py")),
                &format!(
                    r"
from django import template
register = template.Library()

@register.simple_tag
def {app}_marker():
    return '{app}'
"
                ),
            );
        }
        write_file(&root.join("apps/clientname/__init__.py"), "");

        write_multisite_settings(root, "site1", &["clientname.app1", "clientname.app2"]);
        write_multisite_settings(root, "site2", &["clientname.app2", "clientname.app3"]);
    }

    fn write_multisite_settings(root: &Utf8Path, site: &str, apps: &[&str]) {
        write_file(&root.join(format!("projects/{site}/__init__.py")), "");
        write_file(
            &root.join(format!("projects/{site}/settings/__init__.py")),
            "",
        );
        write_file(
            &root.join(format!("projects/{site}/settings/base.py")),
            &format!(
                r#"
from pathlib import Path

PROJECT_DIR = Path(__file__).resolve().parents[1]
SECRET_KEY = "test"
INSTALLED_APPS = {apps:?}
DEFAULT_AUTO_FIELD = "django.db.models.AutoField"
TEMPLATES = [
    {{
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [PROJECT_DIR / "templates"],
        "APP_DIRS": True,
        "OPTIONS": {{}},
    }}
]
"#
            ),
        );
        write_file(
            &root.join(format!("projects/{site}/settings/dev.py")),
            "from .base import *\n",
        );
        write_file(
            &root.join(format!("projects/{site}/templates/{site}/base.html")),
            "",
        );
    }

    fn write_runtime_template_fixture(root: &Utf8Path) {
        write_file(&root.join("project/__init__.py"), "");
        write_file(
            &root.join("project/settings.py"),
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
SECRET_KEY = "test"
INSTALLED_APPS = ["blog"]
DEFAULT_AUTO_FIELD = "django.db.models.AutoField"
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates"],
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
        write_file(&root.join("templates/base.html"), "");
        write_file(&root.join("blog/templates/blog/detail.html"), "");
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SortedComparison<T> {
        missing: Vec<T>,
        extra: Vec<T>,
    }

    impl<T> SortedComparison<T> {
        fn is_empty(&self) -> bool {
            self.missing.is_empty() && self.extra.is_empty()
        }
    }

    fn compare_sorted<T>(
        expected: impl IntoIterator<Item = T>,
        actual: impl IntoIterator<Item = T>,
    ) -> SortedComparison<T>
    where
        T: Clone + Ord,
    {
        let mut expected = expected.into_iter().collect::<Vec<_>>();
        let mut actual = actual.into_iter().collect::<Vec<_>>();
        expected.sort();
        actual.sort();

        let mut missing = Vec::new();
        let mut extra = Vec::new();
        let mut expected_index = 0;
        let mut actual_index = 0;

        while expected_index < expected.len() || actual_index < actual.len() {
            match (expected.get(expected_index), actual.get(actual_index)) {
                (Some(expected_item), Some(actual_item)) if expected_item == actual_item => {
                    expected_index += 1;
                    actual_index += 1;
                }
                (Some(expected_item), Some(actual_item)) if expected_item < actual_item => {
                    missing.push(expected_item.clone());
                    expected_index += 1;
                }
                (Some(_) | None, Some(actual_item)) => {
                    extra.push(actual_item.clone());
                    actual_index += 1;
                }
                (Some(expected_item), None) => {
                    missing.push(expected_item.clone());
                    expected_index += 1;
                }
                (None, None) => break,
            }
        }

        SortedComparison { missing, extra }
    }

    fn assert_sorted_match<T>(label: &str, comparison: &SortedComparison<T>)
    where
        T: std::fmt::Debug,
    {
        assert!(
            comparison.is_empty(),
            "{label} mismatch: missing={:?}, extra={:?}",
            comparison.missing,
            comparison.extra,
        );
    }

    fn fact_value_or_panic<T>(fact: Fact<T>, label: &str) -> T {
        match fact {
            Fact::Known { value } => value,
            Fact::Partial { reasons, .. } => {
                panic!("expected known {label}, got partial: {reasons:?}")
            }
            Fact::Unknown { reasons } => panic!("expected known {label}, got unknown: {reasons:?}"),
            Fact::Ambiguous {
                candidates,
                reasons,
            } => panic!(
                "expected known {label}, got ambiguous: {candidate_count} candidates: {reasons:?}",
                candidate_count = candidates.len(),
            ),
        }
    }

    fn snapshot_loadable_symbol_keys(snapshot: &TemplateLibrarySnapshot) -> Vec<String> {
        snapshot
            .symbols
            .iter()
            .filter_map(|symbol| {
                let load_name = symbol.load_name.as_deref()?;
                Some(format!(
                    "{load_name}:{}:{:?}:{}:{}",
                    symbol.library_module, symbol.kind, symbol.name, symbol.module
                ))
            })
            .collect()
    }

    fn snapshot_builtin_symbol_keys(snapshot: &TemplateLibrarySnapshot) -> Vec<String> {
        snapshot
            .symbols
            .iter()
            .filter(|symbol| symbol.load_name.is_none())
            .map(|symbol| {
                format!(
                    "{}:{:?}:{}:{}",
                    symbol.library_module, symbol.kind, symbol.name, symbol.module
                )
            })
            .collect()
    }

    struct PythonRuntimePaths {
        executable: Utf8PathBuf,
        django_site_packages: Utf8PathBuf,
    }

    fn current_python_runtime_paths() -> PythonRuntimePaths {
        let output = Command::new("python")
            .args([
                "-c",
                "from pathlib import Path; import django, sys; print(getattr(sys, '_base_executable', sys.executable)); print(Path(django.__file__).resolve().parent.parent)",
            ])
            .output()
            .expect("failed to run python for runtime discovery");

        assert!(
            output.status.success(),
            "python runtime discovery failed: {}",
            String::from_utf8_lossy(&output.stderr),
        );

        let output =
            String::from_utf8(output.stdout).expect("python runtime paths should be UTF-8");
        let mut lines = output.lines();
        let executable = lines.next().unwrap_or_default().trim();
        let django_site_packages = lines.next().unwrap_or_default().trim();
        assert!(
            !executable.is_empty() && !django_site_packages.is_empty(),
            "python runtime discovery returned incomplete paths: {output:?}"
        );

        PythonRuntimePaths {
            executable: Utf8PathBuf::from(executable),
            django_site_packages: Utf8PathBuf::from(django_site_packages),
        }
    }

    #[cfg(unix)]
    fn create_runtime_venv(root: &Utf8Path) -> Utf8PathBuf {
        let paths = current_python_runtime_paths();
        let venv = root.join(".venv");
        let bin = venv.join("bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&paths.executable, bin.join("python")).unwrap();
        write_file(
            &venv.join("pyvenv.cfg"),
            &format!(
                "home = {}\ninclude-system-site-packages = false\n",
                paths
                    .executable
                    .parent()
                    .expect("Python executable should have a parent directory")
            ),
        );

        let python_lib_dir = paths
            .django_site_packages
            .parent()
            .expect("Django site-packages should have a Python lib parent");
        let python_dir_name = python_lib_dir
            .file_name()
            .expect("Django site-packages parent should have a directory name");
        let venv_python_lib_dir = venv.join("lib").join(python_dir_name);
        fs::create_dir_all(&venv_python_lib_dir).unwrap();
        std::os::unix::fs::symlink(
            &paths.django_site_packages,
            venv_python_lib_dir.join("site-packages"),
        )
        .unwrap();

        venv
    }

    #[cfg(not(unix))]
    fn create_runtime_venv(_root: &Utf8Path) -> Utf8PathBuf {
        panic!("runtime comparison fixture currently requires Unix symlinks")
    }

    fn runtime_project(
        root: Utf8PathBuf,
        venv: &Utf8Path,
        django_settings_module: &str,
        pythonpath: &[&str],
    ) -> (StaticSnapshotTestDb, Project) {
        StaticSnapshotTestDb::with_project_options(
            root,
            Interpreter::VenvPath(venv.to_string()),
            Some(django_settings_module.to_string()),
            pythonpath
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        )
    }

    fn assert_static_runtime_match(db: &StaticSnapshotTestDb, project: Project) {
        let inspector_dirs = fetch_template_dirs(db).expect("inspector template dirs");
        let static_dirs = fact_value_or_panic(
            assemble_project_static_template_dirs(db, project),
            "template dirs",
        );
        assert_eq!(static_dirs, inspector_dirs);

        let inspector_snapshot =
            fetch_template_library_snapshot(db).expect("inspector template library snapshot");
        let static_snapshot = fact_value_or_panic(
            assemble_project_static_template_library_snapshot(db, project),
            "template library snapshot",
        );

        assert_sorted_match(
            "loadable libraries",
            &compare_sorted(
                inspector_snapshot
                    .libraries
                    .iter()
                    .map(|(name, module)| format!("{name}:{module}")),
                static_snapshot
                    .libraries
                    .iter()
                    .map(|(name, module)| format!("{name}:{module}")),
            ),
        );
        assert_sorted_match(
            "builtins",
            &compare_sorted(
                inspector_snapshot.builtins.iter().cloned(),
                static_snapshot.builtins.iter().cloned(),
            ),
        );
        assert_sorted_match(
            "loadable symbols",
            &compare_sorted(
                snapshot_loadable_symbol_keys(&inspector_snapshot),
                snapshot_loadable_symbol_keys(&static_snapshot),
            ),
        );
        assert_sorted_match(
            "builtin symbols",
            &compare_sorted(
                snapshot_builtin_symbol_keys(&inspector_snapshot),
                snapshot_builtin_symbol_keys(&static_snapshot),
            ),
        );
    }

    #[test]
    fn static_runtime_comparison_reports_missing_and_extra_items() {
        let comparison = compare_sorted(["a", "b"], ["b", "c"]);

        assert_eq!(
            comparison,
            SortedComparison {
                missing: vec!["a"],
                extra: vec!["c"],
            }
        );
    }

    #[test]
    fn static_runtime_comparison_reports_duplicate_items() {
        let comparison = compare_sorted(["a"], ["a", "a"]);

        assert_eq!(
            comparison,
            SortedComparison {
                missing: Vec::new(),
                extra: vec!["a"],
            }
        );
    }

    #[test]
    fn static_runtime_comparison_symbol_keys_include_modules_and_unknown_kind() {
        let snapshot = TemplateLibrarySnapshot {
            libraries: BTreeMap::new(),
            builtins: vec!["django.template.defaulttags".to_string()],
            symbols: vec![
                TemplateSymbolSnapshot {
                    kind: Some(TemplateSymbolKind::Tag),
                    name: "shout".to_string(),
                    load_name: Some("blog_tags".to_string()),
                    library_module: "blog.templatetags.blog_tags".to_string(),
                    module: "blog.templatetags.blog_tags".to_string(),
                    doc: None,
                },
                TemplateSymbolSnapshot {
                    kind: None,
                    name: "lower".to_string(),
                    load_name: None,
                    library_module: "django.template.defaultfilters".to_string(),
                    module: "django.template.defaultfilters".to_string(),
                    doc: None,
                },
            ],
        };

        assert!(snapshot_loadable_symbol_keys(&snapshot).iter().any(|key| {
            key
            == "blog_tags:blog.templatetags.blog_tags:Some(Tag):shout:blog.templatetags.blog_tags"
        }));
        assert!(snapshot_builtin_symbol_keys(&snapshot)
            .iter()
            .any(|key| key
                == "django.template.defaultfilters:None:lower:django.template.defaultfilters"));
    }

    #[test]
    fn template_dirs_routing_prefers_inspector_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_dirs_fixture(&root);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_template_dirs(
            &mut db,
            project,
            vec![root.join("inspector_templates")],
        ));

        assert_eq!(
            project.template_dirs(&db),
            &TemplateDirs::Known(vec![root.join("inspector_templates")])
        );
    }

    #[test]
    fn template_dirs_routing_does_not_assemble_static_when_inspector_dirs_exist() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_dirs_fixture(&root);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_template_dirs_or_static_fallback(
            &mut db,
            project,
            Some(vec![root.join("inspector_templates")]),
            None,
        ));

        assert_eq!(
            project.template_dirs(&db),
            &TemplateDirs::Known(vec![root.join("inspector_templates")])
        );
    }

    #[test]
    fn static_template_dirs_populate_project_template_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_dirs_fixture(&root);
        let dirs = assemble_static_template_dirs(&root, Some("project.settings"), &[], &[]);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_static_template_dirs(&mut db, project, dirs));

        assert_eq!(
            project.template_dirs(&db),
            &TemplateDirs::Known(vec![root.join("templates"), root.join("blog/templates")])
        );
    }

    #[test]
    fn static_template_dirs_drive_template_file_discovery() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_dirs_fixture(&root);
        let dirs = assemble_static_template_dirs(&root, Some("project.settings"), &[], &[]);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_static_template_dirs(&mut db, project, dirs));
        refresh_template_files(&mut db, project);

        assert_eq!(project.template_files(&db).len(), 2);
    }

    #[test]
    fn static_template_dirs_use_static_resolver_src_root() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_dirs_fixture(&root.join("src"));
        let dirs = assemble_static_template_dirs(&root, Some("project.settings"), &[], &[]);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_static_template_dirs(&mut db, project, dirs));

        assert_eq!(
            project.template_dirs(&db),
            &TemplateDirs::Known(vec![
                root.join("src/templates"),
                root.join("src/blog/templates"),
            ])
        );
    }

    #[test]
    fn static_template_dirs_decline_partial_known_data() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(
            &root.join("project/settings.py"),
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = ["missing"]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates"],
        "APP_DIRS": True,
        "OPTIONS": {},
    }
]
"#,
        );
        write_file(&root.join("templates/base.html"), "");

        let dirs = assemble_static_template_dirs(&root, Some("project.settings"), &[], &[]);
        let Fact::Partial { value, reasons } = &dirs else {
            panic!("expected partial dirs when an installed app is unresolved");
        };
        assert_eq!(value, &[root.join("templates")]);
        assert!(!reasons.is_empty());

        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));
        assert!(apply_template_dirs(
            &mut db,
            project,
            vec![root.join("stale_templates")],
        ));
        assert!(!apply_static_template_dirs(&mut db, project, dirs));
        assert_eq!(project.template_dirs(&db), &TemplateDirs::Unknown);
    }

    #[test]
    fn static_template_dirs_clear_stale_template_files_on_partial_fallback() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_static_template_dirs_fixture(&root);
        let stale_dir = root.join("stale_templates");
        write_file(&stale_dir.join("stale.html"), "");
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));
        assert!(apply_template_dirs(&mut db, project, vec![stale_dir]));
        refresh_template_files(&mut db, project);
        assert_eq!(project.template_files(&db).len(), 1);

        write_file(
            &root.join("project/settings.py"),
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = ["missing"]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates"],
        "APP_DIRS": True,
        "OPTIONS": {},
    }
]
"#,
        );
        let dirs = assemble_static_template_dirs(&root, Some("project.settings"), &[], &[]);

        assert!(!apply_static_template_dirs(&mut db, project, dirs));
        refresh_template_files(&mut db, project);

        assert_eq!(project.template_dirs(&db), &TemplateDirs::Unknown);
        assert!(project.template_files(&db).is_empty());
    }

    #[test]
    fn static_template_dirs_apply_known_empty_dirs() {
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

        let dirs = assemble_static_template_dirs(&root, Some("project.settings"), &[], &[]);
        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));

        assert!(apply_static_template_dirs(&mut db, project, dirs));
        assert_eq!(project.template_dirs(&db), &TemplateDirs::Known(Vec::new()));
    }

    #[test]
    #[ignore = "requires Django on the active python; run with `uv run --with django==5.2 cargo test -p djls-semantic static_runtime_comparison_matches_minimal_django_project -- --ignored --nocapture`"]
    fn static_runtime_comparison_matches_minimal_django_project() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_runtime_template_fixture(&root);
        let venv = create_runtime_venv(&root);
        let (db, project) = runtime_project(root, &venv, "project.settings", &[]);

        assert_static_runtime_match(&db, project);

        let static_snapshot = fact_value_or_panic(
            assemble_project_static_template_library_snapshot(&db, project),
            "template library snapshot",
        );
        assert_eq!(
            static_snapshot.libraries.get("blog_tags"),
            Some(&"blog.templatetags.blog_tags".to_string())
        );
        assert!(static_snapshot
            .builtins
            .contains(&"django.template.defaulttags".to_string()));
        assert!(snapshot_loadable_symbol_keys(&static_snapshot)
            .iter()
            .any(|key| {
                key
            == "blog_tags:blog.templatetags.blog_tags:Some(Tag):shout:blog.templatetags.blog_tags"
            }));
        assert!(snapshot_loadable_symbol_keys(&static_snapshot)
            .iter()
            .any(|key| {
                key
            == "blog_tags:blog.templatetags.blog_tags:Some(Filter):emph:blog.templatetags.blog_tags"
            }));
    }

    #[test]
    #[ignore = "requires Django on the active python; run with `uv run --with django==5.2 cargo test -p djls-semantic static_runtime_comparison_matches_split_settings_environments -- --ignored --nocapture`"]
    fn static_runtime_comparison_matches_split_settings_environments() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_multisite_runtime_template_fixture(&root);
        let venv = create_runtime_venv(&root);

        let (site1_db, site1_project) = runtime_project(
            root.clone(),
            &venv,
            "site1.settings.dev",
            &["projects", "apps"],
        );
        assert_static_runtime_match(&site1_db, site1_project);
        let site1_snapshot = fact_value_or_panic(
            assemble_project_static_template_library_snapshot(&site1_db, site1_project),
            "site1 template library snapshot",
        );

        let (site2_db, site2_project) =
            runtime_project(root, &venv, "site2.settings.dev", &["projects", "apps"]);
        assert_static_runtime_match(&site2_db, site2_project);
        let site2_snapshot = fact_value_or_panic(
            assemble_project_static_template_library_snapshot(&site2_db, site2_project),
            "site2 template library snapshot",
        );

        assert!(site1_snapshot.libraries.contains_key("app1_tags"));
        assert!(site1_snapshot.libraries.contains_key("app2_tags"));
        assert!(!site1_snapshot.libraries.contains_key("app3_tags"));
        assert!(!snapshot_loadable_symbol_keys(&site1_snapshot)
            .iter()
            .any(|symbol| symbol.contains("app3_marker")));

        assert!(!site2_snapshot.libraries.contains_key("app1_tags"));
        assert!(site2_snapshot.libraries.contains_key("app2_tags"));
        assert!(site2_snapshot.libraries.contains_key("app3_tags"));
        assert!(!snapshot_loadable_symbol_keys(&site2_snapshot)
            .iter()
            .any(|symbol| symbol.contains("app1_marker")));
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
        assert!(matches!(
            usable_static_template_library_snapshot(snapshot.clone()),
            Some((_, Knowledge::Partial))
        ));

        let (mut db, project) =
            StaticSnapshotTestDb::with_project(root.clone(), Some("project.settings".to_string()));
        assert!(apply_static_template_library_snapshot(
            &mut db, project, snapshot,
        ));
        let libraries = project.template_libraries(&db);
        assert_eq!(libraries.active_knowledge, Knowledge::Partial);
        assert!(libraries
            .loadable
            .contains_key(&LibraryName::parse("blog_tags").unwrap()));
        assert!(libraries
            .registration_modules()
            .contains(&PyModuleName::parse("blog.templatetags.blog_tags").unwrap()));

        let validation_db = TestDatabase::new().with_template_libraries(libraries.clone());
        let errors = collect_errors(
            &validation_db,
            "test.html",
            concat!(
                "{% shout \"hello\" %}\n",
                "{{ value|emph }}\n",
                "{% definitely_unknown_tag %}\n",
                "{{ value|definitely_unknown_filter }}\n",
                "{% load definitely_unknown_library %}\n",
            ),
        );
        assert!(
            errors.iter().any(|error| matches!(
                error,
                ValidationError::UnloadedTag { tag, library, .. }
                    if tag == "shout" && library == "blog_tags"
            )),
            "Expected static partial active facts to keep known unloaded tag diagnostics, got: {errors:?}"
        );
        assert!(
            errors.iter().any(|error| matches!(
                error,
                ValidationError::UnloadedFilter { filter, library, .. }
                    if filter == "emph" && library == "blog_tags"
            )),
            "Expected static partial active facts to keep known unloaded filter diagnostics, got: {errors:?}"
        );
        assert!(
            errors.iter().all(|error| !matches!(
                error,
                ValidationError::UnknownTag { .. }
                    | ValidationError::UnknownFilter { .. }
                    | ValidationError::UnknownLibrary { .. }
            )),
            "Expected static partial active facts to suppress unsafe unknown diagnostics, got: {errors:?}"
        );
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
