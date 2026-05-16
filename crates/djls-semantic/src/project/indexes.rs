use camino::Utf8PathBuf;
use djls_source::Utf8PathClean;
use ignore::WalkBuilder;
use salsa::Setter;

use super::db::Db;
use super::input::Project;
use super::input::ProjectPythonIndex;
use super::input::ProjectPythonModule;
use super::input::ProjectTemplateFile;
use super::input::ProjectTemplateFiles;
use super::resolve::build_search_paths;
use super::resolve::discover_workspace_model_files;
use super::resolve::resolve_modules;
use crate::python::ModulePath;

impl Project {
    pub(crate) fn refresh_template_files(self, db: &mut dyn Db) {
        let next = match self.template_dirs(db).as_known() {
            Some(search_dirs) => ProjectTemplateFiles::discover(db, search_dirs),
            None => ProjectTemplateFiles::default(),
        };

        if self.template_files(db) != &next {
            self.set_template_files(db).to(next);
        }
    }

    pub(crate) fn refresh_python_index(self, db: &mut dyn Db) {
        let root = self.root(db).clone();
        let modules = discover_workspace_model_files(&root)
            .into_iter()
            .map(|(module_path, file_path)| {
                ProjectPythonModule::model(
                    module_path,
                    file_path.clone(),
                    db.get_or_create_file(&file_path),
                )
            })
            .chain(self.templatetag_modules(db))
            .collect();

        let next = ProjectPythonIndex::new(modules);
        if self.python_index(db) != &next {
            self.set_python_index(db).to(next);
        }
    }

    pub(crate) fn refresh_templatetag_modules(self, db: &mut dyn Db) {
        let modules = self
            .python_index(db)
            .models()
            .cloned()
            .chain(self.templatetag_modules(db))
            .collect();

        let next = ProjectPythonIndex::new(modules);
        if self.python_index(db) != &next {
            self.set_python_index(db).to(next);
        }
    }

    fn templatetag_modules(self, db: &dyn Db) -> Vec<ProjectPythonModule> {
        let root = self.root(db).clone();
        let module_paths: Vec<String> = self
            .template_libraries(db)
            .registration_modules()
            .into_iter()
            .map(|module| module.as_str().to_string())
            .collect();

        if module_paths.is_empty() {
            return Vec::new();
        }

        let interpreter = self.interpreter(db).clone();
        let pythonpath = self.pythonpath(db).clone();
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
}

impl ProjectTemplateFiles {
    fn discover(db: &dyn Db, search_dirs: &[Utf8PathBuf]) -> Self {
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

        Self::from_ordered(templates)
    }
}
