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
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::ProjectDiscoveryApplyResult;
use crate::ProjectSourceFilesApplyResult;
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
    pub fn source_file_set_result(&self) -> Option<&ProjectSourceFilesApplyResult> {
        self.node_results.iter().find_map(|result| match result {
            LoadingNodeResult::SourceFileSet { applied, .. } => Some(applied),
            LoadingNodeResult::ProjectDiscoverySet { .. }
            | LoadingNodeResult::PythonSourceModels { .. }
            | LoadingNodeResult::EnvironmentDiscovery { .. } => None,
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
        applied: ProjectSourceFilesApplyResult,
        status: NodeTerminalStatus,
    },
    ProjectDiscoverySet {
        applied: ProjectDiscoveryApplyResult,
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
}

impl LoadingNodeResult {
    #[must_use]
    pub fn node(&self) -> NodeId {
        match self {
            Self::SourceFileSet { .. } => NodeId::SourceFileSet,
            Self::ProjectDiscoverySet { .. } => NodeId::ProjectDiscoverySet,
            Self::PythonSourceModels { .. } => NodeId::PythonSourceModels,
            Self::EnvironmentDiscovery { .. } => NodeId::EnvironmentDiscovery,
        }
    }

    #[must_use]
    pub fn status(&self) -> &NodeTerminalStatus {
        match self {
            Self::SourceFileSet { status, .. }
            | Self::ProjectDiscoverySet { status, .. }
            | Self::PythonSourceModels { status, .. }
            | Self::EnvironmentDiscovery { status, .. } => status,
        }
    }
}

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
            NodeId::ProjectDiscoverySet => {
                observer.node_started(*node);
                let data = effects.load_project_discovery_set();
                let applied = match effects.apply_project_discovery_data(data) {
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
                node_results.push(LoadingNodeResult::ProjectDiscoverySet { applied, status });
            }
            NodeId::PythonSourceModels => {
                observer.node_started(*node);
                let observed = match effects.observe_python_source_index() {
                    LoadingObservationOutcome::Observed(observed) => observed,
                    LoadingObservationOutcome::Superseded => {
                        observer.node_finished(*node, NodeTerminalStatus::Superseded);
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                };
                let status = node_status_from_readiness(&observed);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::PythonSourceModels { observed, status });
            }
            NodeId::EnvironmentDiscovery => {
                observer.node_started(*node);
                let observed = match effects.observe_django_environment_candidates() {
                    LoadingObservationOutcome::Observed(observed) => observed,
                    LoadingObservationOutcome::Superseded => {
                        observer.node_finished(*node, NodeTerminalStatus::Superseded);
                        return LoadingRunResult::aborted(
                            node_results,
                            milestone_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                };
                let status = node_status_from_readiness(&observed);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::EnvironmentDiscovery { observed, status });
            }
        }
        advance_milestones(plan, &node_results, &mut milestone_results, observer);
    }

    LoadingRunResult::completed(node_results, milestone_results)
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
    use crate::first_party_discovery_files_request;
    use crate::first_party_source_files_load_request;
    use crate::merge_first_party_source_file_patch;
    use crate::DjangoEnvironmentCandidate;
    use crate::DjangoEnvironmentCandidatesOutcome;
    use crate::EnvironmentCandidatesIssue;
    use crate::FirstPartySourceFilePatch;
    use crate::ProjectDiscovery;
    use crate::ProjectDiscoveryApplyResult;
    use crate::ProjectDiscoveryLoadRequest;
    use crate::ProjectDiscoverySetData;
    use crate::ProjectSourceFilesApplied;
    use crate::ProjectSourceFilesApplyResult;
    use crate::PythonSourceIndex;
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
        ) -> LoadingApplyOutcome<ProjectSourceFilesApplyResult> {
            self.apply_count += 1;
            if self.source_ready {
                let db = TestDb::default();
                let set = SourceFileSet::new(&db, SourceFileSetData::default());
                let files = crate::ReadyProjectSourceFiles::merged_for_test(set);
                return LoadingApplyOutcome::Applied(ProjectSourceFilesApplyResult::Applied(
                    ProjectSourceFilesApplied::for_test(
                        files,
                        crate::ProjectFilePartitionReadiness::Ready {
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
                .unwrap_or(crate::ProjectSourceFilesIssue::NotLoaded);
            LoadingApplyOutcome::Applied(ProjectSourceFilesApplyResult::Unavailable {
                transition,
                issue,
                previous: None,
            })
        }

        fn load_project_discovery_set(&mut self) -> ProjectDiscoverySetData {
            self.discovery_load_count += 1;
            crate::build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
                self.roots.clone(),
                djls_conf::Settings::default(),
            ))
        }

        fn apply_project_discovery_data(
            &mut self,
            data: ProjectDiscoverySetData,
        ) -> LoadingApplyOutcome<ProjectDiscoveryApplyResult> {
            self.discovery_apply_count += 1;
            if data.roots().is_empty() {
                LoadingApplyOutcome::Applied(ProjectDiscoveryApplyResult::Unavailable(
                    ProjectDiscovery::Absent,
                ))
            } else {
                LoadingApplyOutcome::Applied(ProjectDiscoveryApplyResult::Applied {
                    discovery: ProjectDiscovery::Absent,
                    has_issues: false,
                })
            }
        }

        fn observe_python_source_index(
            &mut self,
        ) -> LoadingObservationOutcome<PythonSourceIndexOutcome> {
            self.python_observe_count += 1;
            if self.python_skipped {
                return LoadingObservationOutcome::Observed(PythonSourceIndexOutcome::Skipped {
                    issue: crate::PythonSourceIndexIssue::NoPythonFiles,
                });
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
                NodeId::ProjectDiscoverySet,
                NodeId::PythonSourceModels,
                NodeId::EnvironmentDiscovery,
            ]
        );
        assert_eq!(
            observer.events,
            vec![
                (NodeId::SourceFileSet, NodeTerminalStatus::Succeeded),
                (NodeId::ProjectDiscoverySet, NodeTerminalStatus::Deferred),
                (NodeId::PythonSourceModels, NodeTerminalStatus::Succeeded),
                (NodeId::EnvironmentDiscovery, NodeTerminalStatus::Succeeded),
            ]
        );
        assert_eq!(result.node_results()[0].node(), NodeId::SourceFileSet);
        assert_eq!(result.node_results()[1].node(), NodeId::ProjectDiscoverySet);
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
            vec![(
                MilestoneId::WorkspaceReady,
                MilestoneTerminalStatus::Succeeded,
            )]
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
            vec![(
                MilestoneId::WorkspaceReady,
                MilestoneTerminalStatus::Degraded
            )]
        );
        assert_eq!(
            result.milestone_results()[0].status(),
            MilestoneTerminalStatus::Degraded
        );
    }

    #[test]
    fn loading_plan_workspace_ready_milestone_does_not_advance_for_unavailable_source_files() {
        let mut effects = FakeEffects {
            roots: vec![Utf8PathBuf::from("/missing")],
            ..FakeEffects::default()
        };

        let (result, observer) = run_fake_plan(&mut effects);

        assert!(observer.milestones.is_empty());
        assert!(result.milestone_results().is_empty());
    }
}
