use camino::Utf8PathBuf;
use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonImport;
use crate::python::PythonModule;
use crate::python::PythonSource;
use crate::python::SearchPath;
use crate::python::resolve_module_detail;

pub(crate) enum ImportSourceResolution {
    Resolved(PythonSource),
    Unresolved,
    SkippedExternal,
    ReadFailed { file: File, path: Utf8PathBuf },
}

pub(crate) trait PythonImportResolver {
    fn resolve_star_import(&mut self, import: PythonImport<'_>) -> ImportSourceResolution;

    fn resolve_named_import(&mut self, import: PythonImport<'_>) -> ImportSourceResolution;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PythonSourceReadMode {
    Tracked,
    Discovery,
}

pub(crate) struct ProjectImportSourceResolver<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    mode: PythonSourceReadMode,
}

impl<'db> ProjectImportSourceResolver<'db> {
    pub(crate) fn tracked(db: &'db dyn ProjectDb, project: Project) -> Self {
        Self::new(db, project, PythonSourceReadMode::Tracked)
    }

    pub(crate) fn discovery(db: &'db dyn ProjectDb, project: Project) -> Self {
        Self::new(db, project, PythonSourceReadMode::Discovery)
    }

    fn new(db: &'db dyn ProjectDb, project: Project, mode: PythonSourceReadMode) -> Self {
        Self { db, project, mode }
    }

    pub(crate) fn read_source(&self, file: File) -> Option<PythonSource> {
        let source = match self.mode {
            PythonSourceReadMode::Tracked => file.source(self.db).as_str().to_string(),
            PythonSourceReadMode::Discovery => self.db.read_file(file.path(self.db)).ok()?,
        };
        Some(PythonSource::new(
            file,
            file.path(self.db).to_path_buf(),
            source,
        ))
    }

    fn resolve_python_import(&self, import: PythonImport<'_>) -> Option<PythonModule> {
        PythonModule::resolve_import(self.db, self.project, import).ok()?
    }

    fn read_resolved_module(&mut self, module: &PythonModule) -> ImportSourceResolution {
        let file = module.file();
        self.read_source(file).map_or_else(
            || ImportSourceResolution::ReadFailed {
                file,
                path: file.path(self.db).to_path_buf(),
            },
            ImportSourceResolution::Resolved,
        )
    }
}

impl PythonImportResolver for ProjectImportSourceResolver<'_> {
    fn resolve_star_import(&mut self, import: PythonImport<'_>) -> ImportSourceResolution {
        let Some(module) = self.resolve_python_import(import) else {
            return ImportSourceResolution::Unresolved;
        };
        self.read_resolved_module(&module)
    }

    fn resolve_named_import(&mut self, import: PythonImport<'_>) -> ImportSourceResolution {
        let Some(module) = self.resolve_python_import(import) else {
            return ImportSourceResolution::Unresolved;
        };

        let detail = resolve_module_detail(self.db, self.project, module.name().clone());
        if !detail
            .selected_root
            .as_ref()
            .is_some_and(SearchPath::is_first_party)
        {
            return ImportSourceResolution::SkippedExternal;
        }

        self.read_resolved_module(&module)
    }
}
