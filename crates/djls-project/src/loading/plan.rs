use crate::ProjectDiscovery;
use crate::ProjectDiscoveryApplyResult;
use crate::ProjectFilePartitionReadiness;
use crate::ProjectSourceFilesApplied;
use crate::ProjectSourceFilesApplyResult;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NodeId {
    SourceFileSet,
    ProjectDiscoverySet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessSourceKind {
    SourceFilePartition,
    ProjectDiscovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeSpec {
    pub id: NodeId,
    pub prerequisites: &'static [NodeId],
    pub readiness_source: ReadinessSourceKind,
}

pub const NODE_SPECS: &[NodeSpec] = &[
    NodeSpec {
        id: NodeId::SourceFileSet,
        prerequisites: &[],
        readiness_source: ReadinessSourceKind::SourceFilePartition,
    },
    NodeSpec {
        id: NodeId::ProjectDiscoverySet,
        prerequisites: &[NodeId::SourceFileSet],
        readiness_source: ReadinessSourceKind::ProjectDiscovery,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadingPlan {
    nodes: &'static [NodeId],
}

impl LoadingPlan {
    #[must_use]
    pub fn phase3() -> Self {
        Self {
            nodes: &[NodeId::SourceFileSet, NodeId::ProjectDiscoverySet],
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
pub fn node_status_from_discovery_readiness(
    result: &ProjectDiscoveryApplyResult,
) -> NodeTerminalStatus {
    match result {
        ProjectDiscoveryApplyResult::Applied {
            discovery: ProjectDiscovery::Ready(_),
            has_issues: false,
        } => NodeTerminalStatus::Succeeded,
        ProjectDiscoveryApplyResult::Applied {
            discovery: ProjectDiscovery::Ready(_),
            has_issues: true,
        } => NodeTerminalStatus::Degraded,
        ProjectDiscoveryApplyResult::Applied {
            discovery: ProjectDiscovery::Absent,
            ..
        }
        | ProjectDiscoveryApplyResult::Unavailable(ProjectDiscovery::Absent) => {
            NodeTerminalStatus::Deferred
        }
        ProjectDiscoveryApplyResult::Applied {
            discovery: ProjectDiscovery::Unavailable { .. },
            ..
        }
        | ProjectDiscoveryApplyResult::Unavailable(ProjectDiscovery::Unavailable { .. })
        | ProjectDiscoveryApplyResult::Unavailable(ProjectDiscovery::Ready(_)) => {
            NodeTerminalStatus::Unavailable
        }
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
    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;

    use super::super::files::ProjectFileSetPartitions;
    use super::*;
    use crate::ProjectDiscoveryIssue;
    use crate::ProjectDiscoveryIssues;
    use crate::ProjectDiscoverySet;
    use crate::ProjectSourceFilesIssue;
    use crate::ReadyProjectSourceFiles;
    use crate::RootDiscoveryInput;

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

    #[test]
    fn node_specs_contains_source_file_and_discovery_nodes_in_phase3() {
        assert_eq!(
            NODE_SPECS,
            &[
                NodeSpec {
                    id: NodeId::SourceFileSet,
                    prerequisites: &[],
                    readiness_source: ReadinessSourceKind::SourceFilePartition,
                },
                NodeSpec {
                    id: NodeId::ProjectDiscoverySet,
                    prerequisites: &[NodeId::SourceFileSet],
                    readiness_source: ReadinessSourceKind::ProjectDiscovery,
                },
            ]
        );
    }

    #[test]
    fn phase3_plan_contains_source_file_set_then_project_discovery_set() {
        assert_eq!(
            LoadingPlan::phase3().nodes(),
            &[NodeId::SourceFileSet, NodeId::ProjectDiscoverySet]
        );
    }

    #[test]
    fn file_partition_readiness_projection_covers_table_classes() {
        let issue = ProjectSourceFilesIssue::NotLoaded;
        let cases = [
            (
                ProjectFilePartitionReadiness::Loading,
                NodeTerminalStatus::Deferred,
            ),
            (
                ProjectFilePartitionReadiness::Ready {
                    summary: djls_source::FileSetSummary::new(1),
                },
                NodeTerminalStatus::Succeeded,
            ),
            (
                ProjectFilePartitionReadiness::Deferred {
                    issue: issue.clone(),
                    previous: None,
                },
                NodeTerminalStatus::Deferred,
            ),
            (
                ProjectFilePartitionReadiness::Skipped {
                    issue: issue.clone(),
                    previous: None,
                },
                NodeTerminalStatus::Skipped,
            ),
            (
                ProjectFilePartitionReadiness::Unavailable {
                    issue,
                    previous: None,
                },
                NodeTerminalStatus::Unavailable,
            ),
            (
                ProjectFilePartitionReadiness::Stale { previous: None },
                NodeTerminalStatus::Deferred,
            ),
        ];

        for (readiness, expected) in cases {
            assert_eq!(
                node_status_from_file_partition_readiness(&readiness),
                expected
            );
        }
    }

    #[test]
    fn discovery_readiness_projection_distinguishes_clean_degraded_and_unavailable() {
        let db = TestDb::default();
        let clean_root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/clean"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            crate::ProjectEnvVars::default(),
            Vec::new(),
        );
        let issue_root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/issue"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            crate::ProjectEnvVars::default(),
            vec![ProjectDiscoveryIssue::NoWorkspaceRoots],
        );
        let unavailable_issues =
            ProjectDiscoveryIssues::new(vec![ProjectDiscoveryIssue::NoWorkspaceRoots])
                .expect("test issue list is non-empty");

        let cases = [
            (
                ProjectDiscoveryApplyResult::Applied {
                    discovery: ProjectDiscovery::Ready(
                        ProjectDiscoverySet::new(vec![clean_root])
                            .expect("test root set is non-empty"),
                    ),
                    has_issues: false,
                },
                NodeTerminalStatus::Succeeded,
            ),
            (
                ProjectDiscoveryApplyResult::Applied {
                    discovery: ProjectDiscovery::Ready(
                        ProjectDiscoverySet::new(vec![issue_root])
                            .expect("test root set is non-empty"),
                    ),
                    has_issues: true,
                },
                NodeTerminalStatus::Degraded,
            ),
            (
                ProjectDiscoveryApplyResult::Applied {
                    discovery: ProjectDiscovery::Absent,
                    has_issues: false,
                },
                NodeTerminalStatus::Deferred,
            ),
            (
                ProjectDiscoveryApplyResult::Unavailable(ProjectDiscovery::Unavailable {
                    issues: unavailable_issues,
                }),
                NodeTerminalStatus::Unavailable,
            ),
        ];

        for (result, expected) in cases {
            assert_eq!(node_status_from_discovery_readiness(&result), expected);
        }
    }

    #[test]
    fn applied_source_files_project_through_partition_transition() {
        let db = TestDb::default();
        let source_file_set = SourceFileSet::new(&db, SourceFileSetData::default());
        let files = ReadyProjectSourceFiles::materialized_for_test(
            ProjectFileSetPartitions::empty(),
            source_file_set,
        );
        let applied = ProjectSourceFilesApplied::for_test(
            files,
            ProjectFilePartitionReadiness::Ready {
                summary: djls_source::FileSetSummary::new(0),
            },
        );
        let result = ProjectSourceFilesApplyResult::Applied(applied);

        assert_eq!(
            node_status_from_readiness(&result),
            NodeTerminalStatus::Succeeded
        );
    }
}
