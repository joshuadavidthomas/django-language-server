use crate::ProjectFilePartitionReadiness;
use crate::ProjectSourceFilesApplied;
use crate::ProjectSourceFilesApplyResult;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NodeId {
    SourceFileSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessSourceKind {
    SourceFilePartition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeSpec {
    pub id: NodeId,
    pub prerequisites: &'static [NodeId],
    pub readiness_source: ReadinessSourceKind,
}

pub const NODE_SPECS: &[NodeSpec] = &[NodeSpec {
    id: NodeId::SourceFileSet,
    prerequisites: &[],
    readiness_source: ReadinessSourceKind::SourceFilePartition,
}];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadingPlan {
    nodes: &'static [NodeId],
}

impl LoadingPlan {
    #[must_use]
    pub fn phase3() -> Self {
        Self {
            nodes: &[NodeId::SourceFileSet],
        }
    }

    #[must_use]
    pub fn nodes(&self) -> &'static [NodeId] {
        self.nodes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeTerminalStatus {
    Succeeded,
    Degraded,
    Deferred,
    Skipped,
    Unavailable,
    Failed,
}

#[must_use]
pub fn node_status_from_readiness(result: &ProjectSourceFilesApplyResult) -> NodeTerminalStatus {
    match result {
        ProjectSourceFilesApplyResult::Applied(applied) => {
            node_status_from_project_source_files_applied(applied)
        }
        ProjectSourceFilesApplyResult::Deferred { .. } => NodeTerminalStatus::Deferred,
        ProjectSourceFilesApplyResult::Unavailable { .. } => NodeTerminalStatus::Unavailable,
        ProjectSourceFilesApplyResult::Failed { .. } => NodeTerminalStatus::Failed,
    }
}

#[must_use]
pub fn node_status_from_project_source_files_applied(
    applied: &ProjectSourceFilesApplied,
) -> NodeTerminalStatus {
    node_status_from_file_partition_readiness(applied.transition().readiness())
}

#[must_use]
pub fn node_status_from_file_partition_readiness(
    readiness: &ProjectFilePartitionReadiness,
) -> NodeTerminalStatus {
    match readiness {
        ProjectFilePartitionReadiness::Loading => NodeTerminalStatus::Deferred,
        ProjectFilePartitionReadiness::Ready { .. } => NodeTerminalStatus::Succeeded,
        ProjectFilePartitionReadiness::Deferred { .. } => NodeTerminalStatus::Deferred,
        ProjectFilePartitionReadiness::Skipped { .. } => NodeTerminalStatus::Skipped,
        ProjectFilePartitionReadiness::Unavailable { .. } => NodeTerminalStatus::Unavailable,
        ProjectFilePartitionReadiness::Stale { .. } => NodeTerminalStatus::Deferred,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_specs_contains_only_source_file_set_in_phase3() {
        assert_eq!(
            NODE_SPECS,
            &[NodeSpec {
                id: NodeId::SourceFileSet,
                prerequisites: &[],
                readiness_source: ReadinessSourceKind::SourceFilePartition,
            }]
        );
    }

    #[test]
    fn phase3_plan_contains_source_file_set_only() {
        assert_eq!(LoadingPlan::phase3().nodes(), &[NodeId::SourceFileSet]);
    }
}
