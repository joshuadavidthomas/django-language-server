use super::plan::NodeId;
use super::plan::NodeTerminalStatus;
use crate::FirstPartySourceFilePatch;
use crate::ProjectSourceFilesApplyResult;

pub trait LoadingEffects {
    fn begin_loading_run(&mut self);
    fn load_source_file_set(&mut self) -> FirstPartySourceFilePatch;
    fn apply_source_file_patch(
        &mut self,
        patch: FirstPartySourceFilePatch,
    ) -> ProjectSourceFilesApplyResult;
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
