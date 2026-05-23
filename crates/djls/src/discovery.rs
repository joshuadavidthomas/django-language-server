use djls_db::DjangoDatabase;
use djls_project::installed_app_file_roots_discovery;
use djls_project::template_directory_file_roots_discovery;
use djls_project::Db as ProjectDb;
use djls_project::DiscoveryApplyOutcome;
use djls_project::DiscoveryCancellation;
use djls_project::DiscoveryHost;
use djls_project::DiscoveryObservationOutcome;
use djls_project::DjangoEnvironmentCandidatesOutcome;
use djls_project::InstalledAppFileRootsOutcome;
use djls_project::ProjectEnrichment;
use djls_project::ProjectRootDiscoveryApplyResult;
use djls_project::ProjectRootDiscoveryUpdate;
use djls_project::PythonSourceIndexOutcome;
use djls_project::ReadySourceFiles;
use djls_project::SourceFilesApplyResult;
use djls_project::SourceFilesUpdate;
use djls_project::TemplateDirectoryFileRootsOutcome;
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
    fn checkpoint(&mut self) -> Result<(), DiscoveryCancellation> {
        Ok(())
    }

    fn load_files_for_roots(
        &mut self,
        request: FilesForRootsRequest,
    ) -> Result<FilesForRootsResult, DiscoveryCancellation> {
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
    ) -> DiscoveryApplyOutcome<SourceFilesApplyResult> {
        DiscoveryApplyOutcome::Applied(self.db.apply_source_files(update))
    }

    fn apply_project_root_discovery(
        &mut self,
        update: ProjectRootDiscoveryUpdate,
    ) -> DiscoveryApplyOutcome<ProjectRootDiscoveryApplyResult> {
        DiscoveryApplyOutcome::Applied(self.db.apply_project_root_discovery(update))
    }

    fn observe_python_source_index(
        &mut self,
    ) -> DiscoveryObservationOutcome<PythonSourceIndexOutcome> {
        let project = ProjectDb::project(self.db);
        DiscoveryObservationOutcome::Observed(
            djls_project::python_source_index(self.db, project).clone(),
        )
    }

    fn observe_django_environment_candidates(
        &mut self,
    ) -> DiscoveryObservationOutcome<DjangoEnvironmentCandidatesOutcome> {
        let project = ProjectDb::project(self.db);
        DiscoveryObservationOutcome::Observed(
            djls_project::django_environment_candidates(self.db, project).clone(),
        )
    }

    fn observe_installed_app_file_roots(
        &mut self,
    ) -> DiscoveryObservationOutcome<InstalledAppFileRootsOutcome> {
        let project = ProjectDb::project(self.db);
        DiscoveryObservationOutcome::Observed(installed_app_file_roots_discovery(self.db, project))
    }

    fn observe_template_directory_file_roots(
        &mut self,
    ) -> DiscoveryObservationOutcome<TemplateDirectoryFileRootsOutcome> {
        let project = ProjectDb::project(self.db);
        DiscoveryObservationOutcome::Observed(template_directory_file_roots_discovery(
            self.db, project,
        ))
    }

    fn load_project_enrichment(&mut self) -> Result<ProjectEnrichment, DiscoveryCancellation> {
        Ok(self.db.load_project_enrichment())
    }

    fn apply_project_enrichment(
        &mut self,
        enrichment: ProjectEnrichment,
    ) -> DiscoveryApplyOutcome<ProjectEnrichment> {
        DiscoveryApplyOutcome::Applied(self.db.apply_enrichment(enrichment))
    }
}
