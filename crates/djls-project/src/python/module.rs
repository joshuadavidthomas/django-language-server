use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use thiserror::Error;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::InvalidModuleName;
use crate::python::PythonModuleName;

#[derive(Clone, PartialEq, Eq)]
pub struct PythonModule {
    name: PythonModuleName,
    path: Utf8PathBuf,
    file: File,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PythonPackage {
    name: PythonModuleName,
    dir: Utf8PathBuf,
    init_file: Option<File>,
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

impl PythonModule {
    pub(crate) fn new(name: PythonModuleName, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    pub(crate) fn resolve_import(
        db: &dyn ProjectDb,
        project: Project,
        import: PythonImport<'_>,
    ) -> Result<Option<Self>, PythonImportError> {
        let name = if import.level == 0 {
            let module = import
                .module
                .ok_or(PythonImportError::EmptyAbsoluteImport)?;
            PythonModuleName::parse(module)?
        } else {
            let root = project
                .search_paths(db)
                .iter()
                .filter(|search_path| import.importer.starts_with(search_path.path()))
                .max_by_key(|search_path| search_path.path().as_str().len())
                .map(super::search_paths::SearchPath::path)
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

            PythonModuleName::parse(&module_parts.join("."))?
        };

        Ok(Self::resolve(db, project, name))
    }

    #[must_use]
    pub fn name(&self) -> &PythonModuleName {
        &self.name
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }
}

#[salsa::tracked]
impl PythonModule {
    #[salsa::tracked]
    pub(crate) fn resolve(
        db: &dyn ProjectDb,
        project: Project,
        name: PythonModuleName,
    ) -> Option<Self> {
        project.touch_search_path_roots(db);

        for search_path in project.search_paths(db).iter() {
            let mut candidate = search_path.path().to_path_buf();
            for part in name.as_str().split('.') {
                candidate.push(part);
            }

            let py_file = candidate.with_extension("py");
            let path = if db.path_is_file(&py_file) {
                py_file
            } else {
                let init_file = candidate.join("__init__.py");
                if !db.path_is_file(&init_file) {
                    continue;
                }
                init_file
            };

            let file = db.get_or_create_file(&path);
            return Some(Self::new(name, path, file));
        }

        None
    }

    #[salsa::tracked]
    pub(crate) fn resolve_source_path(
        db: &dyn ProjectDb,
        project: Project,
        source_path: Utf8PathBuf,
    ) -> Option<Self> {
        let search_path = project
            .search_paths(db)
            .iter()
            .filter(|search_path| source_path.starts_with(search_path.path()))
            .max_by_key(|search_path| search_path.path().as_str().len())?;
        let relative = source_path.strip_prefix(search_path.path()).ok()?;
        let name = PythonModuleName::from_relative_source_path(relative).ok()?;
        let module = Self::resolve(db, project, name)?;
        (module.path() == source_path.as_path()).then_some(module)
    }
}

impl fmt::Debug for PythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonModule")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PythonPackage {
    fn new(name: PythonModuleName, dir: Utf8PathBuf, init_file: Option<File>) -> Self {
        Self {
            name,
            dir,
            init_file,
        }
    }

    pub(crate) fn name(&self) -> &PythonModuleName {
        &self.name
    }

    pub(crate) fn dir(&self) -> &Utf8Path {
        &self.dir
    }
}

#[salsa::tracked]
impl PythonPackage {
    #[salsa::tracked]
    pub(crate) fn resolve(
        db: &dyn ProjectDb,
        project: Project,
        name: PythonModuleName,
    ) -> Option<Self> {
        project.touch_search_path_roots(db);

        let relative = name.as_str().replace('.', "/");
        for search_path in project.search_paths(db).iter() {
            let dir = search_path.path().join(&relative);
            if !db.path_is_dir(&dir) {
                continue;
            }

            let init_path = dir.join("__init__.py");
            let init_file = db
                .path_is_file(&init_path)
                .then(|| db.get_or_create_file(&init_path));
            return Some(Self::new(name, dir, init_file));
        }

        None
    }
}
