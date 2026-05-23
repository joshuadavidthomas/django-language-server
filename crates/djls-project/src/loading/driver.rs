use super::effects::LoadingApplyOutcome;
use super::effects::LoadingEffects;
use super::effects::LoadingExecutionOutcome;
use super::effects::LoadingObservationOutcome;
use super::effects::LoadingObserver;
use super::effects::LoadingRunControl;
use super::plan::node_status_from_readiness;
use super::plan::LoadingPlan;
use super::plan::MilestoneId;
use super::plan::MilestoneTerminalStatus;
use super::plan::NodeId;
use super::plan::NodeTerminalStatus;
use crate::enrichment::ProjectEnrichment;
use crate::root_discovery::ProjectRootDiscoveryApplyResult;
use crate::source_files::PartitionedSourceFileLoadOutcome;
use crate::source_files::SourceFilesApplyResult;
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::PythonSourceIndexOutcome;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadingRunResult {
    node_results: Vec<LoadingNodeResult>,
    milestone_results: Vec<LoadingMilestoneResult>,
    execution_outcome: Option<LoadingExecutionOutcome>,
}

impl LoadingRunResult {
    #[must_use]
    pub fn completed(
        node_results: Vec<LoadingNodeResult>,
        milestone_results: Vec<LoadingMilestoneResult>,
    ) -> Self {
        Self {
            node_results,
            milestone_results,
            execution_outcome: None,
        }
    }

    #[must_use]
    pub fn aborted(
        node_results: Vec<LoadingNodeResult>,
        milestone_results: Vec<LoadingMilestoneResult>,
        execution_outcome: LoadingExecutionOutcome,
    ) -> Self {
        Self {
            node_results,
            milestone_results,
            execution_outcome: Some(execution_outcome),
        }
    }

    #[must_use]
    pub fn node_results(&self) -> &[LoadingNodeResult] {
        &self.node_results
    }

    #[must_use]
    pub fn milestone_results(&self) -> &[LoadingMilestoneResult] {
        &self.milestone_results
    }

    #[must_use]
    pub fn execution_outcome(&self) -> Option<&LoadingExecutionOutcome> {
        self.execution_outcome.as_ref()
    }

    #[must_use]
    pub fn source_file_set_result(&self) -> Option<&SourceFilesApplyResult> {
        self.node_results.iter().find_map(|result| match result {
            LoadingNodeResult::SourceFileSet { applied, .. } => Some(applied),
            LoadingNodeResult::ProjectRootDiscoverySet { .. }
            | LoadingNodeResult::PythonSourceModels { .. }
            | LoadingNodeResult::EnvironmentDiscovery { .. }
            | LoadingNodeResult::InstalledAppFiles { .. }
            | LoadingNodeResult::TemplateDirectoryFiles { .. }
            | LoadingNodeResult::Enrichment { .. } => None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadingMilestoneResult {
    id: MilestoneId,
    status: MilestoneTerminalStatus,
}

impl LoadingMilestoneResult {
    #[must_use]
    pub fn id(&self) -> MilestoneId {
        self.id
    }

    #[must_use]
    pub fn status(&self) -> MilestoneTerminalStatus {
        self.status
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadingNodeResult {
    SourceFileSet {
        applied: SourceFilesApplyResult,
        status: NodeTerminalStatus,
    },
    ProjectRootDiscoverySet {
        applied: ProjectRootDiscoveryApplyResult,
        status: NodeTerminalStatus,
    },
    PythonSourceModels {
        observed: PythonSourceIndexOutcome,
        status: NodeTerminalStatus,
    },
    EnvironmentDiscovery {
        observed: DjangoEnvironmentCandidatesOutcome,
        status: NodeTerminalStatus,
    },
    InstalledAppFiles {
        applied: Vec<SourceFilesApplyResult>,
        status: NodeTerminalStatus,
    },
    TemplateDirectoryFiles {
        applied: Vec<SourceFilesApplyResult>,
        status: NodeTerminalStatus,
    },
    Enrichment {
        applied: ProjectEnrichment,
        status: NodeTerminalStatus,
    },
}

impl LoadingNodeResult {
    #[must_use]
    pub fn node(&self) -> NodeId {
        match self {
            Self::SourceFileSet { .. } => NodeId::SourceFileSet,
            Self::ProjectRootDiscoverySet { .. } => NodeId::ProjectRootDiscoverySet,
            Self::PythonSourceModels { .. } => NodeId::PythonSourceModels,
            Self::EnvironmentDiscovery { .. } => NodeId::EnvironmentDiscovery,
            Self::InstalledAppFiles { .. } => NodeId::InstalledAppFiles,
            Self::TemplateDirectoryFiles { .. } => NodeId::TemplateDirectoryFiles,
            Self::Enrichment { .. } => NodeId::Enrichment,
        }
    }

    #[must_use]
    pub fn status(&self) -> &NodeTerminalStatus {
        match self {
            Self::SourceFileSet { status, .. }
            | Self::ProjectRootDiscoverySet { status, .. }
            | Self::PythonSourceModels { status, .. }
            | Self::EnvironmentDiscovery { status, .. }
            | Self::InstalledAppFiles { status, .. }
            | Self::TemplateDirectoryFiles { status, .. }
            | Self::Enrichment { status, .. } => status,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub fn run_loading_plan(
    plan: LoadingPlan,
    effects: &mut impl LoadingEffects,
    observer: &mut impl LoadingObserver,
) -> LoadingRunResult {
    let mut node_results = Vec::with_capacity(plan.nodes().len());
    let mut milestone_results = Vec::with_capacity(plan.milestones().len());
    match effects.begin_loading_run() {
        LoadingRunControl::Continue => {}
        LoadingRunControl::Abort(outcome) => {
            return LoadingRunResult::aborted(node_results, milestone_results, outcome);
        }
    }

    for node in plan.nodes() {
        match node {
            NodeId::SourceFileSet => {
                observer.node_started(*node);
                let patch = effects.load_source_file_set();
                let applied = match effects.apply_source_file_patch(patch) {
                    LoadingApplyOutcome::Applied(applied) => applied,
                    LoadingApplyOutcome::Superseded => {
                        observer.node_finished(*node, NodeTerminalStatus::Superseded);
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                    LoadingApplyOutcome::RejectedApply => {
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::RejectedApply,
                        );
                    }
                };
                let status = node_status_from_readiness(&applied);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::SourceFileSet { applied, status });
            }
            NodeId::ProjectRootDiscoverySet => {
                observer.node_started(*node);
                let data = effects.load_project_discovery_set();
                let applied = match effects.apply_project_root_discovery(data) {
                    LoadingApplyOutcome::Applied(applied) => applied,
                    LoadingApplyOutcome::Superseded => {
                        observer.node_finished(*node, NodeTerminalStatus::Superseded);
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                    LoadingApplyOutcome::RejectedApply => {
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::RejectedApply,
                        );
                    }
                };
                let status = node_status_from_readiness(&applied);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::ProjectRootDiscoverySet { applied, status });
            }
            NodeId::PythonSourceModels => {
                observer.node_started(*node);
                let index_outcome = match effects.observe_python_source_index() {
                    LoadingObservationOutcome::Observed(index_outcome) => index_outcome,
                    LoadingObservationOutcome::Superseded => {
                        observer.node_finished(*node, NodeTerminalStatus::Superseded);
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                };
                let status = node_status_from_readiness(&index_outcome);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::PythonSourceModels {
                    observed: index_outcome,
                    status,
                });
            }
            NodeId::EnvironmentDiscovery => {
                observer.node_started(*node);
                let environment_outcome = match effects.observe_django_environment_candidates() {
                    LoadingObservationOutcome::Observed(environment_outcome) => environment_outcome,
                    LoadingObservationOutcome::Superseded => {
                        observer.node_finished(*node, NodeTerminalStatus::Superseded);
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                };
                let status = node_status_from_readiness(&environment_outcome);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::EnvironmentDiscovery {
                    observed: environment_outcome,
                    status,
                });
            }
            NodeId::InstalledAppFiles => {
                observer.node_started(*node);
                let (applied, status) = match apply_partitioned_load_outcome(
                    effects.load_installed_app_file_patches(),
                    effects,
                ) {
                    Ok(result) => result,
                    Err(outcome) => {
                        return LoadingRunResult::aborted(node_results, milestone_results, outcome)
                    }
                };
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::InstalledAppFiles { applied, status });
            }
            NodeId::TemplateDirectoryFiles => {
                observer.node_started(*node);
                let (applied, status) = match apply_partitioned_load_outcome(
                    effects.load_template_directory_file_patches(),
                    effects,
                ) {
                    Ok(result) => result,
                    Err(outcome) => {
                        return LoadingRunResult::aborted(node_results, milestone_results, outcome)
                    }
                };
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::TemplateDirectoryFiles { applied, status });
            }
            NodeId::Enrichment => {
                observer.node_started(*node);
                let enrichment = effects.load_project_enrichment();
                let applied = match effects.apply_project_enrichment(enrichment) {
                    LoadingApplyOutcome::Applied(applied) => applied,
                    LoadingApplyOutcome::Superseded => {
                        observer.node_finished(*node, NodeTerminalStatus::Superseded);
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                    LoadingApplyOutcome::RejectedApply => {
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::RejectedApply,
                        );
                    }
                };
                let status = node_status_from_readiness(&applied);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::Enrichment { applied, status });
            }
        }
        advance_milestones(plan, &node_results, &mut milestone_results, observer);
    }

    LoadingRunResult::completed(node_results, milestone_results)
}

fn apply_partitioned_load_outcome(
    outcome: PartitionedSourceFileLoadOutcome,
    effects: &mut impl LoadingEffects,
) -> Result<(Vec<SourceFilesApplyResult>, NodeTerminalStatus), LoadingExecutionOutcome> {
    match outcome {
        PartitionedSourceFileLoadOutcome::Ready(patches) => {
            let applied = apply_partitioned_patches(patches, effects)?;
            let status = aggregate_file_loading_status(&applied);
            Ok((applied, status))
        }
        PartitionedSourceFileLoadOutcome::Degraded { patches, .. } => {
            let applied = apply_partitioned_patches(patches, effects)?;
            let status = if applied.is_empty() {
                NodeTerminalStatus::Degraded
            } else {
                match aggregate_file_loading_status(&applied) {
                    NodeTerminalStatus::Succeeded | NodeTerminalStatus::Skipped => {
                        NodeTerminalStatus::Degraded
                    }
                    status => status,
                }
            };
            Ok((applied, status))
        }
        PartitionedSourceFileLoadOutcome::Deferred { .. } => {
            Ok((Vec::new(), NodeTerminalStatus::Deferred))
        }
        PartitionedSourceFileLoadOutcome::Unavailable { .. } => {
            Ok((Vec::new(), NodeTerminalStatus::Unavailable))
        }
    }
}

fn apply_partitioned_patches(
    patches: Vec<crate::PartitionedSourceFilePatch>,
    effects: &mut impl LoadingEffects,
) -> Result<Vec<SourceFilesApplyResult>, LoadingExecutionOutcome> {
    let mut applied = Vec::new();
    for patch in patches {
        match effects.apply_partitioned_source_file_patch(patch) {
            LoadingApplyOutcome::Applied(result) => applied.push(result),
            LoadingApplyOutcome::Superseded => return Err(LoadingExecutionOutcome::Superseded),
            LoadingApplyOutcome::RejectedApply => {
                return Err(LoadingExecutionOutcome::RejectedApply)
            }
        }
    }
    Ok(applied)
}

fn aggregate_file_loading_status(applied: &[SourceFilesApplyResult]) -> NodeTerminalStatus {
    if applied.is_empty() {
        return NodeTerminalStatus::Succeeded;
    }
    let statuses = applied
        .iter()
        .map(node_status_from_readiness)
        .collect::<Vec<_>>();
    if statuses.contains(&NodeTerminalStatus::Failed) {
        NodeTerminalStatus::Failed
    } else if statuses.contains(&NodeTerminalStatus::Unavailable) {
        NodeTerminalStatus::Unavailable
    } else if statuses.contains(&NodeTerminalStatus::Deferred) {
        NodeTerminalStatus::Deferred
    } else if statuses.contains(&NodeTerminalStatus::Degraded) {
        NodeTerminalStatus::Degraded
    } else if statuses
        .iter()
        .all(|status| *status == NodeTerminalStatus::Skipped)
    {
        NodeTerminalStatus::Skipped
    } else {
        NodeTerminalStatus::Succeeded
    }
}

fn advance_milestones(
    plan: LoadingPlan,
    node_results: &[LoadingNodeResult],
    milestone_results: &mut Vec<LoadingMilestoneResult>,
    observer: &mut impl LoadingObserver,
) {
    for milestone in plan.milestones() {
        if milestone_results
            .iter()
            .any(|result| result.id == milestone.id)
        {
            continue;
        }
        let statuses = milestone
            .prerequisites
            .iter()
            .map(|prerequisite| {
                node_results
                    .iter()
                    .find(|result| result.node() == prerequisite.node)
                    .map(LoadingNodeResult::status)
                    .filter(|status| prerequisite.acceptable_statuses.contains(status))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(statuses) = statuses {
            let status = if statuses
                .iter()
                .all(|status| **status == NodeTerminalStatus::Succeeded)
            {
                MilestoneTerminalStatus::Succeeded
            } else {
                MilestoneTerminalStatus::Degraded
            };
            milestone_results.push(LoadingMilestoneResult {
                id: milestone.id,
                status,
            });
            observer.milestone_reached(milestone.id, status);
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::FileSetSummary;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_workspace::load_files_for_roots;

    use super::*;
    use crate::build_source_roots;
    use crate::enrichment::ProjectEnrichment;
    use crate::environments::DjangoEnvironmentCandidate;
    use crate::environments::EnvironmentCandidatesIssue;
    use crate::first_party_discovery_files_request;
    use crate::first_party_source_files_load_request;
    use crate::merge_first_party_source_file_patch;
    use crate::python::source::PythonSourceIndex;
    use crate::python::source::PythonSourceIndexIssue;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::root_discovery::ProjectRootDiscoveryApplyResult;
    use crate::root_discovery::ProjectRootDiscoveryLoadRequest;
    use crate::root_discovery::ProjectRootDiscoveryUpdate;
    use crate::source_files::FirstPartySourceFilePatch;
    use crate::source_files::SourceFilesApplied;
    use crate::source_files::SourceFilesApplyResult;
    use crate::DjangoEnvironmentCandidatesOutcome;
    use crate::PythonSourceIndexOutcome;

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
    struct FakeEffects {
        reset_count: usize,
        load_count: usize,
        apply_count: usize,
        discovery_load_count: usize,
        discovery_apply_count: usize,
        python_observe_count: usize,
        environment_observe_count: usize,
        roots: Vec<Utf8PathBuf>,
        source_ready: bool,
        python_skipped: bool,
        environment_degraded: bool,
        installed_app_deferred: bool,
        template_directory_deferred: bool,
    }

    impl LoadingEffects for FakeEffects {
        fn begin_loading_run(&mut self) -> LoadingRunControl {
            self.reset_count += 1;
            LoadingRunControl::Continue
        }

        fn load_source_file_set(&mut self) -> FirstPartySourceFilePatch {
            self.load_count += 1;
            let plan = build_source_roots(self.roots.clone());
            let (root_issues, request) =
                first_party_discovery_files_request(first_party_source_files_load_request(plan));
            FirstPartySourceFilePatch::first_party(root_issues, load_files_for_roots(request))
        }

        fn apply_source_file_patch(
            &mut self,
            patch: FirstPartySourceFilePatch,
        ) -> LoadingApplyOutcome<SourceFilesApplyResult> {
            self.apply_count += 1;
            if self.source_ready {
                let db = TestDb::default();
                let set = SourceFileSet::new(&db, SourceFileSetData::default());
                let files = crate::ReadySourceFiles::new(
                    crate::source_files::SourceFileSetPartitions::default(),
                    set,
                );
                return LoadingApplyOutcome::Applied(SourceFilesApplyResult::Applied(
                    SourceFilesApplied::for_test(
                        files,
                        crate::source_files::SourceFilePartitionReadiness::Ready {
                            summary: FileSetSummary::new(0),
                        },
                    ),
                ));
            }
            let update = merge_first_party_source_file_patch(None, patch);
            let transition = update.applied_transition().clone();
            let issue = update
                .issues()
                .first()
                .cloned()
                .unwrap_or(crate::SourceFilesIssue::NotLoaded);
            LoadingApplyOutcome::Applied(SourceFilesApplyResult::Unavailable {
                transition,
                issue,
                previous: None,
            })
        }

        fn load_project_discovery_set(&mut self) -> ProjectRootDiscoveryUpdate {
            self.discovery_load_count += 1;
            crate::load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
                self.roots.clone(),
                djls_conf::Settings::default(),
            ))
        }

        fn apply_project_root_discovery(
            &mut self,
            data: ProjectRootDiscoveryUpdate,
        ) -> LoadingApplyOutcome<ProjectRootDiscoveryApplyResult> {
            self.discovery_apply_count += 1;
            if data.roots().is_empty() {
                LoadingApplyOutcome::Applied(ProjectRootDiscoveryApplyResult::Unavailable(
                    ProjectRootDiscovery::Absent,
                ))
            } else {
                LoadingApplyOutcome::Applied(ProjectRootDiscoveryApplyResult::Applied {
                    discovery: ProjectRootDiscovery::Absent,
                    has_issues: false,
                })
            }
        }

        fn observe_python_source_index(
            &mut self,
        ) -> LoadingObservationOutcome<PythonSourceIndexOutcome> {
            self.python_observe_count += 1;
            if self.python_skipped {
                return LoadingObservationOutcome::Observed(PythonSourceIndexOutcome::Unindexed(
                    PythonSourceIndexIssue::NoPythonFiles,
                ));
            }
            LoadingObservationOutcome::Observed(PythonSourceIndexOutcome::Ready(
                PythonSourceIndex::default(),
            ))
        }

        fn observe_django_environment_candidates(
            &mut self,
        ) -> LoadingObservationOutcome<DjangoEnvironmentCandidatesOutcome> {
            self.environment_observe_count += 1;
            let issues = if self.environment_degraded {
                vec![EnvironmentCandidatesIssue::NoSettingsCandidates]
            } else {
                Vec::new()
            };
            LoadingObservationOutcome::Observed(DjangoEnvironmentCandidatesOutcome::Ready {
                candidates: Vec::<DjangoEnvironmentCandidate>::new(),
                issues,
            })
        }

        fn load_installed_app_file_patches(&mut self) -> PartitionedSourceFileLoadOutcome {
            if self.installed_app_deferred {
                return PartitionedSourceFileLoadOutcome::Deferred {
                    issue: crate::SourceFilesIssue::InstalledAppGap {
                        entry: "UNKNOWN".to_string(),
                    },
                };
            }
            PartitionedSourceFileLoadOutcome::Ready(Vec::new())
        }

        fn load_template_directory_file_patches(&mut self) -> PartitionedSourceFileLoadOutcome {
            if self.template_directory_deferred {
                return PartitionedSourceFileLoadOutcome::Deferred {
                    issue: crate::SourceFilesIssue::TemplateDirectoryGap,
                };
            }
            PartitionedSourceFileLoadOutcome::Ready(Vec::new())
        }

        fn apply_partitioned_source_file_patch(
            &mut self,
            _patch: crate::PartitionedSourceFilePatch,
        ) -> LoadingApplyOutcome<SourceFilesApplyResult> {
            unreachable!("fake effects return no partitioned patches")
        }

        fn load_project_enrichment(&mut self) -> ProjectEnrichment {
            ProjectEnrichment::Disabled
        }

        fn apply_project_enrichment(
            &mut self,
            enrichment: ProjectEnrichment,
        ) -> LoadingApplyOutcome<ProjectEnrichment> {
            LoadingApplyOutcome::Applied(enrichment)
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Vec<(NodeId, NodeTerminalStatus)>,
        milestones: Vec<(MilestoneId, MilestoneTerminalStatus)>,
        started: Vec<NodeId>,
    }

    impl LoadingObserver for RecordingObserver {
        fn node_started(&mut self, node: NodeId) {
            self.started.push(node);
        }

        fn node_finished(&mut self, node: NodeId, status: NodeTerminalStatus) {
            self.events.push((node, status));
        }

        fn milestone_reached(&mut self, milestone: MilestoneId, status: MilestoneTerminalStatus) {
            self.milestones.push((milestone, status));
        }
    }

    fn run_fake_plan(effects: &mut FakeEffects) -> (LoadingRunResult, RecordingObserver) {
        let mut observer = RecordingObserver::default();
        let result = run_loading_plan(LoadingPlan::phase3(), effects, &mut observer);
        (result, observer)
    }

    #[test]
    fn loading_plan_workspace_ready_milestone_advances_after_environment_discovery() {
        let mut effects = FakeEffects {
            source_ready: true,
            roots: vec![Utf8PathBuf::from("/missing")],
            ..FakeEffects::default()
        };
        let (result, observer) = run_fake_plan(&mut effects);

        assert_eq!(effects.reset_count, 1);
        assert_eq!(effects.load_count, 1);
        assert_eq!(effects.apply_count, 1);
        assert_eq!(effects.discovery_load_count, 1);
        assert_eq!(effects.discovery_apply_count, 1);
        assert_eq!(effects.python_observe_count, 1);
        assert_eq!(effects.environment_observe_count, 1);
        assert_eq!(
            observer.started,
            vec![
                NodeId::SourceFileSet,
                NodeId::ProjectRootDiscoverySet,
                NodeId::PythonSourceModels,
                NodeId::EnvironmentDiscovery,
                NodeId::InstalledAppFiles,
                NodeId::TemplateDirectoryFiles,
                NodeId::Enrichment,
            ]
        );
        assert_eq!(
            observer.events,
            vec![
                (NodeId::SourceFileSet, NodeTerminalStatus::Succeeded),
                (
                    NodeId::ProjectRootDiscoverySet,
                    NodeTerminalStatus::Deferred
                ),
                (NodeId::PythonSourceModels, NodeTerminalStatus::Succeeded),
                (NodeId::EnvironmentDiscovery, NodeTerminalStatus::Succeeded),
                (NodeId::InstalledAppFiles, NodeTerminalStatus::Succeeded),
                (
                    NodeId::TemplateDirectoryFiles,
                    NodeTerminalStatus::Succeeded
                ),
                (NodeId::Enrichment, NodeTerminalStatus::Skipped),
            ]
        );
        assert_eq!(result.node_results()[0].node(), NodeId::SourceFileSet);
        assert_eq!(
            result.node_results()[1].node(),
            NodeId::ProjectRootDiscoverySet
        );
        assert_eq!(result.node_results()[2].node(), NodeId::PythonSourceModels);
        assert_eq!(
            result.node_results()[3].node(),
            NodeId::EnvironmentDiscovery
        );
        assert_eq!(
            result.node_results()[0].status(),
            &NodeTerminalStatus::Succeeded
        );
        assert_eq!(
            result.node_results()[1].status(),
            &NodeTerminalStatus::Deferred
        );
        assert_eq!(
            result.node_results()[2].status(),
            &NodeTerminalStatus::Succeeded
        );
        assert_eq!(
            result.node_results()[3].status(),
            &NodeTerminalStatus::Succeeded
        );
        assert_eq!(
            observer.milestones,
            vec![
                (
                    MilestoneId::WorkspaceReady,
                    MilestoneTerminalStatus::Succeeded,
                ),
                (
                    MilestoneId::DjangoAppsReady,
                    MilestoneTerminalStatus::Succeeded,
                ),
            ]
        );
        assert_eq!(
            result.milestone_results()[0].id(),
            MilestoneId::WorkspaceReady
        );
        assert_eq!(
            result.milestone_results()[0].status(),
            MilestoneTerminalStatus::Succeeded
        );
    }

    #[test]
    fn loading_plan_workspace_ready_milestone_advances_degraded_for_accepted_statuses() {
        let mut effects = FakeEffects {
            source_ready: true,
            python_skipped: true,
            environment_degraded: true,
            roots: vec![Utf8PathBuf::from("/missing")],
            ..FakeEffects::default()
        };

        let (result, observer) = run_fake_plan(&mut effects);

        assert_eq!(
            observer.milestones,
            vec![
                (
                    MilestoneId::WorkspaceReady,
                    MilestoneTerminalStatus::Degraded,
                ),
                (
                    MilestoneId::DjangoAppsReady,
                    MilestoneTerminalStatus::Succeeded,
                ),
            ]
        );
        assert_eq!(
            result.milestone_results()[0].status(),
            MilestoneTerminalStatus::Degraded
        );
    }

    #[test]
    fn loading_enrichment_node_runs_after_static_file_nodes_and_skips_by_default() {
        let mut effects = FakeEffects {
            source_ready: true,
            roots: vec![Utf8PathBuf::from("/missing")],
            ..FakeEffects::default()
        };

        let (result, observer) = run_fake_plan(&mut effects);

        assert_eq!(
            result.node_results().last().unwrap().node(),
            NodeId::Enrichment
        );
        assert_eq!(
            result.node_results().last().unwrap().status(),
            &NodeTerminalStatus::Skipped
        );
        assert!(observer
            .events
            .contains(&(NodeId::Enrichment, NodeTerminalStatus::Skipped,)));
    }

    #[test]
    fn loading_plan_django_apps_ready_milestone_degrades_for_deferred_app_files() {
        let mut effects = FakeEffects {
            source_ready: true,
            installed_app_deferred: true,
            roots: vec![Utf8PathBuf::from("/missing")],
            ..FakeEffects::default()
        };

        let (_result, observer) = run_fake_plan(&mut effects);

        assert!(observer
            .events
            .contains(&(NodeId::InstalledAppFiles, NodeTerminalStatus::Deferred,)));
        assert!(observer.events.contains(&(
            NodeId::TemplateDirectoryFiles,
            NodeTerminalStatus::Succeeded,
        )));
        assert!(observer.milestones.contains(&(
            MilestoneId::DjangoAppsReady,
            MilestoneTerminalStatus::Degraded,
        )));
    }

    #[test]
    fn loading_plan_workspace_ready_milestone_does_not_advance_for_unavailable_source_files() {
        let mut effects = FakeEffects {
            roots: vec![Utf8PathBuf::from("/missing")],
            ..FakeEffects::default()
        };

        let (result, observer) = run_fake_plan(&mut effects);

        assert_eq!(
            observer.milestones,
            vec![(
                MilestoneId::DjangoAppsReady,
                MilestoneTerminalStatus::Succeeded,
            )]
        );
        assert_eq!(result.milestone_results().len(), 1);
    }
}
