use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileSystem;
use thiserror::Error;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::InvalidModuleName;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonPackage;

pub(crate) struct PythonResolver<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PythonImport<'a> {
    pub(crate) level: u32,
    pub(crate) module: Option<&'a str>,
    pub(crate) importer: &'a Utf8Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum PythonImportError {
    #[error(transparent)]
    InvalidModuleName(#[from] InvalidModuleName),
    #[error("absolute import must name a module")]
    EmptyAbsoluteImport,
    #[error("relative import resolved to an empty module name")]
    EmptyRelativeImport,
    #[error("importer is outside project search paths: {0}")]
    ImporterOutsideSearchPaths(String),
    #[error("importer is not a python source file: {0}")]
    ImporterIsNotPythonSource(String),
    #[error("relative import has too many leading dots")]
    TooManyDots,
}

impl<'db> PythonResolver<'db> {
    pub(crate) fn new(db: &'db dyn ProjectDb, project: Project) -> Self {
        Self { db, project }
    }

    pub(crate) fn module(&self, name: &PythonModuleName) -> Option<PythonModule> {
        self.project.touch_search_path_roots(self.db);

        for search_path in self.project.search_paths(self.db).iter() {
            let Some(path) = module_file_in_search_path(
                self.db.file_system(),
                name.as_str(),
                search_path.path(),
            ) else {
                continue;
            };
            let file = self.db.get_or_create_file(&path);
            return Some(PythonModule::new(name.clone(), path, file));
        }

        None
    }

    pub(crate) fn module_from_str(
        &self,
        name: &str,
    ) -> Result<Option<PythonModule>, InvalidModuleName> {
        let name = PythonModuleName::parse(name)?;
        Ok(self.module(&name))
    }

    pub(crate) fn package(&self, name: &PythonModuleName) -> Option<PythonPackage> {
        self.project.touch_search_path_roots(self.db);

        let relative = name.as_str().replace('.', "/");
        for search_path in self.project.search_paths(self.db).iter() {
            let dir = search_path.path().join(&relative);
            if !self.db.path_is_dir(&dir) {
                continue;
            }

            let init_path = dir.join("__init__.py");
            let init_file = self
                .db
                .path_is_file(&init_path)
                .then(|| self.db.get_or_create_file(&init_path));
            return Some(PythonPackage::new(name.clone(), dir, init_file));
        }

        None
    }

    pub(crate) fn package_from_str(
        &self,
        name: &str,
    ) -> Result<Option<PythonPackage>, InvalidModuleName> {
        let name = PythonModuleName::parse(name)?;
        Ok(self.package(&name))
    }

    pub(crate) fn import_name(
        &self,
        import: PythonImport<'_>,
    ) -> Result<PythonModuleName, PythonImportError> {
        if import.level == 0 {
            let module = import
                .module
                .ok_or(PythonImportError::EmptyAbsoluteImport)?;
            return Ok(PythonModuleName::parse(module)?);
        }

        let root = self
            .project
            .search_paths(self.db)
            .iter()
            .filter(|search_path| import.importer.starts_with(search_path.path()))
            .max_by_key(|search_path| search_path.path().as_str().len())
            .map(|search_path| search_path.path())
            .ok_or_else(|| {
                PythonImportError::ImporterOutsideSearchPaths(import.importer.to_string())
            })?;
        let relative = import.importer.strip_prefix(root).map_err(|_| {
            PythonImportError::ImporterOutsideSearchPaths(import.importer.to_string())
        })?;
        if relative.extension() != Some("py") {
            return Err(PythonImportError::ImporterIsNotPythonSource(
                import.importer.to_string(),
            ));
        }

        let mut module_parts: Vec<String> = relative
            .parent()
            .ok_or(PythonImportError::EmptyRelativeImport)?
            .components()
            .map(|component| component.as_str().to_string())
            .collect();

        for _ in 1..import.level {
            module_parts.pop().ok_or(PythonImportError::TooManyDots)?;
        }

        if let Some(module) = import.module {
            module_parts.extend(
                module
                    .split('.')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        }

        if module_parts.is_empty() {
            return Err(PythonImportError::EmptyRelativeImport);
        }

        Ok(PythonModuleName::parse(&module_parts.join("."))?)
    }

    pub(crate) fn import_module(
        &self,
        import: PythonImport<'_>,
    ) -> Result<Option<PythonModule>, PythonImportError> {
        let name = self.import_name(import)?;
        Ok(self.module(&name))
    }

    pub(crate) fn source_file_module(&self, source_path: &Utf8Path) -> Option<PythonModule> {
        let name = self.module_name_for_source_path(source_path)?;
        let module = self.module(&name)?;
        (module.path() == source_path).then_some(module)
    }

    fn module_name_for_source_path(&self, source_path: &Utf8Path) -> Option<PythonModuleName> {
        let search_path = self
            .project
            .search_paths(self.db)
            .iter()
            .filter(|search_path| source_path.starts_with(search_path.path()))
            .max_by_key(|search_path| search_path.path().as_str().len())?;
        let relative = source_path.strip_prefix(search_path.path()).ok()?;
        PythonModuleName::from_relative_source_path(relative).ok()
    }
}

fn module_file_in_search_path(
    fs: &dyn FileSystem,
    module_name: &str,
    search_path: &Utf8Path,
) -> Option<Utf8PathBuf> {
    let mut candidate = search_path.to_path_buf();
    for part in module_name.split('.') {
        candidate.push(part);
    }

    let py_file = candidate.with_extension("py");
    if fs.is_file(&py_file) {
        return Some(py_file);
    }

    let init_file = candidate.join("__init__.py");
    fs.is_file(&init_file).then_some(init_file)
}
