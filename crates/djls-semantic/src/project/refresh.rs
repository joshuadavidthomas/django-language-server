use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::Utf8PathClean;
use ignore::WalkBuilder;
use salsa::Setter;

use crate::project::db::Db;
use crate::project::resolve::build_search_paths;
use crate::project::resolve::discover_workspace_model_files;
use crate::project::resolve::resolve_modules;
use crate::project::Project;
use crate::project::ProjectPythonIndex;
use crate::project::ProjectPythonModule;
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
    let next_libraries = current.apply_active_snapshot(Some(response));
    if project.template_libraries(db) != &next_libraries {
        project.set_template_libraries(db).to(next_libraries);

        let modules = project
            .python_index(db)
            .models()
            .cloned()
            .chain(collect_templatetag_modules(db, project))
            .collect();
        let next_index = ProjectPythonIndex::new(modules);
        if project.python_index(db) != &next_index {
            project.set_python_index(db).to(next_index);
        }
    }

    true
}

/// Refresh all external project data.
///
/// This is the imperative boundary between the outside world and Salsa inputs:
/// it asks Django/Python/the filesystem for current facts, writes changed facts
/// into the `Project` input, then lets tracked semantic queries handle editor
/// file contents and downstream derivations.
pub fn refresh_external_data(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    if let Some(dirs) = super::django::fetch_template_dirs(db) {
        let next_dirs = TemplateDirs::Known(dirs);
        if project.template_dirs(db) != &next_dirs {
            project.set_template_dirs(db).to(next_dirs);
        }
    }

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let dsm = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    if let Some(response) = super::symbols::fetch_template_library_snapshot(db) {
        super::cache::save_template_library_snapshot(
            &root,
            &interpreter,
            dsm.as_deref(),
            &pythonpath,
            &response,
        );

        let current = project.template_libraries(db).clone();
        let next_libraries = current.apply_active_snapshot(Some(response));
        if project.template_libraries(db) != &next_libraries {
            project.set_template_libraries(db).to(next_libraries);
        }
    }

    let next_templates = match project.template_dirs(db).as_known() {
        Some(search_dirs) => {
            ProjectTemplateFiles::from_ordered(collect_template_files(db, search_dirs))
        }
        None => ProjectTemplateFiles::default(),
    };
    if project.template_files(db) != &next_templates {
        project.set_template_files(db).to(next_templates);
    }

    let modules = collect_model_modules(db, &root)
        .into_iter()
        .chain(collect_templatetag_modules(db, project))
        .collect();
    let next_index = ProjectPythonIndex::new(modules);
    if project.python_index(db) != &next_index {
        project.set_python_index(db).to(next_index);
    }

    super::external::refresh_external_semantic_data(db);
}

fn collect_model_modules(db: &dyn Db, root: &Utf8Path) -> Vec<ProjectPythonModule> {
    discover_workspace_model_files(root)
        .into_iter()
        .map(|(module_path, file_path)| {
            ProjectPythonModule::model(
                module_path,
                file_path.clone(),
                db.get_or_create_file(&file_path),
            )
        })
        .collect()
}

fn collect_templatetag_modules(db: &dyn Db, project: Project) -> Vec<ProjectPythonModule> {
    let root = project.root(db).clone();
    let module_paths: Vec<String> = project
        .template_libraries(db)
        .registration_modules()
        .into_iter()
        .map(|module| module.as_str().to_string())
        .collect();

    if module_paths.is_empty() {
        return Vec::new();
    }

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
            ProjectPythonModule::templatetag(
                ModulePath::new(module.module_path),
                module.file_path.clone(),
                db.get_or_create_file(&module.file_path),
            )
        })
        .collect()
}

fn collect_template_files(db: &dyn Db, search_dirs: &[Utf8PathBuf]) -> Vec<ProjectTemplateFile> {
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
