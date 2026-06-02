use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;

use crate::apps::InstalledAppFileRoots;
use crate::enrichment::ProjectEnrichment;
use crate::python::PythonSourceIndexIssue;
use crate::root_discovery::load_project_root_discovery;
use crate::root_discovery::ProjectRootDiscovery;
use crate::root_discovery::ProjectRootDiscoveryLoadRequest;
use crate::root_discovery::ProjectRootDiscoveryUpdate;
use crate::source_files::build_source_roots;
use crate::source_files::first_party_discovery_files_request;
use crate::source_files::source_files_update_from_first_party_patch;
use crate::source_files::ReadySourceFiles;
use crate::source_files::SourceFilePartitionPatch;
use crate::source_files::SourceFilePartitionReadiness;
use crate::source_files::SourceFilesApplyResult;
use crate::source_files::SourceFilesIssue;
use crate::source_files::SourceFilesUpdate;
use crate::templates::template_directory_files_request;
use crate::templates::template_directory_source_files_update;
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::PythonSourceIndexOutcome;

#[derive(Clone, Debug, PartialEq)]
pub struct DjangoDiscoveryRequest {
    workspace_roots: Vec<Utf8PathBuf>,
    client_settings: Settings,
}

impl DjangoDiscoveryRequest {
    #[must_use]
    pub fn new(workspace_roots: Vec<Utf8PathBuf>, client_settings: Settings) -> Self {
        Self {
            workspace_roots,
            client_settings,
        }
    }

    #[must_use]
    pub fn workspace_roots(&self) -> &[Utf8PathBuf] {
        &self.workspace_roots
    }

    #[must_use]
    pub fn client_settings(&self) -> &Settings {
        &self.client_settings
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DiscoveryStage {
    SourceFiles,
    ProjectRootDiscovery,
    PythonSourceModels,
    DjangoEnvironments,
    InstalledAppFiles,
    TemplateDirectoryFiles,
    Enrichment,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DiscoveryMilestone {
    WorkspaceReady,
    DjangoAppsReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryStageStatus {
    Succeeded,
    Degraded,
    Deferred,
    Skipped,
    Unavailable,
    Failed,
    Superseded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryExecutionOutcome {
    Superseded,
    StaleSnapshot,
}

pub type DiscoveryApply<T> = Result<T, DiscoveryExecutionOutcome>;
pub type DiscoveryObservation<T> = Result<T, DiscoveryExecutionOutcome>;

pub trait DiscoveryHost {
    fn checkpoint(&mut self) -> Result<(), DiscoveryExecutionOutcome>;

    fn load_files_for_roots(
        &mut self,
        request: FilesForRootsRequest,
    ) -> Result<FilesForRootsResult, DiscoveryExecutionOutcome>;

    fn current_source_files(&mut self) -> Option<ReadySourceFiles>;

    fn apply_source_files(
        &mut self,
        update: SourceFilesUpdate,
    ) -> Result<SourceFilesApplyResult, DiscoveryExecutionOutcome>;

    fn apply_project_root_discovery(
        &mut self,
        update: ProjectRootDiscoveryUpdate,
    ) -> Result<ProjectRootDiscovery, DiscoveryExecutionOutcome>;

    fn observe_python_source_index(
        &mut self,
    ) -> Result<PythonSourceIndexOutcome, DiscoveryExecutionOutcome>;

    fn observe_django_environment_candidates(
        &mut self,
    ) -> Result<DjangoEnvironmentCandidatesOutcome, DiscoveryExecutionOutcome>;

    fn observe_installed_app_file_roots(
        &mut self,
    ) -> Result<Option<InstalledAppFileRoots>, DiscoveryExecutionOutcome>;

    fn observe_template_directory_file_roots(
        &mut self,
    ) -> Result<Option<Vec<Utf8PathBuf>>, DiscoveryExecutionOutcome>;

    fn load_project_enrichment(&mut self) -> Result<ProjectEnrichment, DiscoveryExecutionOutcome>;

    fn apply_project_enrichment(
        &mut self,
        enrichment: ProjectEnrichment,
    ) -> Result<ProjectEnrichment, DiscoveryExecutionOutcome>;
}

pub trait DiscoveryObserver {
    fn stage_started(&mut self, stage: DiscoveryStage);
    fn stage_finished(&mut self, stage: DiscoveryStage, status: DiscoveryStageStatus);
    fn milestone_reached(&mut self, _milestone: DiscoveryMilestone, _status: DiscoveryStageStatus) {
    }
}

#[derive(Default)]
pub struct NoopDiscoveryObserver;

impl DiscoveryObserver for NoopDiscoveryObserver {
    fn stage_started(&mut self, _stage: DiscoveryStage) {}

    fn stage_finished(&mut self, _stage: DiscoveryStage, _status: DiscoveryStageStatus) {}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryRunResult {
    stage_records: Vec<DiscoveryStageRecord>,
    milestone_results: Vec<DiscoveryMilestoneResult>,
    execution_outcome: Option<DiscoveryExecutionOutcome>,
}

impl DiscoveryRunResult {
    #[must_use]
    pub fn completed(
        stage_records: Vec<DiscoveryStageRecord>,
        milestone_results: Vec<DiscoveryMilestoneResult>,
    ) -> Self {
        Self {
            stage_records,
            milestone_results,
            execution_outcome: None,
        }
    }

    #[must_use]
    pub fn aborted(
        stage_records: Vec<DiscoveryStageRecord>,
        milestone_results: Vec<DiscoveryMilestoneResult>,
        execution_outcome: DiscoveryExecutionOutcome,
    ) -> Self {
        Self {
            stage_records,
            milestone_results,
            execution_outcome: Some(execution_outcome),
        }
    }

    #[must_use]
    pub fn stage_records(&self) -> &[DiscoveryStageRecord] {
        &self.stage_records
    }

    #[must_use]
    pub fn milestone_results(&self) -> &[DiscoveryMilestoneResult] {
        &self.milestone_results
    }

    #[must_use]
    pub fn execution_outcome(&self) -> Option<&DiscoveryExecutionOutcome> {
        self.execution_outcome.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryMilestoneResult {
    milestone: DiscoveryMilestone,
    status: DiscoveryStageStatus,
}

impl DiscoveryMilestoneResult {
    #[must_use]
    pub fn milestone(&self) -> DiscoveryMilestone {
        self.milestone
    }

    #[must_use]
    pub fn status(&self) -> DiscoveryStageStatus {
        self.status
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryStageRecord {
    stage: DiscoveryStage,
    status: DiscoveryStageStatus,
}

impl DiscoveryStageRecord {
    #[must_use]
    pub fn new(stage: DiscoveryStage, status: DiscoveryStageStatus) -> Self {
        Self { stage, status }
    }

    #[must_use]
    pub fn stage(&self) -> DiscoveryStage {
        self.stage
    }

    #[must_use]
    pub fn status(&self) -> &DiscoveryStageStatus {
        &self.status
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MilestoneSpec {
    milestone: DiscoveryMilestone,
    prerequisites: &'static [MilestonePrerequisite],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MilestonePrerequisite {
    stage: DiscoveryStage,
    acceptable_statuses: &'static [DiscoveryStageStatus],
}

const DISCOVERY_STAGES: &[DiscoveryStage] = &[
    DiscoveryStage::SourceFiles,
    DiscoveryStage::ProjectRootDiscovery,
    DiscoveryStage::PythonSourceModels,
    DiscoveryStage::DjangoEnvironments,
    DiscoveryStage::InstalledAppFiles,
    DiscoveryStage::TemplateDirectoryFiles,
    DiscoveryStage::Enrichment,
];

const MILESTONE_SPECS: &[MilestoneSpec] = &[
    MilestoneSpec {
        milestone: DiscoveryMilestone::WorkspaceReady,
        prerequisites: &[
            MilestonePrerequisite {
                stage: DiscoveryStage::SourceFiles,
                acceptable_statuses: &[DiscoveryStageStatus::Succeeded],
            },
            MilestonePrerequisite {
                stage: DiscoveryStage::PythonSourceModels,
                acceptable_statuses: &[
                    DiscoveryStageStatus::Succeeded,
                    DiscoveryStageStatus::Skipped,
                ],
            },
            MilestonePrerequisite {
                stage: DiscoveryStage::DjangoEnvironments,
                acceptable_statuses: &[
                    DiscoveryStageStatus::Succeeded,
                    DiscoveryStageStatus::Degraded,
                ],
            },
        ],
    },
    MilestoneSpec {
        milestone: DiscoveryMilestone::DjangoAppsReady,
        prerequisites: &[
            MilestonePrerequisite {
                stage: DiscoveryStage::InstalledAppFiles,
                acceptable_statuses: &[
                    DiscoveryStageStatus::Succeeded,
                    DiscoveryStageStatus::Skipped,
                ],
            },
            MilestonePrerequisite {
                stage: DiscoveryStage::TemplateDirectoryFiles,
                acceptable_statuses: &[
                    DiscoveryStageStatus::Succeeded,
                    DiscoveryStageStatus::Skipped,
                ],
            },
        ],
    },
];

fn enrichment_status(enrichment: &ProjectEnrichment) -> DiscoveryStageStatus {
    match enrichment {
        ProjectEnrichment::Absent | ProjectEnrichment::RuntimeUnavailable => {
            DiscoveryStageStatus::Skipped
        }
        ProjectEnrichment::Fresh(_) => DiscoveryStageStatus::Succeeded,
        ProjectEnrichment::InspectorFailed => DiscoveryStageStatus::Failed,
    }
}

fn source_files_status(result: &SourceFilesApplyResult) -> DiscoveryStageStatus {
    match result {
        SourceFilesApplyResult::Applied(applied) => {
            file_partition_readiness_status(applied.transition().readiness())
        }
        SourceFilesApplyResult::Deferred { .. } => DiscoveryStageStatus::Deferred,
        SourceFilesApplyResult::Unavailable { .. } => DiscoveryStageStatus::Unavailable,
        SourceFilesApplyResult::Failed { .. } => DiscoveryStageStatus::Failed,
    }
}

fn project_root_discovery_status(discovery: &ProjectRootDiscovery) -> DiscoveryStageStatus {
    match discovery {
        ProjectRootDiscovery::Ready(roots)
            if roots.iter().any(|root| !root.issues().is_empty()) =>
        {
            DiscoveryStageStatus::Degraded
        }
        ProjectRootDiscovery::Ready(_) => DiscoveryStageStatus::Succeeded,
        ProjectRootDiscovery::Absent => DiscoveryStageStatus::Deferred,
        ProjectRootDiscovery::NoWorkspaceRoots
        | ProjectRootDiscovery::FixtureDoesNotModelDiscovery => DiscoveryStageStatus::Unavailable,
    }
}

fn django_environment_candidates_status(
    result: &DjangoEnvironmentCandidatesOutcome,
) -> DiscoveryStageStatus {
    match result {
        DjangoEnvironmentCandidatesOutcome::Ready(_) => DiscoveryStageStatus::Succeeded,
        DjangoEnvironmentCandidatesOutcome::Deferred => DiscoveryStageStatus::Deferred,
    }
}

fn python_source_index_status(result: &PythonSourceIndexOutcome) -> DiscoveryStageStatus {
    match result {
        PythonSourceIndexOutcome::Ready(_) => DiscoveryStageStatus::Succeeded,
        PythonSourceIndexOutcome::Unindexed(PythonSourceIndexIssue::NoPythonFiles) => {
            DiscoveryStageStatus::Skipped
        }
        PythonSourceIndexOutcome::Unindexed(
            PythonSourceIndexIssue::SourceInventoryUnavailable(SourceFilesIssue::NotLoaded),
        ) => DiscoveryStageStatus::Deferred,
        PythonSourceIndexOutcome::Unindexed(
            PythonSourceIndexIssue::LayoutUnavailable
            | PythonSourceIndexIssue::SourceInventoryUnavailable(_),
        ) => DiscoveryStageStatus::Unavailable,
    }
}

fn file_partition_readiness_status(
    readiness: &SourceFilePartitionReadiness,
) -> DiscoveryStageStatus {
    match readiness {
        SourceFilePartitionReadiness::Ready { .. } => DiscoveryStageStatus::Succeeded,
        SourceFilePartitionReadiness::Skipped { .. } => DiscoveryStageStatus::Skipped,
        SourceFilePartitionReadiness::Unavailable { .. } => DiscoveryStageStatus::Unavailable,
        SourceFilePartitionReadiness::Loading
        | SourceFilePartitionReadiness::Deferred { .. }
        | SourceFilePartitionReadiness::Stale { .. } => DiscoveryStageStatus::Deferred,
    }
}

#[allow(clippy::too_many_lines)]
pub fn run_django_discovery(
    request: &DjangoDiscoveryRequest,
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> DiscoveryRunResult {
    let mut stage_records = Vec::with_capacity(DISCOVERY_STAGES.len());
    let mut milestone_results = Vec::with_capacity(MILESTONE_SPECS.len());

    if let Err(outcome) = host.checkpoint() {
        return DiscoveryRunResult::aborted(stage_records, milestone_results, outcome);
    }

    for stage in DISCOVERY_STAGES {
        let result = match stage {
            DiscoveryStage::SourceFiles => run_source_files_stage(request, host, observer),
            DiscoveryStage::ProjectRootDiscovery => {
                run_project_root_discovery_stage(request, host, observer)
            }
            DiscoveryStage::PythonSourceModels => run_python_source_models_stage(host, observer),
            DiscoveryStage::DjangoEnvironments => run_django_environments_stage(host, observer),
            DiscoveryStage::InstalledAppFiles => run_installed_app_files_stage(host, observer),
            DiscoveryStage::TemplateDirectoryFiles => {
                run_template_directory_files_stage(host, observer)
            }
            DiscoveryStage::Enrichment => run_enrichment_stage(host, observer),
        };
        match result {
            Ok(result) => stage_records.push(result),
            Err(outcome) => {
                return DiscoveryRunResult::aborted(stage_records, milestone_results, outcome)
            }
        }
        advance_milestones(&stage_records, &mut milestone_results, observer);
    }

    DiscoveryRunResult::completed(stage_records, milestone_results)
}

fn run_source_files_stage(
    request: &DjangoDiscoveryRequest,
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageRecord, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::SourceFiles;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let plan = build_source_roots(request.workspace_roots.clone());
    let (root_issues, files_request) = first_party_discovery_files_request(plan);
    let applied =
        load_and_apply_source_files(host, files_request, stage, observer, |previous, files| {
            let patch = SourceFilePartitionPatch::first_party(root_issues, files);
            source_files_update_from_first_party_patch(previous, patch)
        })?;
    let status = source_files_status(&applied);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageRecord::new(stage, status))
}

fn run_project_root_discovery_stage(
    request: &DjangoDiscoveryRequest,
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageRecord, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::ProjectRootDiscovery;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let roots = build_source_roots(request.workspace_roots.clone())
        .roots()
        .iter()
        .map(|root| root.path().to_owned())
        .collect();
    let update = load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
        roots,
        request.client_settings.clone(),
    ));
    let applied = apply_or_abort(stage, observer, || {
        host.apply_project_root_discovery(update)
    })?;
    let status = project_root_discovery_status(&applied);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageRecord::new(stage, status))
}

fn run_python_source_models_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageRecord, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::PythonSourceModels;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let source_index = observe_or_abort(stage, observer, || host.observe_python_source_index())?;
    let status = python_source_index_status(&source_index);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageRecord::new(stage, status))
}

fn run_django_environments_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageRecord, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::DjangoEnvironments;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let environment_candidates = observe_or_abort(stage, observer, || {
        host.observe_django_environment_candidates()
    })?;
    let status = django_environment_candidates_status(&environment_candidates);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageRecord::new(stage, status))
}

fn run_installed_app_files_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageRecord, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::InstalledAppFiles;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let discovery = observe_or_abort(stage, observer, || host.observe_installed_app_file_roots())?;
    let status = match discovery {
        Some(roots) => {
            let issues = roots.issues().to_vec();
            let applied = load_and_apply_source_files(
                host,
                roots.files_request(),
                stage,
                observer,
                |previous, result| roots.source_files_update(previous, result),
            )?;
            stage_status_with_discovery_issues(&applied, &issues)
        }
        None => DiscoveryStageStatus::Deferred,
    };
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageRecord::new(stage, status))
}

fn run_template_directory_files_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageRecord, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::TemplateDirectoryFiles;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let discovery = observe_or_abort(stage, observer, || {
        host.observe_template_directory_file_roots()
    })?;
    let status = match discovery {
        Some(roots) => {
            let applied = load_and_apply_source_files(
                host,
                template_directory_files_request(roots),
                stage,
                observer,
                template_directory_source_files_update,
            )?;
            source_files_status(&applied)
        }
        None => DiscoveryStageStatus::Deferred,
    };
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageRecord::new(stage, status))
}

fn run_enrichment_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageRecord, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::Enrichment;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let enrichment = observe_or_abort(stage, observer, || host.load_project_enrichment())?;
    checkpoint(host, stage, observer)?;
    let applied = apply_or_abort(stage, observer, || {
        host.apply_project_enrichment(enrichment)
    })?;
    let status = enrichment_status(&applied);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageRecord::new(stage, status))
}

fn checkpoint(
    host: &mut impl DiscoveryHost,
    stage: DiscoveryStage,
    observer: &mut impl DiscoveryObserver,
) -> Result<(), DiscoveryExecutionOutcome> {
    match host.checkpoint() {
        Ok(()) => Ok(()),
        Err(outcome) => {
            finish_aborted_stage(stage, &outcome, observer);
            Err(outcome)
        }
    }
}

fn observe_or_abort<T>(
    stage: DiscoveryStage,
    observer: &mut impl DiscoveryObserver,
    observe: impl FnOnce() -> DiscoveryObservation<T>,
) -> Result<T, DiscoveryExecutionOutcome> {
    match observe() {
        Ok(result) => Ok(result),
        Err(outcome) => {
            finish_aborted_stage(stage, &outcome, observer);
            Err(outcome)
        }
    }
}

fn apply_or_abort<T>(
    stage: DiscoveryStage,
    observer: &mut impl DiscoveryObserver,
    apply: impl FnOnce() -> DiscoveryApply<T>,
) -> Result<T, DiscoveryExecutionOutcome> {
    match apply() {
        Ok(result) => Ok(result),
        Err(outcome) => {
            finish_aborted_stage(stage, &outcome, observer);
            Err(outcome)
        }
    }
}

fn load_files_for_roots(
    host: &mut impl DiscoveryHost,
    request: FilesForRootsRequest,
    stage: DiscoveryStage,
    observer: &mut impl DiscoveryObserver,
) -> Result<FilesForRootsResult, DiscoveryExecutionOutcome> {
    observe_or_abort(stage, observer, || host.load_files_for_roots(request))
}

fn load_and_apply_source_files(
    host: &mut impl DiscoveryHost,
    request: FilesForRootsRequest,
    stage: DiscoveryStage,
    observer: &mut impl DiscoveryObserver,
    update: impl FnOnce(Option<&ReadySourceFiles>, FilesForRootsResult) -> SourceFilesUpdate,
) -> Result<SourceFilesApplyResult, DiscoveryExecutionOutcome> {
    let result = load_files_for_roots(host, request, stage, observer)?;
    checkpoint(host, stage, observer)?;
    let previous = host.current_source_files();
    let update = update(previous.as_ref(), result);
    apply_or_abort(stage, observer, || host.apply_source_files(update))
}

fn stage_status_with_discovery_issues(
    applied: &SourceFilesApplyResult,
    issues: &[SourceFilesIssue],
) -> DiscoveryStageStatus {
    let status = source_files_status(applied);
    if issues.is_empty() {
        return status;
    }
    match status {
        DiscoveryStageStatus::Succeeded | DiscoveryStageStatus::Skipped => {
            DiscoveryStageStatus::Degraded
        }
        status => status,
    }
}

fn advance_milestones(
    stage_records: &[DiscoveryStageRecord],
    milestone_results: &mut Vec<DiscoveryMilestoneResult>,
    observer: &mut impl DiscoveryObserver,
) {
    for milestone in MILESTONE_SPECS {
        if milestone_results
            .iter()
            .any(|result| result.milestone == milestone.milestone)
        {
            continue;
        }
        let statuses = milestone
            .prerequisites
            .iter()
            .map(|prerequisite| {
                stage_records
                    .iter()
                    .find(|record| record.stage() == prerequisite.stage)
                    .map(DiscoveryStageRecord::status)
                    .filter(|status| prerequisite.acceptable_statuses.contains(status))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(statuses) = statuses {
            let status = if statuses
                .iter()
                .all(|status| **status == DiscoveryStageStatus::Succeeded)
            {
                DiscoveryStageStatus::Succeeded
            } else {
                DiscoveryStageStatus::Degraded
            };
            milestone_results.push(DiscoveryMilestoneResult {
                milestone: milestone.milestone,
                status,
            });
            observer.milestone_reached(milestone.milestone, status);
        }
    }
}

fn finish_aborted_stage(
    stage: DiscoveryStage,
    outcome: &DiscoveryExecutionOutcome,
    observer: &mut impl DiscoveryObserver,
) {
    if matches!(outcome, DiscoveryExecutionOutcome::Superseded) {
        observer.stage_finished(stage, DiscoveryStageStatus::Superseded);
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::FileSetSummary;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_workspace::load_files_for_roots;

    use super::*;
    use crate::python::PythonSourceIndex;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::source_files::SourceFilesApplied;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    #[derive(Default)]
    #[allow(clippy::struct_excessive_bools)]
    struct FakeHost {
        checkpoint_count: usize,
        load_count: usize,
        apply_count: usize,
        discovery_apply_count: usize,
        python_observe_count: usize,
        environment_observe_count: usize,
        source_ready: bool,
        python_skipped: bool,
        installed_app_deferred: bool,
        template_directory_deferred: bool,
        cancel_on_checkpoint: Option<usize>,
        cancel_on_file_load: bool,
        applied_updates: Vec<SourceFilesUpdate>,
    }

    impl DiscoveryHost for FakeHost {
        fn checkpoint(&mut self) -> Result<(), DiscoveryExecutionOutcome> {
            self.checkpoint_count += 1;
            if self.cancel_on_checkpoint == Some(self.checkpoint_count) {
                return Err(DiscoveryExecutionOutcome::Superseded);
            }
            Ok(())
        }

        fn load_files_for_roots(
            &mut self,
            request: FilesForRootsRequest,
        ) -> Result<FilesForRootsResult, DiscoveryExecutionOutcome> {
            self.load_count += 1;
            if self.cancel_on_file_load {
                return Err(DiscoveryExecutionOutcome::Superseded);
            }
            Ok(load_files_for_roots(request))
        }

        fn current_source_files(&mut self) -> Option<ReadySourceFiles> {
            None
        }

        fn apply_source_files(
            &mut self,
            update: SourceFilesUpdate,
        ) -> DiscoveryApply<SourceFilesApplyResult> {
            self.apply_count += 1;
            if self.source_ready || update.applied_transition().partitions().is_empty() {
                self.applied_updates.push(update);
                let db = TestDb::default();
                let set = SourceFileSet::new(&db, SourceFileSetData::default());
                let files = ReadySourceFiles::materialized_for_test(
                    crate::source_files::SourceFileSetPartitions::default(),
                    set,
                );
                return Ok(SourceFilesApplyResult::Applied(
                    SourceFilesApplied::for_test(
                        files,
                        crate::source_files::SourceFilePartitionReadiness::Ready {
                            summary: FileSetSummary::new(0),
                        },
                    ),
                ));
            }
            let transition = update.applied_transition().clone();
            let issue = update
                .issues()
                .first()
                .cloned()
                .unwrap_or(crate::SourceFilesIssue::NotLoaded);
            self.applied_updates.push(update);
            Ok(SourceFilesApplyResult::Unavailable {
                transition,
                issue,
                previous: None,
            })
        }

        fn apply_project_root_discovery(
            &mut self,
            update: ProjectRootDiscoveryUpdate,
        ) -> DiscoveryApply<ProjectRootDiscovery> {
            self.discovery_apply_count += 1;
            if update.roots().is_empty() {
                Ok(ProjectRootDiscovery::NoWorkspaceRoots)
            } else {
                Ok(ProjectRootDiscovery::Absent)
            }
        }

        fn observe_python_source_index(
            &mut self,
        ) -> DiscoveryObservation<PythonSourceIndexOutcome> {
            self.python_observe_count += 1;
            if self.python_skipped {
                return Ok(PythonSourceIndexOutcome::Unindexed(
                    PythonSourceIndexIssue::NoPythonFiles,
                ));
            }
            Ok(PythonSourceIndexOutcome::Ready(PythonSourceIndex::default()))
        }

        fn observe_django_environment_candidates(
            &mut self,
        ) -> DiscoveryObservation<DjangoEnvironmentCandidatesOutcome> {
            self.environment_observe_count += 1;
            Ok(DjangoEnvironmentCandidatesOutcome::Ready(Vec::new()))
        }

        fn observe_installed_app_file_roots(
            &mut self,
        ) -> DiscoveryObservation<Option<InstalledAppFileRoots>> {
            if self.installed_app_deferred {
                return Ok(None);
            }
            Ok(Some(crate::apps::InstalledAppFileRoots::new(
                Vec::new(),
                Vec::new(),
            )))
        }

        fn observe_template_directory_file_roots(
            &mut self,
        ) -> DiscoveryObservation<Option<Vec<Utf8PathBuf>>> {
            if self.template_directory_deferred {
                return Ok(None);
            }
            Ok(Some(Vec::new()))
        }

        fn load_project_enrichment(
            &mut self,
        ) -> Result<ProjectEnrichment, DiscoveryExecutionOutcome> {
            Ok(ProjectEnrichment::RuntimeUnavailable)
        }

        fn apply_project_enrichment(
            &mut self,
            enrichment: ProjectEnrichment,
        ) -> DiscoveryApply<ProjectEnrichment> {
            Ok(enrichment)
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Vec<(DiscoveryStage, DiscoveryStageStatus)>,
        milestones: Vec<(DiscoveryMilestone, DiscoveryStageStatus)>,
        started: Vec<DiscoveryStage>,
    }

    impl DiscoveryObserver for RecordingObserver {
        fn stage_started(&mut self, stage: DiscoveryStage) {
            self.started.push(stage);
        }

        fn stage_finished(&mut self, stage: DiscoveryStage, status: DiscoveryStageStatus) {
            self.events.push((stage, status));
        }

        fn milestone_reached(
            &mut self,
            milestone: DiscoveryMilestone,
            status: DiscoveryStageStatus,
        ) {
            self.milestones.push((milestone, status));
        }
    }

    fn run_fake_discovery(host: &mut FakeHost) -> (DiscoveryRunResult, RecordingObserver) {
        let mut observer = RecordingObserver::default();
        let request = DjangoDiscoveryRequest::new(
            vec![Utf8PathBuf::from("/missing")],
            djls_conf::Settings::default(),
        );
        let result = run_django_discovery(&request, host, &mut observer);
        (result, observer)
    }

    #[test]
    fn django_discovery_workspace_ready_milestone_advances_after_environment_discovery() {
        let mut host = FakeHost {
            source_ready: true,
            ..FakeHost::default()
        };
        let (result, observer) = run_fake_discovery(&mut host);

        assert_eq!(host.load_count, 3);
        assert_eq!(host.apply_count, 3);
        assert_eq!(host.discovery_apply_count, 1);
        assert_eq!(host.python_observe_count, 1);
        assert_eq!(host.environment_observe_count, 1);
        assert_eq!(
            observer.started,
            vec![
                DiscoveryStage::SourceFiles,
                DiscoveryStage::ProjectRootDiscovery,
                DiscoveryStage::PythonSourceModels,
                DiscoveryStage::DjangoEnvironments,
                DiscoveryStage::InstalledAppFiles,
                DiscoveryStage::TemplateDirectoryFiles,
                DiscoveryStage::Enrichment,
            ]
        );
        assert_eq!(
            observer.events,
            vec![
                (DiscoveryStage::SourceFiles, DiscoveryStageStatus::Succeeded),
                (
                    DiscoveryStage::ProjectRootDiscovery,
                    DiscoveryStageStatus::Deferred,
                ),
                (
                    DiscoveryStage::PythonSourceModels,
                    DiscoveryStageStatus::Succeeded,
                ),
                (
                    DiscoveryStage::DjangoEnvironments,
                    DiscoveryStageStatus::Succeeded,
                ),
                (
                    DiscoveryStage::InstalledAppFiles,
                    DiscoveryStageStatus::Succeeded,
                ),
                (
                    DiscoveryStage::TemplateDirectoryFiles,
                    DiscoveryStageStatus::Succeeded,
                ),
                (DiscoveryStage::Enrichment, DiscoveryStageStatus::Skipped),
            ]
        );
        assert_eq!(
            result.stage_records()[0].stage(),
            DiscoveryStage::SourceFiles
        );
        assert_eq!(
            result.stage_records()[1].stage(),
            DiscoveryStage::ProjectRootDiscovery,
        );
        assert_eq!(
            result.stage_records()[2].stage(),
            DiscoveryStage::PythonSourceModels,
        );
        assert_eq!(
            result.stage_records()[3].stage(),
            DiscoveryStage::DjangoEnvironments,
        );
        assert_eq!(
            observer.milestones,
            vec![
                (
                    DiscoveryMilestone::WorkspaceReady,
                    DiscoveryStageStatus::Succeeded,
                ),
                (
                    DiscoveryMilestone::DjangoAppsReady,
                    DiscoveryStageStatus::Succeeded,
                ),
            ]
        );
        assert_eq!(
            result.milestone_results()[0].milestone(),
            DiscoveryMilestone::WorkspaceReady,
        );
        assert_eq!(
            result.milestone_results()[0].status(),
            DiscoveryStageStatus::Succeeded,
        );
    }

    #[test]
    fn django_discovery_workspace_ready_milestone_advances_degraded_for_accepted_statuses() {
        let mut host = FakeHost {
            source_ready: true,
            python_skipped: true,
            ..FakeHost::default()
        };

        let (result, observer) = run_fake_discovery(&mut host);

        assert_eq!(
            observer.milestones,
            vec![
                (
                    DiscoveryMilestone::WorkspaceReady,
                    DiscoveryStageStatus::Degraded,
                ),
                (
                    DiscoveryMilestone::DjangoAppsReady,
                    DiscoveryStageStatus::Succeeded,
                ),
            ]
        );
        assert_eq!(
            result.milestone_results()[0].status(),
            DiscoveryStageStatus::Degraded,
        );
    }

    #[test]
    fn django_discovery_enrichment_runs_last_and_is_not_a_milestone_prerequisite() {
        let mut host = FakeHost {
            source_ready: true,
            ..FakeHost::default()
        };

        let (result, observer) = run_fake_discovery(&mut host);

        assert_eq!(
            result.stage_records().last().unwrap().stage(),
            DiscoveryStage::Enrichment,
        );
        assert_eq!(
            result.stage_records().last().unwrap().status(),
            &DiscoveryStageStatus::Skipped,
        );
        assert!(observer
            .events
            .contains(&(DiscoveryStage::Enrichment, DiscoveryStageStatus::Skipped,)));
        assert!(observer.milestones.iter().all(|(milestone, _)| matches!(
            milestone,
            DiscoveryMilestone::WorkspaceReady | DiscoveryMilestone::DjangoAppsReady,
        )));
    }

    #[test]
    fn django_discovery_django_apps_ready_milestone_waits_for_deferred_app_files() {
        let mut host = FakeHost {
            source_ready: true,
            installed_app_deferred: true,
            ..FakeHost::default()
        };

        let (_result, observer) = run_fake_discovery(&mut host);

        assert!(observer.events.contains(&(
            DiscoveryStage::InstalledAppFiles,
            DiscoveryStageStatus::Deferred,
        )));
        assert!(observer.events.contains(&(
            DiscoveryStage::TemplateDirectoryFiles,
            DiscoveryStageStatus::Succeeded,
        )));
        assert!(!observer
            .milestones
            .iter()
            .any(|(milestone, _)| *milestone == DiscoveryMilestone::DjangoAppsReady));
    }

    #[test]
    fn django_discovery_workspace_ready_milestone_does_not_advance_for_unavailable_source_files() {
        let mut host = FakeHost::default();

        let (result, observer) = run_fake_discovery(&mut host);

        assert_eq!(
            observer.milestones,
            vec![(
                DiscoveryMilestone::DjangoAppsReady,
                DiscoveryStageStatus::Succeeded,
            )]
        );
        assert_eq!(result.milestone_results().len(), 1);
    }

    #[test]
    fn django_discovery_superseded_before_file_load_aborts_without_apply() {
        let mut host = FakeHost {
            cancel_on_checkpoint: Some(2),
            source_ready: true,
            ..FakeHost::default()
        };

        let (result, observer) = run_fake_discovery(&mut host);

        assert_eq!(
            result.execution_outcome(),
            Some(&DiscoveryExecutionOutcome::Superseded),
        );
        assert_eq!(host.load_count, 0);
        assert_eq!(host.apply_count, 0);
        assert_eq!(
            observer.events,
            vec![(
                DiscoveryStage::SourceFiles,
                DiscoveryStageStatus::Superseded
            )]
        );
    }

    #[test]
    fn django_discovery_superseded_after_file_load_aborts_without_apply() {
        let mut host = FakeHost {
            cancel_on_checkpoint: Some(3),
            source_ready: true,
            ..FakeHost::default()
        };

        let (result, observer) = run_fake_discovery(&mut host);

        assert_eq!(
            result.execution_outcome(),
            Some(&DiscoveryExecutionOutcome::Superseded),
        );
        assert_eq!(host.load_count, 1);
        assert_eq!(host.apply_count, 0);
        assert_eq!(
            observer.events,
            vec![(
                DiscoveryStage::SourceFiles,
                DiscoveryStageStatus::Superseded
            )]
        );
    }
}
