use camino::Utf8PathBuf;
use djls_db::DjangoDatabase;
use djls_project::build_source_roots;
use djls_project::first_party_discovery_files_request;
use djls_project::first_party_source_files_load_request;
use djls_project::installed_app_file_load_outcome;
use djls_project::merge_first_party_source_file_patch;
use djls_project::merge_partitioned_source_file_patch;
use djls_project::template_directory_file_load_outcome;
use djls_project::Db as ProjectDb;
use djls_project::DjangoEnvironmentCandidatesOutcome;
use djls_project::FirstPartySourceFilePatch;
use djls_project::LoadingApplyOutcome;
use djls_project::LoadingEffects;
use djls_project::LoadingObservationOutcome;
use djls_project::LoadingRunControl;
use djls_project::PartitionedSourceFilePatch;
use djls_project::ProjectEnrichment;
use djls_project::ProjectRootDiscoveryApplyResult;
use djls_project::ProjectRootDiscoveryLoadRequest;
use djls_project::ProjectRootDiscoveryUpdate;
use djls_project::PythonSourceIndexOutcome;
use djls_project::SourceFilesApplyResult;
use djls_workspace::load_files_for_roots;

pub(crate) struct CliLoadingExecutor<'db> {
    db: &'db mut DjangoDatabase,
    roots: Vec<Utf8PathBuf>,
}

impl<'db> CliLoadingExecutor<'db> {
    pub(crate) fn new(db: &'db mut DjangoDatabase, roots: Vec<Utf8PathBuf>) -> Self {
        Self { db, roots }
    }
}

impl LoadingEffects for CliLoadingExecutor<'_> {
    fn begin_loading_run(&mut self) -> LoadingRunControl {
        LoadingRunControl::Continue
    }

    fn load_source_file_set(&mut self) -> FirstPartySourceFilePatch {
        let plan = build_source_roots(self.roots.clone());
        let (root_issues, request) =
            first_party_discovery_files_request(first_party_source_files_load_request(plan));
        FirstPartySourceFilePatch::first_party(root_issues, load_files_for_roots(request))
    }

    fn apply_source_file_patch(
        &mut self,
        patch: FirstPartySourceFilePatch,
    ) -> LoadingApplyOutcome<SourceFilesApplyResult> {
        let current = ProjectDb::project(self.db)
            .source_inventory(self.db)
            .ready();
        let update = merge_first_party_source_file_patch(current.as_ref(), patch);
        LoadingApplyOutcome::Applied(self.db.apply_source_files(update))
    }

    fn load_project_discovery_set(&mut self) -> ProjectRootDiscoveryUpdate {
        let roots = build_source_roots(self.roots.clone())
            .roots()
            .iter()
            .map(|root| root.path().to_owned())
            .collect();
        djls_project::load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
            roots,
            self.db.settings(),
        ))
    }

    fn apply_project_root_discovery(
        &mut self,
        data: ProjectRootDiscoveryUpdate,
    ) -> LoadingApplyOutcome<ProjectRootDiscoveryApplyResult> {
        LoadingApplyOutcome::Applied(self.db.apply_project_root_discovery(data))
    }

    fn observe_python_source_index(
        &mut self,
    ) -> LoadingObservationOutcome<PythonSourceIndexOutcome> {
        let project = ProjectDb::project(self.db);
        LoadingObservationOutcome::Observed(
            djls_project::python_source_index(self.db, project).clone(),
        )
    }

    fn observe_django_environment_candidates(
        &mut self,
    ) -> LoadingObservationOutcome<DjangoEnvironmentCandidatesOutcome> {
        let project = ProjectDb::project(self.db);
        LoadingObservationOutcome::Observed(
            djls_project::django_environment_candidates(self.db, project).clone(),
        )
    }

    fn load_installed_app_file_patches(
        &mut self,
    ) -> djls_project::PartitionedSourceFileLoadOutcome {
        let project = ProjectDb::project(self.db);
        installed_app_file_load_outcome(self.db, project)
    }

    fn load_template_directory_file_patches(
        &mut self,
    ) -> djls_project::PartitionedSourceFileLoadOutcome {
        let project = ProjectDb::project(self.db);
        template_directory_file_load_outcome(self.db, project)
    }

    fn apply_partitioned_source_file_patch(
        &mut self,
        patch: PartitionedSourceFilePatch,
    ) -> LoadingApplyOutcome<SourceFilesApplyResult> {
        let current = ProjectDb::project(self.db)
            .source_inventory(self.db)
            .ready();
        let update = merge_partitioned_source_file_patch(current.as_ref(), patch);
        LoadingApplyOutcome::Applied(self.db.apply_source_files(update))
    }

    fn load_project_enrichment(&mut self) -> ProjectEnrichment {
        self.db.load_project_enrichment()
    }

    fn apply_project_enrichment(
        &mut self,
        enrichment: ProjectEnrichment,
    ) -> LoadingApplyOutcome<ProjectEnrichment> {
        LoadingApplyOutcome::Applied(self.db.apply_enrichment(enrichment))
    }
}
