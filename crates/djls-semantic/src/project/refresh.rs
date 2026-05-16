use camino::Utf8PathBuf;
use djls_source::Utf8PathClean;
use ignore::WalkBuilder;
use salsa::Setter;

use crate::project::db::Db;
use crate::project::resolve::build_search_paths;
use crate::project::resolve::discover_workspace_model_files;
use crate::project::resolve::resolve_modules;
use crate::project::ProjectPythonModule;
use crate::project::ProjectPythonModules;
use crate::project::ProjectTemplateFile;
use crate::project::ProjectTemplateFiles;
use crate::project::TemplateDirs;
use crate::python::ModulePath;

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
        refresh_project_templatetag_modules(db);
    }

    true
}

/// Refresh all external project data.
///
/// Updates active template library data from project introspection, refreshes
/// the first-party project file set, then scans installed packages for
/// validation rules and model definitions. Workspace file contents still flow through
/// tracked Salsa files.
pub fn refresh_external_data(db: &mut dyn Db) {
    refresh_template_dirs(db);
    refresh_template_libraries(db);
    refresh_project_files(db);
    refresh_external_semantic_data(db);
}

/// Refresh first-party files used by project-aware semantic analysis.
fn refresh_project_files(db: &mut dyn Db) {
    refresh_project_template_files(db);
    refresh_project_model_modules(db);
    refresh_project_templatetag_modules(db);
}

fn refresh_project_template_files(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    let next = ProjectTemplateFiles::from_ordered(discover_project_template_files(db));
    if project.template_files(db) != &next {
        project.set_template_files(db).to(next);
    }
}

fn refresh_project_model_modules(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    let root = project.root(db).clone();
    let model_modules = discover_workspace_model_files(&root)
        .into_iter()
        .map(|(module_path, file_path)| {
            ProjectPythonModule::new(
                module_path,
                file_path.clone(),
                db.get_or_create_file(&file_path),
            )
        })
        .collect();

    let next = ProjectPythonModules::new(model_modules);
    if project.model_modules(db) != &next {
        project.set_model_modules(db).to(next);
    }
}

fn refresh_project_templatetag_modules(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    let root = project.root(db).clone();
    let module_paths: Vec<String> = project
        .template_libraries(db)
        .registration_modules()
        .into_iter()
        .map(|module| module.as_str().to_string())
        .collect();

    let templatetag_modules = if module_paths.is_empty() {
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
            .map(|module| {
                ProjectPythonModule::new(
                    ModulePath::new(module.module_path),
                    module.file_path.clone(),
                    db.get_or_create_file(&module.file_path),
                )
            })
            .collect()
    };

    let next = ProjectPythonModules::new(templatetag_modules);
    if project.templatetag_modules(db) != &next {
        project.set_templatetag_modules(db).to(next);
    }
}

fn discover_project_template_files(db: &dyn Db) -> Vec<ProjectTemplateFile> {
    let Some(project) = db.project() else {
        return Vec::new();
    };

    let Some(search_dirs) = project.template_dirs(db).as_known() else {
        return Vec::new();
    };

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

    templates
}

/// Refresh template directories from the configured project introspector.
fn refresh_template_dirs(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    let Some(dirs) = super::django::fetch_template_dirs(db) else {
        return;
    };

    let next = TemplateDirs::Known(dirs);
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

    let Some(response) = super::symbols::fetch_template_library_snapshot(db) else {
        return;
    };

    super::cache::save_template_library_snapshot(
        &root,
        &interpreter,
        dsm.as_deref(),
        &pythonpath,
        &response,
    );

    let current = project.template_libraries(db).clone();
    let next = current.apply_active_snapshot(Some(response));
    if project.template_libraries(db) != &next {
        project.set_template_libraries(db).to(next);
    }
}

/// Refresh external semantic data for the current project.
fn refresh_external_semantic_data(db: &mut dyn Db) {
    super::external::refresh_external_semantic_data(db);
}
