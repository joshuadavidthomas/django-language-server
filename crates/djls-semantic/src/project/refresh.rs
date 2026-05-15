use salsa::Setter;

use crate::project::db::Db;
use crate::project::resolve::build_search_paths;
use crate::project::resolve::discover_workspace_model_files;
use crate::project::resolve::resolve_modules;
use crate::project::WorkspaceSemanticSources;

/// Populate template libraries from the filesystem cache, if available.
///
/// This is a fast, synchronous startup path. It gives completions and
/// diagnostics previously discovered library data while fresh project
/// introspection runs in the background.
pub fn load_template_library_cache(db: &mut dyn Db) -> bool {
    let Some(project) = db.project() else {
        return false;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let dsm = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    let Some(response) = super::cache::load_cached_template_library_snapshot(
        &root,
        &interpreter,
        dsm.as_deref(),
        &pythonpath,
    ) else {
        return false;
    };

    let current = project.template_libraries(db).clone();
    let next = current.apply_active_snapshot(Some(response));
    if project.template_libraries(db) != &next {
        project.set_template_libraries(db).to(next);
    }
    refresh_workspace_semantic_sources(db);

    true
}

/// Refresh all external project data.
///
/// Updates active template library data from project introspection, refreshes
/// workspace file discovery, then scans installed packages for validation
/// rules and model definitions. Workspace file contents still flow through
/// tracked Salsa files.
pub fn refresh_external_data(db: &mut dyn Db) {
    refresh_template_dirs(db);
    refresh_template_libraries(db);
    refresh_workspace_semantic_sources(db);
    refresh_external_semantic_data(db);
}

/// Refresh workspace file discovery used by semantic extraction.
fn refresh_workspace_semantic_sources(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    let root = project.root(db).clone();
    let model_files = discover_workspace_model_files(&root);

    let module_paths: Vec<String> = project
        .template_libraries(db)
        .registration_modules()
        .into_iter()
        .map(|module| module.as_str().to_string())
        .collect();

    let registration_modules = if module_paths.is_empty() {
        Vec::new()
    } else {
        let interpreter = project.interpreter(db).clone();
        let pythonpath = project.pythonpath(db).clone();
        let search_paths = build_search_paths(&interpreter, &root, &pythonpath);
        let (workspace_modules, _external) = resolve_modules(
            module_paths.iter().map(String::as_str),
            &search_paths,
            &root,
        );
        workspace_modules
            .into_iter()
            .map(|module| (module.module_path, module.file_path))
            .collect()
    };

    let next = WorkspaceSemanticSources::new(model_files, registration_modules);
    if project.workspace_semantic_sources(db) != &next {
        project.set_workspace_semantic_sources(db).to(next);
    }
}

/// Refresh template directories from the configured project introspector.
fn refresh_template_dirs(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    let next = super::django::fetch_template_dirs(db);
    if project.template_dirs(db) != &next {
        project.set_template_dirs(db).to(next);
    }
}

/// Refresh active template libraries from the configured project introspector.
fn refresh_template_libraries(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let dsm = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    let response = super::symbols::fetch_template_library_snapshot(db);

    if let Some(ref response) = response {
        super::cache::save_template_library_snapshot(
            &root,
            &interpreter,
            dsm.as_deref(),
            &pythonpath,
            response,
        );
    }

    let current = project.template_libraries(db).clone();
    let next = current.apply_active_snapshot(response);
    if project.template_libraries(db) != &next {
        project.set_template_libraries(db).to(next);
    }
}

/// Refresh external semantic data for the current project.
fn refresh_external_semantic_data(db: &mut dyn Db) {
    super::external::refresh_external_semantic_data(db);
}
