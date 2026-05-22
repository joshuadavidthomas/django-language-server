use crate::DjangoEnvironmentCandidatesOutcome;
use crate::ProjectDiscovery;
use crate::ProjectDiscoveryApplyResult;
use crate::ProjectFilePartitionReadiness;
use crate::ProjectSourceFilesApplied;
use crate::ProjectSourceFilesApplyResult;
use crate::PythonSourceIndexOutcome;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NodeId {
    SourceFileSet,
    ProjectDiscoverySet,
    PythonSourceModels,
    EnvironmentDiscovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessSourceKind {
    SourceFilePartition,
    ProjectDiscovery,
    PythonSourceIndex,
    EnvironmentCandidates,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeSpec {
    pub id: NodeId,
    pub prerequisites: &'static [NodeId],
    pub readiness_source: ReadinessSourceKind,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MilestoneId {
    WorkspaceReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MilestoneSpec {
    pub id: MilestoneId,
    pub prerequisites: &'static [MilestonePrerequisite],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MilestonePrerequisite {
    pub node: NodeId,
    pub acceptable_statuses: &'static [NodeTerminalStatus],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MilestoneTerminalStatus {
    Succeeded,
    Degraded,
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
    NodeSpec {
        id: NodeId::PythonSourceModels,
        prerequisites: &[NodeId::SourceFileSet, NodeId::ProjectDiscoverySet],
        readiness_source: ReadinessSourceKind::PythonSourceIndex,
    },
    NodeSpec {
        id: NodeId::EnvironmentDiscovery,
        prerequisites: &[
            NodeId::SourceFileSet,
            NodeId::ProjectDiscoverySet,
            NodeId::PythonSourceModels,
        ],
        readiness_source: ReadinessSourceKind::EnvironmentCandidates,
    },
];

pub const MILESTONE_SPECS: &[MilestoneSpec] = &[MilestoneSpec {
    id: MilestoneId::WorkspaceReady,
    prerequisites: &[
        MilestonePrerequisite {
            node: NodeId::SourceFileSet,
            acceptable_statuses: &[NodeTerminalStatus::Succeeded],
        },
        MilestonePrerequisite {
            node: NodeId::PythonSourceModels,
            acceptable_statuses: &[NodeTerminalStatus::Succeeded, NodeTerminalStatus::Skipped],
        },
        MilestonePrerequisite {
            node: NodeId::EnvironmentDiscovery,
            acceptable_statuses: &[NodeTerminalStatus::Succeeded, NodeTerminalStatus::Degraded],
        },
    ],
}];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadingPlan {
    nodes: &'static [NodeId],
    milestones: &'static [MilestoneSpec],
}

impl LoadingPlan {
    #[must_use]
    pub fn phase3() -> Self {
        Self {
            nodes: &[
                NodeId::SourceFileSet,
                NodeId::ProjectDiscoverySet,
                NodeId::PythonSourceModels,
                NodeId::EnvironmentDiscovery,
            ],
            milestones: MILESTONE_SPECS,
        }
    }

    #[must_use]
    pub fn nodes(&self) -> &'static [NodeId] {
        self.nodes
    }

    #[must_use]
    pub fn milestones(&self) -> &'static [MilestoneSpec] {
        self.milestones
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
    Superseded,
}

pub trait LoadingReadiness {
    fn terminal_status(&self) -> NodeTerminalStatus;
}

#[must_use]
pub fn node_status_from_readiness(result: &impl LoadingReadiness) -> NodeTerminalStatus {
    result.terminal_status()
}

impl LoadingReadiness for ProjectSourceFilesApplyResult {
    fn terminal_status(&self) -> NodeTerminalStatus {
        match self {
            ProjectSourceFilesApplyResult::Applied(applied) => {
                node_status_from_project_source_files_applied(applied)
            }
            ProjectSourceFilesApplyResult::Deferred { .. } => NodeTerminalStatus::Deferred,
            ProjectSourceFilesApplyResult::Unavailable { .. } => NodeTerminalStatus::Unavailable,
            ProjectSourceFilesApplyResult::Failed { .. } => NodeTerminalStatus::Failed,
        }
    }
}

#[must_use]
pub fn node_status_from_discovery_readiness(
    result: &ProjectDiscoveryApplyResult,
) -> NodeTerminalStatus {
    node_status_from_readiness(result)
}

impl LoadingReadiness for ProjectDiscoveryApplyResult {
    fn terminal_status(&self) -> NodeTerminalStatus {
        match self {
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
}

impl LoadingReadiness for DjangoEnvironmentCandidatesOutcome {
    fn terminal_status(&self) -> NodeTerminalStatus {
        match self {
            DjangoEnvironmentCandidatesOutcome::Ready { issues, .. } if issues.is_empty() => {
                NodeTerminalStatus::Succeeded
            }
            DjangoEnvironmentCandidatesOutcome::Ready { .. }
            | DjangoEnvironmentCandidatesOutcome::Ambiguous { .. } => NodeTerminalStatus::Degraded,
            DjangoEnvironmentCandidatesOutcome::Unavailable { .. } => {
                NodeTerminalStatus::Unavailable
            }
            DjangoEnvironmentCandidatesOutcome::Deferred { .. } => NodeTerminalStatus::Deferred,
        }
    }
}

impl LoadingReadiness for PythonSourceIndexOutcome {
    fn terminal_status(&self) -> NodeTerminalStatus {
        match self {
            PythonSourceIndexOutcome::Ready(_) => NodeTerminalStatus::Succeeded,
            PythonSourceIndexOutcome::Skipped { .. } => NodeTerminalStatus::Skipped,
            PythonSourceIndexOutcome::Unavailable { .. } => NodeTerminalStatus::Unavailable,
            PythonSourceIndexOutcome::Deferred { .. } => NodeTerminalStatus::Deferred,
        }
    }
}

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
    use crate::DjangoEnvironmentCandidate;
    use crate::EnvironmentCandidatesIssue;
    use crate::ProjectDiscoveryIssue;
    use crate::ProjectDiscoveryIssues;
    use crate::ProjectDiscoverySet;
    use crate::ProjectSourceFilesIssue;
    use crate::PythonSourceIndex;
    use crate::PythonSourceIndexIssue;
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
                NodeSpec {
                    id: NodeId::PythonSourceModels,
                    prerequisites: &[NodeId::SourceFileSet, NodeId::ProjectDiscoverySet],
                    readiness_source: ReadinessSourceKind::PythonSourceIndex,
                },
                NodeSpec {
                    id: NodeId::EnvironmentDiscovery,
                    prerequisites: &[
                        NodeId::SourceFileSet,
                        NodeId::ProjectDiscoverySet,
                        NodeId::PythonSourceModels,
                    ],
                    readiness_source: ReadinessSourceKind::EnvironmentCandidates,
                },
            ]
        );
    }

    #[test]
    fn loading_plan_workspace_ready_milestone_policy_uses_static_readiness_nodes() {
        assert_eq!(
            LoadingPlan::phase3().milestones(),
            &[MilestoneSpec {
                id: MilestoneId::WorkspaceReady,
                prerequisites: &[
                    MilestonePrerequisite {
                        node: NodeId::SourceFileSet,
                        acceptable_statuses: &[NodeTerminalStatus::Succeeded],
                    },
                    MilestonePrerequisite {
                        node: NodeId::PythonSourceModels,
                        acceptable_statuses: &[
                            NodeTerminalStatus::Succeeded,
                            NodeTerminalStatus::Skipped,
                        ],
                    },
                    MilestonePrerequisite {
                        node: NodeId::EnvironmentDiscovery,
                        acceptable_statuses: &[
                            NodeTerminalStatus::Succeeded,
                            NodeTerminalStatus::Degraded,
                        ],
                    },
                ],
            }]
        );
    }

    #[test]
    fn phase3_plan_contains_source_file_set_then_project_discovery_set() {
        assert_eq!(
            LoadingPlan::phase3().nodes(),
            &[
                NodeId::SourceFileSet,
                NodeId::ProjectDiscoverySet,
                NodeId::PythonSourceModels,
                NodeId::EnvironmentDiscovery,
            ]
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
    fn loading_environment_discovery_projection_covers_readiness_classes() {
        let cases = [
            (
                DjangoEnvironmentCandidatesOutcome::Ready {
                    candidates: vec![DjangoEnvironmentCandidate::for_test()],
                    issues: Vec::new(),
                },
                NodeTerminalStatus::Succeeded,
            ),
            (
                DjangoEnvironmentCandidatesOutcome::Ready {
                    candidates: vec![DjangoEnvironmentCandidate::for_test()],
                    issues: vec![EnvironmentCandidatesIssue::NoSettingsCandidates],
                },
                NodeTerminalStatus::Degraded,
            ),
            (
                DjangoEnvironmentCandidatesOutcome::Deferred {
                    issue: EnvironmentCandidatesIssue::NoSettingsCandidates,
                },
                NodeTerminalStatus::Deferred,
            ),
            (
                DjangoEnvironmentCandidatesOutcome::Unavailable {
                    issue: EnvironmentCandidatesIssue::NoSettingsCandidates,
                },
                NodeTerminalStatus::Unavailable,
            ),
        ];

        for (result, expected) in cases {
            assert_eq!(node_status_from_readiness(&result), expected);
        }
    }

    #[test]
    fn loading_python_source_models_projection_covers_readiness_classes() {
        let cases = [
            (
                PythonSourceIndexOutcome::Ready(PythonSourceIndex::default()),
                NodeTerminalStatus::Succeeded,
            ),
            (
                PythonSourceIndexOutcome::Skipped {
                    issue: PythonSourceIndexIssue::NoPythonFiles,
                },
                NodeTerminalStatus::Skipped,
            ),
            (
                PythonSourceIndexOutcome::Deferred {
                    issue: PythonSourceIndexIssue::NoPythonFiles,
                },
                NodeTerminalStatus::Deferred,
            ),
            (
                PythonSourceIndexOutcome::Unavailable {
                    issue: PythonSourceIndexIssue::LayoutUnavailable,
                },
                NodeTerminalStatus::Unavailable,
            ),
        ];

        for (result, expected) in cases {
            assert_eq!(node_status_from_readiness(&result), expected);
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
