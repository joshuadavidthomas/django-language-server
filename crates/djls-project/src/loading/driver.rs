use super::effects::LoadingApplyOutcome;
use super::effects::LoadingEffects;
use super::effects::LoadingExecutionOutcome;
use super::effects::LoadingObserver;
use super::effects::LoadingRunControl;
use super::plan::node_status_from_readiness;
use super::plan::LoadingPlan;
use super::plan::NodeId;
use super::plan::NodeTerminalStatus;
use crate::ProjectSourceFilesApplyResult;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadingRunResult {
    node_results: Vec<LoadingNodeResult>,
    execution_outcome: Option<LoadingExecutionOutcome>,
}

impl LoadingRunResult {
    #[must_use]
    pub fn completed(node_results: Vec<LoadingNodeResult>) -> Self {
        Self {
            node_results,
            execution_outcome: None,
        }
    }

    #[must_use]
    pub fn aborted(
        node_results: Vec<LoadingNodeResult>,
        execution_outcome: LoadingExecutionOutcome,
    ) -> Self {
        Self {
            node_results,
            execution_outcome: Some(execution_outcome),
        }
    }

    #[must_use]
    pub fn node_results(&self) -> &[LoadingNodeResult] {
        &self.node_results
    }

    #[must_use]
    pub fn execution_outcome(&self) -> Option<&LoadingExecutionOutcome> {
        self.execution_outcome.as_ref()
    }

    #[must_use]
    pub fn source_file_set_result(&self) -> Option<&ProjectSourceFilesApplyResult> {
        self.node_results.iter().find_map(|result| match result {
            LoadingNodeResult::SourceFileSet { applied, .. } => Some(applied),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadingNodeResult {
    SourceFileSet {
        applied: ProjectSourceFilesApplyResult,
        status: NodeTerminalStatus,
    },
}

impl LoadingNodeResult {
    #[must_use]
    pub fn node(&self) -> NodeId {
        match self {
            Self::SourceFileSet { .. } => NodeId::SourceFileSet,
        }
    }

    #[must_use]
    pub fn status(&self) -> &NodeTerminalStatus {
        match self {
            Self::SourceFileSet { status, .. } => status,
        }
    }
}

pub fn run_loading_plan(
    plan: LoadingPlan,
    effects: &mut impl LoadingEffects,
    observer: &mut impl LoadingObserver,
) -> LoadingRunResult {
    let mut node_results = Vec::with_capacity(plan.nodes().len());
    match effects.begin_loading_run() {
        LoadingRunControl::Continue => {}
        LoadingRunControl::Abort(outcome) => {
            return LoadingRunResult::aborted(node_results, outcome)
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
                        return LoadingRunResult::aborted(
                            node_results,
                            LoadingExecutionOutcome::Superseded,
                        );
                    }
                    LoadingApplyOutcome::RejectedApply => {
                        return LoadingRunResult::aborted(
                            node_results,
                            LoadingExecutionOutcome::RejectedApply,
                        );
                    }
                };
                let status = node_status_from_readiness(&applied);
                observer.node_finished(*node, status.clone());
                node_results.push(LoadingNodeResult::SourceFileSet { applied, status });
            }
        }
    }

    LoadingRunResult::completed(node_results)
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_workspace::load_files_for_roots;

    use super::*;
    use crate::build_source_roots;
    use crate::first_party_discovery_files_request;
    use crate::first_party_source_files_load_request;
    use crate::merge_first_party_source_file_patch;
    use crate::FirstPartySourceFilePatch;
    use crate::ProjectSourceFilesApplyResult;

    #[derive(Default)]
    struct FakeEffects {
        reset_count: usize,
        load_count: usize,
        apply_count: usize,
        roots: Vec<Utf8PathBuf>,
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
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Vec<(NodeId, NodeTerminalStatus)>,
        started: Vec<NodeId>,
    }

    impl LoadingObserver for RecordingObserver {
        fn node_started(&mut self, node: NodeId) {
            self.started.push(node);
        }

        fn node_finished(&mut self, node: NodeId, status: NodeTerminalStatus) {
            self.events.push((node, status));
        }
    }

    #[test]
    fn runner_executes_source_file_set_and_emits_observer_events() {
        let mut effects = FakeEffects {
            roots: vec![Utf8PathBuf::from("/missing")],
            ..FakeEffects::default()
        };
        let mut observer = RecordingObserver::default();

        let result = run_loading_plan(LoadingPlan::phase3(), &mut effects, &mut observer);

        assert_eq!(effects.reset_count, 1);
        assert_eq!(effects.load_count, 1);
        assert_eq!(effects.apply_count, 1);
        assert_eq!(observer.started, vec![NodeId::SourceFileSet]);
        assert_eq!(
            observer.events,
            vec![(NodeId::SourceFileSet, NodeTerminalStatus::Unavailable)]
        );
        assert_eq!(result.node_results()[0].node(), NodeId::SourceFileSet);
        assert_eq!(
            result.node_results()[0].status(),
            &NodeTerminalStatus::Unavailable
        );
    }
}
