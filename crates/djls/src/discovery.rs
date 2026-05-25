use djls_db::DjangoDatabase;
use djls_project::installed_app_file_roots_discovery;
use djls_project::template_directory_file_roots_discovery;
use djls_project::Db as ProjectDb;
use djls_project::DiscoveryApply;
use djls_project::DiscoveryExecutionOutcome;
use djls_project::DiscoveryHost;
use djls_project::DiscoveryObservation;
use djls_project::DjangoEnvironmentCandidatesOutcome;
use djls_project::InstalledAppFileRoots;
use djls_project::ProjectEnrichment;
use djls_project::ProjectRootDiscovery;
use djls_project::ProjectRootDiscoveryUpdate;
use djls_project::PythonSourceIndexOutcome;
use djls_project::ReadySourceFiles;
use djls_project::SourceFilesApplyResult;
use djls_project::SourceFilesUpdate;
use djls_workspace::load_files_for_roots;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;

pub(crate) struct CliDiscoveryHost<'db> {
    db: &'db mut DjangoDatabase,
}

impl<'db> CliDiscoveryHost<'db> {
    pub(crate) fn new(db: &'db mut DjangoDatabase) -> Self {
        Self { db }
    }
}

impl DiscoveryHost for CliDiscoveryHost<'_> {
    fn checkpoint(&mut self) -> Result<(), DiscoveryExecutionOutcome> {
        Ok(())
    }

    fn load_files_for_roots(
        &mut self,
        request: FilesForRootsRequest,
    ) -> Result<FilesForRootsResult, DiscoveryExecutionOutcome> {
        Ok(load_files_for_roots(request))
    }

    fn current_source_files(&mut self) -> Option<ReadySourceFiles> {
        ProjectDb::project(self.db)
            .source_inventory(self.db)
            .ready()
    }

    fn apply_source_files(
        &mut self,
        update: SourceFilesUpdate,
    ) -> DiscoveryApply<SourceFilesApplyResult> {
        Ok(self.db.apply_source_files(update))
    }

    fn apply_project_root_discovery(
        &mut self,
        update: ProjectRootDiscoveryUpdate,
    ) -> DiscoveryApply<ProjectRootDiscovery> {
        Ok(self.db.apply_project_root_discovery(update))
    }

    fn observe_python_source_index(&mut self) -> DiscoveryObservation<PythonSourceIndexOutcome> {
        let project = ProjectDb::project(self.db);
        Ok(djls_project::python_source_index(self.db, project).clone())
    }

    fn observe_django_environment_candidates(
        &mut self,
    ) -> DiscoveryObservation<DjangoEnvironmentCandidatesOutcome> {
        let project = ProjectDb::project(self.db);
        Ok(djls_project::django_environment_candidates(self.db, project).clone())
    }

    fn observe_installed_app_file_roots(
        &mut self,
    ) -> DiscoveryObservation<Option<InstalledAppFileRoots>> {
        let project = ProjectDb::project(self.db);
        Ok(installed_app_file_roots_discovery(self.db, project))
    }

    fn observe_template_directory_file_roots(
        &mut self,
    ) -> DiscoveryObservation<Option<Vec<camino::Utf8PathBuf>>> {
        let project = ProjectDb::project(self.db);
        Ok(template_directory_file_roots_discovery(self.db, project))
    }

    fn load_project_enrichment(&mut self) -> Result<ProjectEnrichment, DiscoveryExecutionOutcome> {
        Ok(self.db.load_project_enrichment())
    }

    fn apply_project_enrichment(
        &mut self,
        enrichment: ProjectEnrichment,
    ) -> DiscoveryApply<ProjectEnrichment> {
        Ok(self.db.apply_enrichment(enrichment))
    }
}
