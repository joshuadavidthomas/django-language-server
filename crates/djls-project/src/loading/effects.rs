use super::plan::NodeId;
use super::plan::NodeTerminalStatus;
use crate::FirstPartySourceFilePatch;
use crate::ProjectDiscoveryApplyResult;
use crate::ProjectDiscoverySetData;
use crate::ProjectSourceFilesApplyResult;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadingRunControl {
    Continue,
    Abort(LoadingExecutionOutcome),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadingApplyOutcome<T> {
    Applied(T),
    Superseded,
    RejectedApply,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadingExecutionOutcome {
    Superseded,
    RejectedApply,
}

pub trait LoadingEffects {
    fn begin_loading_run(&mut self) -> LoadingRunControl;
    fn load_source_file_set(&mut self) -> FirstPartySourceFilePatch;
    fn apply_source_file_patch(
        &mut self,
        patch: FirstPartySourceFilePatch,
    ) -> LoadingApplyOutcome<ProjectSourceFilesApplyResult>;
    fn load_project_discovery_set(&mut self) -> ProjectDiscoverySetData;
    fn apply_project_discovery_data(
        &mut self,
        data: ProjectDiscoverySetData,
    ) -> LoadingApplyOutcome<ProjectDiscoveryApplyResult>;
}

pub trait LoadingObserver {
    fn node_started(&mut self, node: NodeId);
    fn node_finished(&mut self, node: NodeId, status: NodeTerminalStatus);
}

#[derive(Default)]
pub struct NoopLoadingObserver;

impl LoadingObserver for NoopLoadingObserver {
    fn node_started(&mut self, _node: NodeId) {}

    fn node_finished(&mut self, _node: NodeId, _status: NodeTerminalStatus) {}
}
