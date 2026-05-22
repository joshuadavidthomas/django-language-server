use camino::Utf8PathBuf;
use djls_db::DjangoDatabase;
use djls_project::build_source_roots;
use djls_project::first_party_discovery_files_request;
use djls_project::first_party_source_files_load_request;
use djls_project::merge_first_party_source_file_patch;
use djls_project::Db as _;
use djls_project::FirstPartySourceFilePatch;
use djls_project::LoadingEffects;
use djls_project::ProjectSourceFilesApplyResult;
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
    fn begin_loading_run(&mut self) {
        djls_project::Db::begin_project_loading_run(self.db);
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
    ) -> ProjectSourceFilesApplyResult {
        let current = self
            .db
            .project_loading_state()
            .source_files(self.db)
            .ready_or_previous();
        let update = merge_first_party_source_file_patch(current.as_ref(), patch);
        self.db.apply_project_source_files(update)
    }
}
