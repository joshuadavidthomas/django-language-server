use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;

use crate::apps::InstalledAppFileRootsDiscovery;
use crate::enrichment::ProjectEnrichment;
use crate::enrichment::ProjectEnrichmentIssue;
use crate::python::PythonSourceIndexIssue;
use crate::root_discovery::load_project_root_discovery;
use crate::root_discovery::ProjectRootDiscovery;
use crate::root_discovery::ProjectRootDiscoveryApplyResult;
use crate::root_discovery::ProjectRootDiscoveryLoadRequest;
use crate::root_discovery::ProjectRootDiscoveryUpdate;
use crate::source_files::build_source_roots;
use crate::source_files::first_party_discovery_files_request;
use crate::source_files::first_party_source_files_load_request;
use crate::source_files::merge_first_party_source_file_patch;
use crate::source_files::FirstPartySourceFilePatch;
use crate::source_files::ReadySourceFiles;
use crate::source_files::SourceFilePartitionReadiness;
use crate::source_files::SourceFilesApplied;
use crate::source_files::SourceFilesApplyResult;
use crate::source_files::SourceFilesIssue;
use crate::source_files::SourceFilesUpdate;
use crate::templates::TemplateDirectoryFileRoots;
use crate::templates::TemplateDirectoryFileRootsDiscovery;
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
pub enum DiscoveryMilestoneStatus {
    Succeeded,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryCancellation {
    Superseded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryExecutionOutcome {
    Superseded,
    StaleSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryApplyOutcome<T> {
    Applied(T),
    Aborted(DiscoveryExecutionOutcome),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryObservationOutcome<T> {
    Observed(T),
    Cancelled(DiscoveryCancellation),
}

pub trait DiscoveryHost {
    fn checkpoint(&mut self) -> Result<(), DiscoveryCancellation>;

    fn load_files_for_roots(
        &mut self,
        request: FilesForRootsRequest,
    ) -> Result<FilesForRootsResult, DiscoveryCancellation>;

    fn current_source_files(&mut self) -> Option<ReadySourceFiles>;

    fn apply_source_files(
        &mut self,
        update: SourceFilesUpdate,
    ) -> DiscoveryApplyOutcome<SourceFilesApplyResult>;

    fn apply_project_root_discovery(
        &mut self,
        update: ProjectRootDiscoveryUpdate,
    ) -> DiscoveryApplyOutcome<ProjectRootDiscoveryApplyResult>;

    fn observe_python_source_index(
        &mut self,
    ) -> DiscoveryObservationOutcome<PythonSourceIndexOutcome>;

    fn observe_django_environment_candidates(
        &mut self,
    ) -> DiscoveryObservationOutcome<DjangoEnvironmentCandidatesOutcome>;

    fn observe_installed_app_file_roots(
        &mut self,
    ) -> DiscoveryObservationOutcome<InstalledAppFileRootsDiscovery>;

    fn observe_template_directory_file_roots(
        &mut self,
    ) -> DiscoveryObservationOutcome<TemplateDirectoryFileRootsDiscovery>;

    fn load_project_enrichment(&mut self) -> Result<ProjectEnrichment, DiscoveryCancellation>;

    fn apply_project_enrichment(
        &mut self,
        enrichment: ProjectEnrichment,
    ) -> DiscoveryApplyOutcome<ProjectEnrichment>;
}

pub trait DiscoveryObserver {
    fn stage_started(&mut self, stage: DiscoveryStage);
    fn stage_finished(&mut self, stage: DiscoveryStage, status: DiscoveryStageStatus);
    fn milestone_reached(
        &mut self,
        _milestone: DiscoveryMilestone,
        _status: DiscoveryMilestoneStatus,
    ) {
    }
}

#[derive(Default)]
pub struct NoopDiscoveryObserver;

impl DiscoveryObserver for NoopDiscoveryObserver {
    fn stage_started(&mut self, _stage: DiscoveryStage) {}

    fn stage_finished(&mut self, _stage: DiscoveryStage, _status: DiscoveryStageStatus) {}

    fn milestone_reached(
        &mut self,
        _milestone: DiscoveryMilestone,
        _status: DiscoveryMilestoneStatus,
    ) {
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryRunResult {
    stage_results: Vec<DiscoveryStageResult>,
    milestone_results: Vec<DiscoveryMilestoneResult>,
    execution_outcome: Option<DiscoveryExecutionOutcome>,
}

impl DiscoveryRunResult {
    #[must_use]
    pub fn completed(
        stage_results: Vec<DiscoveryStageResult>,
        milestone_results: Vec<DiscoveryMilestoneResult>,
    ) -> Self {
        Self {
            stage_results,
            milestone_results,
            execution_outcome: None,
        }
    }

    #[must_use]
    pub fn aborted(
        stage_results: Vec<DiscoveryStageResult>,
        milestone_results: Vec<DiscoveryMilestoneResult>,
        execution_outcome: DiscoveryExecutionOutcome,
    ) -> Self {
        Self {
            stage_results,
            milestone_results,
            execution_outcome: Some(execution_outcome),
        }
    }

    #[must_use]
    pub fn stage_results(&self) -> &[DiscoveryStageResult] {
        &self.stage_results
    }

    #[must_use]
    pub fn milestone_results(&self) -> &[DiscoveryMilestoneResult] {
        &self.milestone_results
    }

    #[must_use]
    pub fn execution_outcome(&self) -> Option<&DiscoveryExecutionOutcome> {
        self.execution_outcome.as_ref()
    }

    #[must_use]
    pub fn source_file_set_result(&self) -> Option<&SourceFilesApplyResult> {
        self.stage_results.iter().find_map(|result| match result {
            DiscoveryStageResult::SourceFiles { applied, .. } => Some(applied),
            DiscoveryStageResult::ProjectRootDiscovery { .. }
            | DiscoveryStageResult::PythonSourceModels { .. }
            | DiscoveryStageResult::DjangoEnvironments { .. }
            | DiscoveryStageResult::InstalledAppFiles { .. }
            | DiscoveryStageResult::TemplateDirectoryFiles { .. }
            | DiscoveryStageResult::Enrichment { .. } => None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryMilestoneResult {
    milestone: DiscoveryMilestone,
    status: DiscoveryMilestoneStatus,
}

impl DiscoveryMilestoneResult {
    #[must_use]
    pub fn milestone(&self) -> DiscoveryMilestone {
        self.milestone
    }

    #[must_use]
    pub fn status(&self) -> DiscoveryMilestoneStatus {
        self.status
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryStageResult {
    SourceFiles {
        applied: SourceFilesApplyResult,
        status: DiscoveryStageStatus,
    },
    ProjectRootDiscovery {
        applied: ProjectRootDiscoveryApplyResult,
        status: DiscoveryStageStatus,
    },
    PythonSourceModels {
        observed: PythonSourceIndexOutcome,
        status: DiscoveryStageStatus,
    },
    DjangoEnvironments {
        observed: DjangoEnvironmentCandidatesOutcome,
        status: DiscoveryStageStatus,
    },
    InstalledAppFiles {
        applied: Vec<SourceFilesApplyResult>,
        status: DiscoveryStageStatus,
    },
    TemplateDirectoryFiles {
        applied: Vec<SourceFilesApplyResult>,
        status: DiscoveryStageStatus,
    },
    Enrichment {
        applied: ProjectEnrichment,
        status: DiscoveryStageStatus,
    },
}

impl DiscoveryStageResult {
    #[must_use]
    pub fn stage(&self) -> DiscoveryStage {
        match self {
            Self::SourceFiles { .. } => DiscoveryStage::SourceFiles,
            Self::ProjectRootDiscovery { .. } => DiscoveryStage::ProjectRootDiscovery,
            Self::PythonSourceModels { .. } => DiscoveryStage::PythonSourceModels,
            Self::DjangoEnvironments { .. } => DiscoveryStage::DjangoEnvironments,
            Self::InstalledAppFiles { .. } => DiscoveryStage::InstalledAppFiles,
            Self::TemplateDirectoryFiles { .. } => DiscoveryStage::TemplateDirectoryFiles,
            Self::Enrichment { .. } => DiscoveryStage::Enrichment,
        }
    }

    #[must_use]
    pub fn status(&self) -> &DiscoveryStageStatus {
        match self {
            Self::SourceFiles { status, .. }
            | Self::ProjectRootDiscovery { status, .. }
            | Self::PythonSourceModels { status, .. }
            | Self::DjangoEnvironments { status, .. }
            | Self::InstalledAppFiles { status, .. }
            | Self::TemplateDirectoryFiles { status, .. }
            | Self::Enrichment { status, .. } => status,
        }
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

pub trait DiscoveryReadiness {
    fn stage_status(&self) -> DiscoveryStageStatus;
}

#[must_use]
pub fn stage_status_from_readiness(result: &impl DiscoveryReadiness) -> DiscoveryStageStatus {
    result.stage_status()
}

impl DiscoveryReadiness for ProjectEnrichment {
    fn stage_status(&self) -> DiscoveryStageStatus {
        match self {
            ProjectEnrichment::Absent | ProjectEnrichment::Disabled => {
                DiscoveryStageStatus::Skipped
            }
            ProjectEnrichment::Fresh(_) => DiscoveryStageStatus::Succeeded,
            ProjectEnrichment::Unresolved(ProjectEnrichmentIssue::InspectorFailed(_)) => {
                DiscoveryStageStatus::Failed
            }
            ProjectEnrichment::Unresolved(
                ProjectEnrichmentIssue::RuntimeUnavailable { .. }
                | ProjectEnrichmentIssue::FixtureDoesNotModelEnrichment,
            ) => DiscoveryStageStatus::Unavailable,
        }
    }
}

impl DiscoveryReadiness for SourceFilesApplyResult {
    fn stage_status(&self) -> DiscoveryStageStatus {
        match self {
            SourceFilesApplyResult::Applied(applied) => {
                stage_status_from_project_source_files_applied(applied)
            }
            SourceFilesApplyResult::Deferred { .. } => DiscoveryStageStatus::Deferred,
            SourceFilesApplyResult::Unavailable { .. } => DiscoveryStageStatus::Unavailable,
            SourceFilesApplyResult::Failed { .. } => DiscoveryStageStatus::Failed,
        }
    }
}

impl DiscoveryReadiness for ProjectRootDiscoveryApplyResult {
    fn stage_status(&self) -> DiscoveryStageStatus {
        match self {
            ProjectRootDiscoveryApplyResult::Applied {
                discovery: ProjectRootDiscovery::Ready(_),
                has_issues: false,
            } => DiscoveryStageStatus::Succeeded,
            ProjectRootDiscoveryApplyResult::Applied {
                discovery: ProjectRootDiscovery::Ready(_),
                has_issues: true,
            } => DiscoveryStageStatus::Degraded,
            ProjectRootDiscoveryApplyResult::Applied {
                discovery: ProjectRootDiscovery::Absent,
                ..
            }
            | ProjectRootDiscoveryApplyResult::Unavailable(ProjectRootDiscovery::Absent) => {
                DiscoveryStageStatus::Deferred
            }
            ProjectRootDiscoveryApplyResult::Applied {
                discovery: ProjectRootDiscovery::Unavailable { .. },
                ..
            }
            | ProjectRootDiscoveryApplyResult::Unavailable(
                ProjectRootDiscovery::Unavailable { .. } | ProjectRootDiscovery::Ready(_),
            ) => DiscoveryStageStatus::Unavailable,
        }
    }
}

impl DiscoveryReadiness for DjangoEnvironmentCandidatesOutcome {
    fn stage_status(&self) -> DiscoveryStageStatus {
        match self {
            DjangoEnvironmentCandidatesOutcome::Ready { issues, .. } if issues.is_empty() => {
                DiscoveryStageStatus::Succeeded
            }
            DjangoEnvironmentCandidatesOutcome::Ready { .. }
            | DjangoEnvironmentCandidatesOutcome::Ambiguous { .. } => {
                DiscoveryStageStatus::Degraded
            }
            DjangoEnvironmentCandidatesOutcome::Unavailable { .. } => {
                DiscoveryStageStatus::Unavailable
            }
            DjangoEnvironmentCandidatesOutcome::Deferred { .. } => DiscoveryStageStatus::Deferred,
        }
    }
}

impl DiscoveryReadiness for PythonSourceIndexOutcome {
    fn stage_status(&self) -> DiscoveryStageStatus {
        match self {
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
}

#[must_use]
pub fn stage_status_from_project_source_files_applied(
    applied: &SourceFilesApplied,
) -> DiscoveryStageStatus {
    stage_status_from_file_partition_readiness(applied.transition().readiness())
}

#[must_use]
pub fn stage_status_from_file_partition_readiness(
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
    let mut stage_results = Vec::with_capacity(DISCOVERY_STAGES.len());
    let mut milestone_results = Vec::with_capacity(MILESTONE_SPECS.len());

    if let Err(cancellation) = host.checkpoint() {
        return DiscoveryRunResult::aborted(
            stage_results,
            milestone_results,
            execution_outcome_from_cancellation(cancellation),
        );
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
            Ok(result) => stage_results.push(result),
            Err(outcome) => {
                return DiscoveryRunResult::aborted(stage_results, milestone_results, outcome)
            }
        }
        advance_milestones(&stage_results, &mut milestone_results, observer);
    }

    DiscoveryRunResult::completed(stage_results, milestone_results)
}

fn run_source_files_stage(
    request: &DjangoDiscoveryRequest,
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageResult, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::SourceFiles;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let plan = build_source_roots(request.workspace_roots.clone());
    let (root_issues, files_request) =
        first_party_discovery_files_request(first_party_source_files_load_request(plan));
    let files = load_files_for_roots(host, files_request, stage, observer)?;
    checkpoint(host, stage, observer)?;
    let patch = FirstPartySourceFilePatch::first_party(root_issues, files);
    let previous = host.current_source_files();
    let update = merge_first_party_source_file_patch(previous.as_ref(), patch);
    let applied = apply_source_files(host, update, stage, observer)?;
    let status = stage_status_from_readiness(&applied);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageResult::SourceFiles { applied, status })
}

fn run_project_root_discovery_stage(
    request: &DjangoDiscoveryRequest,
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageResult, DiscoveryExecutionOutcome> {
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
    let applied = match host.apply_project_root_discovery(update) {
        DiscoveryApplyOutcome::Applied(applied) => applied,
        DiscoveryApplyOutcome::Aborted(outcome) => {
            finish_aborted_stage(stage, &outcome, observer);
            return Err(outcome);
        }
    };
    let status = stage_status_from_readiness(&applied);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageResult::ProjectRootDiscovery { applied, status })
}

fn run_python_source_models_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageResult, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::PythonSourceModels;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let source_index = match host.observe_python_source_index() {
        DiscoveryObservationOutcome::Observed(outcome) => outcome,
        DiscoveryObservationOutcome::Cancelled(cancellation) => {
            let outcome = execution_outcome_from_cancellation(cancellation);
            finish_aborted_stage(stage, &outcome, observer);
            return Err(outcome);
        }
    };
    let status = stage_status_from_readiness(&source_index);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageResult::PythonSourceModels {
        observed: source_index,
        status,
    })
}

fn run_django_environments_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageResult, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::DjangoEnvironments;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let environment_candidates = match host.observe_django_environment_candidates() {
        DiscoveryObservationOutcome::Observed(candidates) => candidates,
        DiscoveryObservationOutcome::Cancelled(cancellation) => {
            let outcome = execution_outcome_from_cancellation(cancellation);
            finish_aborted_stage(stage, &outcome, observer);
            return Err(outcome);
        }
    };
    let status = stage_status_from_readiness(&environment_candidates);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageResult::DjangoEnvironments {
        observed: environment_candidates,
        status,
    })
}

fn run_installed_app_files_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageResult, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::InstalledAppFiles;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let discovery = match host.observe_installed_app_file_roots() {
        DiscoveryObservationOutcome::Observed(discovery) => discovery,
        DiscoveryObservationOutcome::Cancelled(cancellation) => {
            let outcome = execution_outcome_from_cancellation(cancellation);
            finish_aborted_stage(stage, &outcome, observer);
            return Err(outcome);
        }
    };
    let (applied, status) = match discovery {
        InstalledAppFileRootsDiscovery::Ready(roots) => {
            let result = load_files_for_roots(host, roots.files_request(), stage, observer)?;
            checkpoint(host, stage, observer)?;
            let previous = host.current_source_files();
            let update = roots.source_files_update(previous.as_ref(), result);
            let applied = apply_source_files(host, update, stage, observer)?;
            let status = stage_status_with_discovery_issues(&applied, roots.issues());
            (vec![applied], status)
        }
        InstalledAppFileRootsDiscovery::WaitingForDjangoEnvironments => {
            (Vec::new(), DiscoveryStageStatus::Deferred)
        }
        InstalledAppFileRootsDiscovery::DjangoEnvironmentsUnavailable => {
            (Vec::new(), DiscoveryStageStatus::Unavailable)
        }
    };
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageResult::InstalledAppFiles { applied, status })
}

fn run_template_directory_files_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageResult, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::TemplateDirectoryFiles;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let discovery = match host.observe_template_directory_file_roots() {
        DiscoveryObservationOutcome::Observed(discovery) => discovery,
        DiscoveryObservationOutcome::Cancelled(cancellation) => {
            let outcome = execution_outcome_from_cancellation(cancellation);
            finish_aborted_stage(stage, &outcome, observer);
            return Err(outcome);
        }
    };
    let (applied, status) = match discovery {
        TemplateDirectoryFileRootsDiscovery::Ready(roots) => {
            let result = load_files_for_roots(host, roots.files_request(), stage, observer)?;
            checkpoint(host, stage, observer)?;
            let previous = host.current_source_files();
            let update = TemplateDirectoryFileRoots::source_files_update(previous.as_ref(), result);
            let applied = apply_source_files(host, update, stage, observer)?;
            let status = stage_status_from_readiness(&applied);
            (vec![applied], status)
        }
        TemplateDirectoryFileRootsDiscovery::WaitingForDjangoEnvironments => {
            (Vec::new(), DiscoveryStageStatus::Deferred)
        }
        TemplateDirectoryFileRootsDiscovery::DjangoEnvironmentsUnavailable => {
            (Vec::new(), DiscoveryStageStatus::Unavailable)
        }
    };
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageResult::TemplateDirectoryFiles { applied, status })
}

fn run_enrichment_stage(
    host: &mut impl DiscoveryHost,
    observer: &mut impl DiscoveryObserver,
) -> Result<DiscoveryStageResult, DiscoveryExecutionOutcome> {
    let stage = DiscoveryStage::Enrichment;
    observer.stage_started(stage);
    checkpoint(host, stage, observer)?;
    let enrichment = match host.load_project_enrichment() {
        Ok(enrichment) => enrichment,
        Err(cancellation) => {
            let outcome = execution_outcome_from_cancellation(cancellation);
            finish_aborted_stage(stage, &outcome, observer);
            return Err(outcome);
        }
    };
    checkpoint(host, stage, observer)?;
    let applied = match host.apply_project_enrichment(enrichment) {
        DiscoveryApplyOutcome::Applied(applied) => applied,
        DiscoveryApplyOutcome::Aborted(outcome) => {
            finish_aborted_stage(stage, &outcome, observer);
            return Err(outcome);
        }
    };
    let status = stage_status_from_readiness(&applied);
    observer.stage_finished(stage, status.clone());
    Ok(DiscoveryStageResult::Enrichment { applied, status })
}

fn checkpoint(
    host: &mut impl DiscoveryHost,
    stage: DiscoveryStage,
    observer: &mut impl DiscoveryObserver,
) -> Result<(), DiscoveryExecutionOutcome> {
    match host.checkpoint() {
        Ok(()) => Ok(()),
        Err(cancellation) => {
            let outcome = execution_outcome_from_cancellation(cancellation);
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
    match host.load_files_for_roots(request) {
        Ok(result) => Ok(result),
        Err(cancellation) => {
            let outcome = execution_outcome_from_cancellation(cancellation);
            finish_aborted_stage(stage, &outcome, observer);
            Err(outcome)
        }
    }
}

fn apply_source_files(
    host: &mut impl DiscoveryHost,
    update: SourceFilesUpdate,
    stage: DiscoveryStage,
    observer: &mut impl DiscoveryObserver,
) -> Result<SourceFilesApplyResult, DiscoveryExecutionOutcome> {
    match host.apply_source_files(update) {
        DiscoveryApplyOutcome::Applied(applied) => Ok(applied),
        DiscoveryApplyOutcome::Aborted(outcome) => {
            finish_aborted_stage(stage, &outcome, observer);
            Err(outcome)
        }
    }
}

fn stage_status_with_discovery_issues(
    applied: &SourceFilesApplyResult,
    issues: &[SourceFilesIssue],
) -> DiscoveryStageStatus {
    let status = stage_status_from_readiness(applied);
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
    stage_results: &[DiscoveryStageResult],
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
                stage_results
                    .iter()
                    .find(|result| result.stage() == prerequisite.stage)
                    .map(DiscoveryStageResult::status)
                    .filter(|status| prerequisite.acceptable_statuses.contains(status))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(statuses) = statuses {
            let status = if statuses
                .iter()
                .all(|status| **status == DiscoveryStageStatus::Succeeded)
            {
                DiscoveryMilestoneStatus::Succeeded
            } else {
                DiscoveryMilestoneStatus::Degraded
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

fn execution_outcome_from_cancellation(
    cancellation: DiscoveryCancellation,
) -> DiscoveryExecutionOutcome {
    match cancellation {
        DiscoveryCancellation::Superseded => DiscoveryExecutionOutcome::Superseded,
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
    use crate::environments::DjangoEnvironmentCandidate;
    use crate::environments::EnvironmentCandidatesIssue;
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
        environment_degraded: bool,
        installed_app_deferred: bool,
        template_directory_deferred: bool,
        cancel_on_checkpoint: Option<usize>,
        cancel_on_file_load: bool,
        applied_updates: Vec<SourceFilesUpdate>,
    }

    impl DiscoveryHost for FakeHost {
        fn checkpoint(&mut self) -> Result<(), DiscoveryCancellation> {
            self.checkpoint_count += 1;
            if self.cancel_on_checkpoint == Some(self.checkpoint_count) {
                return Err(DiscoveryCancellation::Superseded);
            }
            Ok(())
        }

        fn load_files_for_roots(
            &mut self,
            request: FilesForRootsRequest,
        ) -> Result<FilesForRootsResult, DiscoveryCancellation> {
            self.load_count += 1;
            if self.cancel_on_file_load {
                return Err(DiscoveryCancellation::Superseded);
            }
            Ok(load_files_for_roots(request))
        }

        fn current_source_files(&mut self) -> Option<ReadySourceFiles> {
            None
        }

        fn apply_source_files(
            &mut self,
            update: SourceFilesUpdate,
        ) -> DiscoveryApplyOutcome<SourceFilesApplyResult> {
            self.apply_count += 1;
            if self.source_ready || update.applied_transition().partitions().is_empty() {
                self.applied_updates.push(update);
                let db = TestDb::default();
                let set = SourceFileSet::new(&db, SourceFileSetData::default());
                let files = ReadySourceFiles::materialized_for_test(
                    crate::source_files::SourceFileSetPartitions::default(),
                    set,
                );
                return DiscoveryApplyOutcome::Applied(SourceFilesApplyResult::Applied(
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
            DiscoveryApplyOutcome::Applied(SourceFilesApplyResult::Unavailable {
                transition,
                issue,
                previous: None,
            })
        }

        fn apply_project_root_discovery(
            &mut self,
            update: ProjectRootDiscoveryUpdate,
        ) -> DiscoveryApplyOutcome<ProjectRootDiscoveryApplyResult> {
            self.discovery_apply_count += 1;
            if update.roots().is_empty() {
                DiscoveryApplyOutcome::Applied(ProjectRootDiscoveryApplyResult::Unavailable(
                    ProjectRootDiscovery::Absent,
                ))
            } else {
                DiscoveryApplyOutcome::Applied(ProjectRootDiscoveryApplyResult::Applied {
                    discovery: ProjectRootDiscovery::Absent,
                    has_issues: false,
                })
            }
        }

        fn observe_python_source_index(
            &mut self,
        ) -> DiscoveryObservationOutcome<PythonSourceIndexOutcome> {
            self.python_observe_count += 1;
            if self.python_skipped {
                return DiscoveryObservationOutcome::Observed(PythonSourceIndexOutcome::Unindexed(
                    PythonSourceIndexIssue::NoPythonFiles,
                ));
            }
            DiscoveryObservationOutcome::Observed(PythonSourceIndexOutcome::Ready(
                PythonSourceIndex::default(),
            ))
        }

        fn observe_django_environment_candidates(
            &mut self,
        ) -> DiscoveryObservationOutcome<DjangoEnvironmentCandidatesOutcome> {
            self.environment_observe_count += 1;
            let issues = if self.environment_degraded {
                vec![EnvironmentCandidatesIssue::NoSettingsCandidates]
            } else {
                Vec::new()
            };
            DiscoveryObservationOutcome::Observed(DjangoEnvironmentCandidatesOutcome::Ready {
                candidates: Vec::<DjangoEnvironmentCandidate>::new(),
                issues,
            })
        }

        fn observe_installed_app_file_roots(
            &mut self,
        ) -> DiscoveryObservationOutcome<InstalledAppFileRootsDiscovery> {
            if self.installed_app_deferred {
                return DiscoveryObservationOutcome::Observed(
                    InstalledAppFileRootsDiscovery::WaitingForDjangoEnvironments,
                );
            }
            DiscoveryObservationOutcome::Observed(InstalledAppFileRootsDiscovery::Ready(
                crate::apps::InstalledAppFileRoots::new(Vec::new(), Vec::new()),
            ))
        }

        fn observe_template_directory_file_roots(
            &mut self,
        ) -> DiscoveryObservationOutcome<TemplateDirectoryFileRootsDiscovery> {
            if self.template_directory_deferred {
                return DiscoveryObservationOutcome::Observed(
                    TemplateDirectoryFileRootsDiscovery::WaitingForDjangoEnvironments,
                );
            }
            DiscoveryObservationOutcome::Observed(TemplateDirectoryFileRootsDiscovery::Ready(
                crate::templates::TemplateDirectoryFileRoots::new(Vec::new()),
            ))
        }

        fn load_project_enrichment(&mut self) -> Result<ProjectEnrichment, DiscoveryCancellation> {
            Ok(ProjectEnrichment::Disabled)
        }

        fn apply_project_enrichment(
            &mut self,
            enrichment: ProjectEnrichment,
        ) -> DiscoveryApplyOutcome<ProjectEnrichment> {
            DiscoveryApplyOutcome::Applied(enrichment)
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Vec<(DiscoveryStage, DiscoveryStageStatus)>,
        milestones: Vec<(DiscoveryMilestone, DiscoveryMilestoneStatus)>,
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
            status: DiscoveryMilestoneStatus,
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
            result.stage_results()[0].stage(),
            DiscoveryStage::SourceFiles
        );
        assert_eq!(
            result.stage_results()[1].stage(),
            DiscoveryStage::ProjectRootDiscovery,
        );
        assert_eq!(
            result.stage_results()[2].stage(),
            DiscoveryStage::PythonSourceModels,
        );
        assert_eq!(
            result.stage_results()[3].stage(),
            DiscoveryStage::DjangoEnvironments,
        );
        assert_eq!(
            observer.milestones,
            vec![
                (
                    DiscoveryMilestone::WorkspaceReady,
                    DiscoveryMilestoneStatus::Succeeded,
                ),
                (
                    DiscoveryMilestone::DjangoAppsReady,
                    DiscoveryMilestoneStatus::Succeeded,
                ),
            ]
        );
        assert_eq!(
            result.milestone_results()[0].milestone(),
            DiscoveryMilestone::WorkspaceReady,
        );
        assert_eq!(
            result.milestone_results()[0].status(),
            DiscoveryMilestoneStatus::Succeeded,
        );
    }

    #[test]
    fn django_discovery_workspace_ready_milestone_advances_degraded_for_accepted_statuses() {
        let mut host = FakeHost {
            source_ready: true,
            python_skipped: true,
            environment_degraded: true,
            ..FakeHost::default()
        };

        let (result, observer) = run_fake_discovery(&mut host);

        assert_eq!(
            observer.milestones,
            vec![
                (
                    DiscoveryMilestone::WorkspaceReady,
                    DiscoveryMilestoneStatus::Degraded,
                ),
                (
                    DiscoveryMilestone::DjangoAppsReady,
                    DiscoveryMilestoneStatus::Succeeded,
                ),
            ]
        );
        assert_eq!(
            result.milestone_results()[0].status(),
            DiscoveryMilestoneStatus::Degraded,
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
            result.stage_results().last().unwrap().stage(),
            DiscoveryStage::Enrichment,
        );
        assert_eq!(
            result.stage_results().last().unwrap().status(),
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
                DiscoveryMilestoneStatus::Succeeded,
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
