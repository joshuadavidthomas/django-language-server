use std::collections::BTreeMap;
use std::collections::BTreeSet;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::DiscoveredSourceFile;
use djls_source::FileRootKind;
use djls_source::FileSetSummary;
use djls_source::SourceFileSet;
use djls_source::SourceRoot;
use djls_source::SourceRootEntry;
use djls_source::SourceRootId;
use djls_workspace::FileLoadPredicate;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;
use djls_workspace::WalkOptions;
use djls_workspace::WorkspaceRootIssue;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFileInventory {
    Ready(ReadySourceFiles),
    Unavailable { issue: SourceFilesIssue },
}

impl SourceFileInventory {
    #[must_use]
    pub fn ready(&self) -> Option<ReadySourceFiles> {
        match self {
            Self::Ready(files) => Some(files.clone()),
            Self::Unavailable { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadySourceFiles {
    pub(crate) partitions: SourceFileSetPartitions,
    merged: SourceFileSet,
}

impl ReadySourceFiles {
    #[must_use]
    pub(crate) fn new(partitions: SourceFileSetPartitions, merged: SourceFileSet) -> Self {
        Self { partitions, merged }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn materialized_for_test(
        partitions: SourceFileSetPartitions,
        merged: SourceFileSet,
    ) -> Self {
        Self::new(partitions, merged)
    }

    #[must_use]
    pub fn merged(&self) -> SourceFileSet {
        self.merged
    }

    #[must_use]
    pub(crate) fn discovered_files(&self) -> DiscoveredSourceFiles {
        self.partitions.merged_discovered_files()
    }

    #[must_use]
    pub fn summary(&self, db: &dyn djls_source::Db) -> FileSetSummary {
        *self.merged.data(db).summary()
    }

    #[must_use]
    pub fn root_readiness_for_partition(
        &self,
        path: &Utf8Path,
        matches_partition: impl Fn(&FileSetPartitionId) -> bool,
    ) -> Option<SourceFilePartitionReadiness> {
        self.partitions
            .root_readiness_for_partition(path, matches_partition)
    }

    #[must_use]
    pub fn has_partition_readiness(&self) -> bool {
        self.partitions.has_partitions()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFilesIssue {
    NotLoaded,
    MissingRoot {
        root: SourceRootId,
        path: Utf8PathBuf,
    },
    DuplicateRoot {
        root: SourceRootId,
        duplicate_path: Utf8PathBuf,
    },
    WalkFailed {
        root: SourceRootId,
        path: Utf8PathBuf,
        error_kind: std::io::ErrorKind,
    },
    PartitionConflict {
        path: Utf8PathBuf,
        winner: FileSetPartitionId,
        shadowed: FileSetPartitionId,
    },
    FixtureUnavailable {
        surface: SourceFilesFixtureSurface,
    },
    MaterializationFailed {
        path: Utf8PathBuf,
        error_kind: std::io::ErrorKind,
    },
    InstalledAppGap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFilesFixtureSurface {
    SourceFiles,
    Partitions,
    Materialization,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SourceRootsPlan {
    roots: Vec<SourceRoot>,
    issues: Vec<SourceFilesIssue>,
}

impl SourceRootsPlan {
    #[must_use]
    pub(crate) fn roots(&self) -> &[SourceRoot] {
        &self.roots
    }

    #[cfg(test)]
    #[must_use]
    pub fn issues(&self) -> &[SourceFilesIssue] {
        &self.issues
    }
}

#[must_use]
pub(crate) fn build_source_roots(
    raw_roots: impl IntoIterator<Item = Utf8PathBuf>,
) -> SourceRootsPlan {
    build_source_roots_with_kind(raw_roots, FileRootKind::Project)
}

#[must_use]
pub(crate) fn build_source_roots_with_kind(
    raw_roots: impl IntoIterator<Item = Utf8PathBuf>,
    kind: FileRootKind,
) -> SourceRootsPlan {
    let mut roots = Vec::new();
    let mut issues = Vec::new();
    let mut seen = BTreeSet::new();

    for raw_path in raw_roots {
        let path = dunce::canonicalize(&raw_path)
            .ok()
            .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
            .unwrap_or_else(|| raw_path.clone());
        let id = SourceRootId::new(path.clone());
        if !seen.insert(id.clone()) {
            issues.push(SourceFilesIssue::DuplicateRoot {
                root: id,
                duplicate_path: raw_path,
            });
            continue;
        }

        roots.push(SourceRoot::new(id, path, kind));
    }

    SourceRootsPlan { roots, issues }
}

pub(crate) struct SourceFilesLoadRequest {
    roots: Vec<SourceRoot>,
    root_issues: Vec<SourceFilesIssue>,
    predicate: FileLoadPredicate,
    options: WalkOptions,
}

impl SourceFilesLoadRequest {
    fn new(
        roots: Vec<SourceRoot>,
        root_issues: Vec<SourceFilesIssue>,
        predicate: FileLoadPredicate,
        options: WalkOptions,
    ) -> Self {
        Self {
            roots,
            root_issues,
            predicate,
            options,
        }
    }
}

#[must_use]
fn first_party_file_predicate() -> FileLoadPredicate {
    Box::new(|path| {
        matches!(
            path.extension(),
            Some("html" | "htm" | "txt" | "py" | "json" | "toml" | "yaml" | "yml")
        )
    })
}

#[must_use]
fn first_party_walk_options() -> WalkOptions {
    WalkOptions {
        hidden: false,
        globs: vec![
            "!**/.venv/**".to_string(),
            "!**/venv/**".to_string(),
            "!**/node_modules/**".to_string(),
            "!**/__pycache__/**".to_string(),
            "!**/target/**".to_string(),
        ],
        no_ignore: false,
        follow_links: false,
        max_depth: None,
    }
}

#[must_use]
pub(crate) fn first_party_source_files_load_request(
    plan: SourceRootsPlan,
) -> SourceFilesLoadRequest {
    SourceFilesLoadRequest::new(
        plan.roots,
        plan.issues,
        first_party_file_predicate(),
        first_party_walk_options(),
    )
}

#[must_use]
pub(crate) fn first_party_discovery_files_request(
    request: SourceFilesLoadRequest,
) -> (Vec<SourceFilesIssue>, FilesForRootsRequest) {
    let files_request =
        FilesForRootsRequest::new(request.roots, request.predicate, request.options);
    (request.root_issues, files_request)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileSetPartitionGroup {
    FirstParty,
    ConfiguredTemplateDirectory,
    InstalledApp,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FileSetPartitionId {
    FirstParty,
    ConfiguredTemplateDirectory(SourceRootId),
    InstalledApp(SourceRootId),
}

impl FileSetPartitionId {
    fn group(&self) -> FileSetPartitionGroup {
        match self {
            Self::FirstParty => FileSetPartitionGroup::FirstParty,
            Self::ConfiguredTemplateDirectory(_) => {
                FileSetPartitionGroup::ConfiguredTemplateDirectory
            }
            Self::InstalledApp(_) => FileSetPartitionGroup::InstalledApp,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSetPartition {
    id: FileSetPartitionId,
    precedence: u16,
}

impl FileSetPartition {
    #[must_use]
    pub fn first_party() -> Self {
        Self {
            id: FileSetPartitionId::FirstParty,
            precedence: 100,
        }
    }

    #[must_use]
    pub fn configured_template_directory(root: SourceRootId) -> Self {
        Self {
            id: FileSetPartitionId::ConfiguredTemplateDirectory(root),
            precedence: 75,
        }
    }

    #[must_use]
    pub fn installed_app(root: SourceRootId) -> Self {
        Self {
            id: FileSetPartitionId::InstalledApp(root),
            precedence: 50,
        }
    }

    #[must_use]
    pub fn id(&self) -> &FileSetPartitionId {
        &self.id
    }

    #[must_use]
    pub fn precedence(&self) -> u16 {
        self.precedence
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFilePartitionReadiness {
    Loading,
    Ready {
        summary: FileSetSummary,
    },
    Deferred {
        issue: SourceFilesIssue,
        previous: Option<FileSetSummary>,
    },
    Skipped {
        issue: SourceFilesIssue,
        previous: Option<FileSetSummary>,
    },
    Unavailable {
        issue: SourceFilesIssue,
        previous: Option<FileSetSummary>,
    },
    Stale {
        previous: Option<FileSetSummary>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceFileSetPartitionSnapshot {
    partition: FileSetPartition,
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
    readiness: SourceFilePartitionReadiness,
}

impl SourceFileSetPartitionSnapshot {
    fn new(
        partition: FileSetPartition,
        roots: Vec<SourceRoot>,
        files: Vec<DiscoveredSourceFile>,
        readiness: SourceFilePartitionReadiness,
    ) -> Self {
        let summary = FileSetSummary::new(files.len());
        Self {
            partition,
            roots,
            files,
            summary,
            readiness,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SourceFileSetPartitions {
    partitions: Vec<SourceFileSetPartitionSnapshot>,
}

impl SourceFileSetPartitions {
    #[cfg(test)]
    fn with_first_party(snapshot: SourceFileSetPartitionSnapshot) -> Self {
        Self {
            partitions: vec![snapshot],
        }
    }

    fn replace_partition(&self, snapshot: SourceFileSetPartitionSnapshot) -> Self {
        self.replace_partition_group(snapshot.partition.id().group(), vec![snapshot])
    }

    fn replace_partition_group(
        &self,
        group: FileSetPartitionGroup,
        snapshots: Vec<SourceFileSetPartitionSnapshot>,
    ) -> Self {
        let mut partitions = self
            .partitions
            .iter()
            .filter(|partition| partition.partition.id().group() != group)
            .cloned()
            .collect::<Vec<_>>();
        partitions.extend(snapshots);
        partitions.sort_by(|left, right| {
            right
                .partition
                .precedence()
                .cmp(&left.partition.precedence())
                .then_with(|| {
                    format!("{:?}", left.partition.id()).cmp(&format!("{:?}", right.partition.id()))
                })
        });
        Self { partitions }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn first_party_readiness(&self) -> Option<&SourceFilePartitionReadiness> {
        self.partitions
            .iter()
            .find(|partition| partition.partition.id() == &FileSetPartitionId::FirstParty)
            .map(|partition| &partition.readiness)
    }

    #[must_use]
    pub(crate) fn root_readiness_for_partition(
        &self,
        path: &Utf8Path,
        matches_partition: impl Fn(&FileSetPartitionId) -> bool,
    ) -> Option<SourceFilePartitionReadiness> {
        self.partitions
            .iter()
            .find(|partition| {
                matches_partition(partition.partition.id())
                    && partition
                        .roots
                        .iter()
                        .any(|root| path.starts_with(root.path()))
            })
            .map(|partition| partition.readiness.clone())
    }

    #[must_use]
    pub(crate) fn has_partitions(&self) -> bool {
        !self.partitions.is_empty()
    }

    pub(crate) fn merged_discovered_files(&self) -> DiscoveredSourceFiles {
        let roots = self
            .partitions
            .iter()
            .flat_map(|partition| partition.roots.iter().cloned())
            .map(SourceRootEntry::new)
            .collect::<Vec<_>>();
        let mut selected_files = BTreeMap::<Utf8PathBuf, DiscoveredSourceFile>::new();
        for partition in &self.partitions {
            for file in &partition.files {
                selected_files
                    .entry(file.path().to_owned())
                    .or_insert_with(|| file.clone());
            }
        }
        let files = selected_files.into_values().collect::<Vec<_>>();
        DiscoveredSourceFiles::new(roots, files)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FirstPartySourceFilePatch {
    partition: FileSetPartition,
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
    issues: Vec<SourceFilesIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PartitionedSourceFilePatch {
    partition: FileSetPartition,
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
    issues: Vec<SourceFilesIssue>,
}

impl PartitionedSourceFilePatch {
    #[must_use]
    pub(crate) fn installed_app(result: FilesForRootsResult) -> Vec<Self> {
        partitioned_patches(result, FileSetPartition::installed_app)
    }

    #[must_use]
    pub(crate) fn configured_template_directory(result: FilesForRootsResult) -> Vec<Self> {
        partitioned_patches(result, FileSetPartition::configured_template_directory)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PartitionedSourceFilePatchSet {
    group: FileSetPartitionGroup,
    patches: Vec<PartitionedSourceFilePatch>,
    issues: Vec<SourceFilesIssue>,
}

impl PartitionedSourceFilePatchSet {
    #[must_use]
    pub(crate) fn installed_apps(
        result: FilesForRootsResult,
        issues: Vec<SourceFilesIssue>,
    ) -> Self {
        Self {
            group: FileSetPartitionGroup::InstalledApp,
            patches: PartitionedSourceFilePatch::installed_app(result),
            issues,
        }
    }

    #[must_use]
    pub(crate) fn configured_template_directories(result: FilesForRootsResult) -> Self {
        Self {
            group: FileSetPartitionGroup::ConfiguredTemplateDirectory,
            patches: PartitionedSourceFilePatch::configured_template_directory(result),
            issues: Vec::new(),
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn partitioned_patches(
    result: FilesForRootsResult,
    partition_for_root: impl Fn(SourceRootId) -> FileSetPartition,
) -> Vec<PartitionedSourceFilePatch> {
    result
        .roots()
        .iter()
        .map(|root| {
            let files = result
                .files()
                .iter()
                .filter(|file| file.root() == root.id())
                .cloned()
                .collect::<Vec<_>>();
            let issues = result
                .root_issues()
                .iter()
                .filter(|issue| workspace_issue_root(issue) == root.id())
                .map(project_issue_from_workspace_issue)
                .collect::<Vec<_>>();
            PartitionedSourceFilePatch {
                partition: partition_for_root(root.id().clone()),
                roots: vec![root.clone()],
                summary: FileSetSummary::new(files.len()),
                files,
                issues,
            }
        })
        .collect()
}

fn workspace_issue_root(issue: &WorkspaceRootIssue) -> &SourceRootId {
    match issue {
        WorkspaceRootIssue::MissingRoot { root, .. }
        | WorkspaceRootIssue::UnreadableRoot { root, .. } => root,
    }
}

impl FirstPartySourceFilePatch {
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn first_party(
        root_plan_issues: Vec<SourceFilesIssue>,
        result: FilesForRootsResult,
    ) -> Self {
        let roots = result.roots().to_vec();
        let files = assign_longest_prefix_owners(result.files(), &roots);
        let mut issues = root_plan_issues;
        issues.extend(
            result
                .root_issues()
                .iter()
                .map(project_issue_from_workspace_issue),
        );
        Self {
            partition: FileSetPartition::first_party(),
            roots,
            summary: FileSetSummary::new(files.len()),
            files,
            issues,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn issues(&self) -> &[SourceFilesIssue] {
        &self.issues
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveredSourceFiles {
    roots: Vec<SourceRootEntry>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
}

impl DiscoveredSourceFiles {
    fn new(roots: Vec<SourceRootEntry>, files: Vec<DiscoveredSourceFile>) -> Self {
        let summary = FileSetSummary::new(files.len());
        Self {
            roots,
            files,
            summary,
        }
    }

    #[must_use]
    fn roots(&self) -> &[SourceRootEntry] {
        &self.roots
    }

    #[must_use]
    fn files(&self) -> &[DiscoveredSourceFile] {
        &self.files
    }

    #[must_use]
    fn summary(&self) -> FileSetSummary {
        self.summary
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFilesMaterializationPatch {
    changed_roots: Vec<SourceRootEntry>,
    removed_roots: Vec<SourceRootId>,
    upserted_files: Vec<DiscoveredSourceFile>,
    removed_files: Vec<Utf8PathBuf>,
    summary: FileSetSummary,
}

impl SourceFilesMaterializationPatch {
    #[must_use]
    pub fn changed_roots(&self) -> &[SourceRootEntry] {
        &self.changed_roots
    }

    #[must_use]
    pub fn removed_roots(&self) -> &[SourceRootId] {
        &self.removed_roots
    }

    #[must_use]
    pub fn upserted_files(&self) -> &[DiscoveredSourceFile] {
        &self.upserted_files
    }

    #[must_use]
    pub fn removed_files(&self) -> &[Utf8PathBuf] {
        &self.removed_files
    }

    #[must_use]
    pub fn summary(&self) -> FileSetSummary {
        self.summary
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFilePartitionTransition {
    partitions: Vec<FileSetPartition>,
    readiness: SourceFilePartitionReadiness,
}

impl SourceFilePartitionTransition {
    #[must_use]
    pub fn partition(&self) -> Option<&FileSetPartition> {
        self.partitions.first()
    }

    #[must_use]
    pub fn partitions(&self) -> &[FileSetPartition] {
        &self.partitions
    }

    #[must_use]
    pub fn readiness(&self) -> &SourceFilePartitionReadiness {
        &self.readiness
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFilesUpdate {
    partitions: SourceFileSetPartitions,
    materialization: SourceFilesMaterializationPatch,
    applied_transition: SourceFilePartitionTransition,
    issues: Vec<SourceFilesIssue>,
    apply_blocking_issues: Vec<SourceFilesIssue>,
}

impl SourceFilesUpdate {
    #[must_use]
    pub fn materialization(&self) -> &SourceFilesMaterializationPatch {
        &self.materialization
    }

    #[must_use]
    pub fn applied_transition(&self) -> &SourceFilePartitionTransition {
        &self.applied_transition
    }

    #[must_use]
    pub fn issues(&self) -> &[SourceFilesIssue] {
        &self.issues
    }

    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn decide_apply(
        self,
        previous: Option<ReadySourceFiles>,
        materialized: SourceFileSetMaterialized,
    ) -> SourceFilesApplyDecision {
        if let Some(issue) = first_fatal_update_issue(&self.apply_blocking_issues) {
            return terminal_source_files_apply_decision(
                self.applied_transition,
                issue,
                previous,
                TerminalSourceFilesAvailability::Unavailable,
            );
        }

        if let Some(issue) = materialized
            .issues
            .first()
            .map(project_issue_from_materialization_issue)
        {
            return terminal_source_files_apply_decision(
                self.applied_transition,
                issue,
                previous,
                TerminalSourceFilesAvailability::Failed,
            );
        }

        if !materialized_source_file_set_matches_update(&self, &materialized.discovered) {
            let issue = SourceFilesIssue::MaterializationFailed {
                path: Utf8PathBuf::from("<source-file-set>"),
                error_kind: std::io::ErrorKind::InvalidData,
            };
            return terminal_source_files_apply_decision(
                self.applied_transition,
                issue,
                previous,
                TerminalSourceFilesAvailability::Failed,
            );
        }

        let files = ReadySourceFiles::new(self.partitions, materialized.source_file_set);
        SourceFilesApplyDecision::new(
            SourceFilesApplyResult::Applied(SourceFilesApplied {
                files: files.clone(),
                transition: self.applied_transition,
                issues: self.issues,
            }),
            Some(SourceFileInventory::Ready(files)),
        )
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn partitions(&self) -> &SourceFileSetPartitions {
        &self.partitions
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFileSetMaterialized {
    source_file_set: SourceFileSet,
    discovered: DiscoveredSourceFiles,
    handle_changes: SourceFileHandleChanges,
    issues: Vec<SourceFileMaterializationIssue>,
}

impl SourceFileSetMaterialized {
    #[must_use]
    pub fn new(
        source_file_set: SourceFileSet,
        roots: Vec<SourceRootEntry>,
        files: Vec<DiscoveredSourceFile>,
        handle_changes: SourceFileHandleChanges,
        issues: Vec<SourceFileMaterializationIssue>,
    ) -> Self {
        Self {
            source_file_set,
            discovered: DiscoveredSourceFiles::new(roots, files),
            handle_changes,
            issues,
        }
    }

    #[must_use]
    pub fn source_file_set(&self) -> SourceFileSet {
        self.source_file_set
    }

    #[must_use]
    pub fn handle_changes(&self) -> &SourceFileHandleChanges {
        &self.handle_changes
    }

    #[must_use]
    pub fn issues(&self) -> &[SourceFileMaterializationIssue] {
        &self.issues
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceFileHandleChanges {
    preserved: usize,
    created: usize,
    removed: usize,
}

impl SourceFileHandleChanges {
    #[must_use]
    pub fn new(preserved: usize, created: usize, removed: usize) -> Self {
        Self {
            preserved,
            created,
            removed,
        }
    }

    #[must_use]
    /// Count of upserted paths that reused an existing `File` handle.
    ///
    /// This does not include unchanged paths carried forward by the patch.
    pub fn preserved(&self) -> usize {
        self.preserved
    }

    #[must_use]
    pub fn created(&self) -> usize {
        self.created
    }

    #[must_use]
    pub fn removed(&self) -> usize {
        self.removed
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFileMaterializationIssue {
    MissingRoot {
        root: SourceRootId,
    },
    MaterializationFailed {
        path: Utf8PathBuf,
        error_kind: std::io::ErrorKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFilesApplyResult {
    Applied(SourceFilesApplied),
    Deferred {
        transition: SourceFilePartitionTransition,
        issue: SourceFilesIssue,
        previous: Option<ReadySourceFiles>,
    },
    Unavailable {
        transition: SourceFilePartitionTransition,
        issue: SourceFilesIssue,
        previous: Option<ReadySourceFiles>,
    },
    Failed {
        transition: SourceFilePartitionTransition,
        issue: SourceFilesIssue,
        previous: Option<ReadySourceFiles>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFilesApplyDecision {
    result: SourceFilesApplyResult,
    next_inventory: Option<SourceFileInventory>,
}

impl SourceFilesApplyDecision {
    fn new(result: SourceFilesApplyResult, next_inventory: Option<SourceFileInventory>) -> Self {
        Self {
            result,
            next_inventory,
        }
    }

    #[must_use]
    pub fn result(&self) -> &SourceFilesApplyResult {
        &self.result
    }

    #[must_use]
    pub fn next_inventory(&self) -> Option<&SourceFileInventory> {
        self.next_inventory.as_ref()
    }

    #[must_use]
    pub fn into_result(self) -> SourceFilesApplyResult {
        self.result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFilesApplied {
    files: ReadySourceFiles,
    transition: SourceFilePartitionTransition,
    issues: Vec<SourceFilesIssue>,
}

impl SourceFilesApplied {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn for_test(
        files: ReadySourceFiles,
        readiness: SourceFilePartitionReadiness,
    ) -> Self {
        Self {
            files,
            transition: SourceFilePartitionTransition {
                partitions: vec![FileSetPartition::first_party()],
                readiness,
            },
            issues: Vec::new(),
        }
    }

    #[must_use]
    pub fn files(&self) -> &ReadySourceFiles {
        &self.files
    }

    #[must_use]
    pub fn transition(&self) -> &SourceFilePartitionTransition {
        &self.transition
    }

    #[must_use]
    pub fn issues(&self) -> &[SourceFilesIssue] {
        &self.issues
    }
}

fn first_fatal_update_issue(issues: &[SourceFilesIssue]) -> Option<SourceFilesIssue> {
    issues
        .iter()
        .find(|issue| !matches!(issue, SourceFilesIssue::PartitionConflict { .. }))
        .cloned()
}

enum TerminalSourceFilesAvailability {
    Unavailable,
    Failed,
}

#[allow(clippy::needless_pass_by_value)]
fn terminal_source_files_apply_decision(
    transition: SourceFilePartitionTransition,
    issue: SourceFilesIssue,
    previous: Option<ReadySourceFiles>,
    availability: TerminalSourceFilesAvailability,
) -> SourceFilesApplyDecision {
    let next_inventory = if previous.is_none() {
        Some(SourceFileInventory::Unavailable {
            issue: issue.clone(),
        })
    } else {
        None
    };
    let result = match availability {
        TerminalSourceFilesAvailability::Unavailable => SourceFilesApplyResult::Unavailable {
            transition,
            issue,
            previous,
        },
        TerminalSourceFilesAvailability::Failed => SourceFilesApplyResult::Failed {
            transition,
            issue,
            previous,
        },
    };
    SourceFilesApplyDecision::new(result, next_inventory)
}

fn materialized_source_file_set_matches_update(
    update: &SourceFilesUpdate,
    data: &DiscoveredSourceFiles,
) -> bool {
    let expected = update.partitions.merged_discovered_files();
    if expected.summary() != data.summary() {
        return false;
    }

    let expected_roots = expected
        .roots()
        .iter()
        .map(|entry| (entry.root().id().clone(), entry.root().clone()))
        .collect::<BTreeMap<_, _>>();
    let actual_roots = data
        .roots()
        .iter()
        .map(|entry| (entry.root().id().clone(), entry.root().clone()))
        .collect::<BTreeMap<_, _>>();
    if expected_roots != actual_roots {
        return false;
    }

    let expected_files = expected
        .files()
        .iter()
        .map(|file| (file.path().to_owned(), file.root().clone()))
        .collect::<BTreeMap<_, _>>();
    let actual_files = data
        .files()
        .iter()
        .map(|file| (file.path().to_owned(), file.root().clone()))
        .collect::<BTreeMap<_, _>>();
    expected_files == actual_files
}

fn project_issue_from_materialization_issue(
    issue: &SourceFileMaterializationIssue,
) -> SourceFilesIssue {
    match issue {
        SourceFileMaterializationIssue::MissingRoot { root } => SourceFilesIssue::MissingRoot {
            root: root.clone(),
            path: root.as_path().to_owned(),
        },
        SourceFileMaterializationIssue::MaterializationFailed { path, error_kind } => {
            SourceFilesIssue::MaterializationFailed {
                path: path.clone(),
                error_kind: *error_kind,
            }
        }
    }
}

#[cfg(test)]
#[must_use]
fn merge_partitioned_source_file_patch(
    current: Option<&ReadySourceFiles>,
    patch: PartitionedSourceFilePatch,
) -> SourceFilesUpdate {
    merge_partitioned_source_file_patch_set(
        current,
        PartitionedSourceFilePatchSet {
            group: patch.partition.id().group(),
            patches: vec![patch],
            issues: Vec::new(),
        },
    )
}

#[must_use]
pub(crate) fn merge_partitioned_source_file_patch_set(
    current: Option<&ReadySourceFiles>,
    patch_set: PartitionedSourceFilePatchSet,
) -> SourceFilesUpdate {
    let readiness = partition_set_readiness(current, &patch_set);
    let snapshots = patch_set
        .patches
        .iter()
        .map(|patch| {
            SourceFileSetPartitionSnapshot::new(
                patch.partition.clone(),
                patch.roots.clone(),
                patch.files.clone(),
                patch_readiness(current, patch),
            )
        })
        .collect::<Vec<_>>();
    let current_partitions = current
        .map(|files| files.partitions.clone())
        .unwrap_or_default();
    let partitions = current_partitions.replace_partition_group(patch_set.group, snapshots);
    let merged = partitions.merged_discovered_files();
    let previous = current.map(ReadySourceFiles::discovered_files);
    let materialization = materialization_patch(previous.as_ref(), &merged);
    let applied_transition = SourceFilePartitionTransition {
        partitions: patch_set
            .patches
            .iter()
            .map(|patch| patch.partition.clone())
            .collect(),
        readiness,
    };
    let mut issues = patch_set.issues;
    issues.extend(patch_set.patches.into_iter().flat_map(|patch| patch.issues));
    issues.extend(partition_conflicts(&partitions));

    SourceFilesUpdate {
        partitions,
        materialization,
        applied_transition,
        issues,
        apply_blocking_issues: Vec::new(),
    }
}

fn partition_set_readiness(
    current: Option<&ReadySourceFiles>,
    patch_set: &PartitionedSourceFilePatchSet,
) -> SourceFilePartitionReadiness {
    if let Some(issue) = patch_set
        .patches
        .iter()
        .flat_map(|patch| patch.issues.iter())
        .next()
    {
        return SourceFilePartitionReadiness::Unavailable {
            issue: issue.clone(),
            previous: current.map(|files| files.discovered_files().summary()),
        };
    }

    SourceFilePartitionReadiness::Ready {
        summary: FileSetSummary::new(
            patch_set
                .patches
                .iter()
                .map(|patch| patch.summary.included_files())
                .sum(),
        ),
    }
}

fn patch_readiness(
    current: Option<&ReadySourceFiles>,
    patch: &PartitionedSourceFilePatch,
) -> SourceFilePartitionReadiness {
    if let Some(issue) = patch.issues.first() {
        return SourceFilePartitionReadiness::Unavailable {
            issue: issue.clone(),
            previous: current.map(|files| files.discovered_files().summary()),
        };
    }

    SourceFilePartitionReadiness::Ready {
        summary: patch.summary,
    }
}

fn partition_conflicts(partitions: &SourceFileSetPartitions) -> Vec<SourceFilesIssue> {
    let mut winners = BTreeMap::<Utf8PathBuf, FileSetPartitionId>::new();
    let mut issues = Vec::new();
    for partition in &partitions.partitions {
        for file in &partition.files {
            if let Some(winner) = winners.get(file.path()) {
                issues.push(SourceFilesIssue::PartitionConflict {
                    path: file.path().to_owned(),
                    winner: winner.clone(),
                    shadowed: partition.partition.id().clone(),
                });
            } else {
                winners.insert(file.path().to_owned(), partition.partition.id().clone());
            }
        }
    }
    issues
}

#[must_use]
pub(crate) fn merge_first_party_source_file_patch(
    current: Option<&ReadySourceFiles>,
    patch: FirstPartySourceFilePatch,
) -> SourceFilesUpdate {
    let readiness = first_party_readiness(current, &patch);
    let snapshot = SourceFileSetPartitionSnapshot::new(
        patch.partition.clone(),
        patch.roots.clone(),
        patch.files.clone(),
        readiness.clone(),
    );
    let current_partitions = current
        .map(|files| files.partitions.clone())
        .unwrap_or_default();
    let partitions = current_partitions.replace_partition(snapshot);
    let merged = partitions.merged_discovered_files();
    let previous = current.map(ReadySourceFiles::discovered_files);
    let materialization = materialization_patch(previous.as_ref(), &merged);
    let applied_transition = SourceFilePartitionTransition {
        partitions: vec![patch.partition],
        readiness,
    };

    SourceFilesUpdate {
        partitions,
        materialization,
        applied_transition,
        issues: patch.issues.clone(),
        apply_blocking_issues: patch.issues,
    }
}

fn first_party_readiness(
    current: Option<&ReadySourceFiles>,
    patch: &FirstPartySourceFilePatch,
) -> SourceFilePartitionReadiness {
    if let Some(issue) = patch.issues.first() {
        return SourceFilePartitionReadiness::Unavailable {
            issue: issue.clone(),
            previous: current.map(|files| files.discovered_files().summary()),
        };
    }

    SourceFilePartitionReadiness::Ready {
        summary: patch.summary,
    }
}

fn materialization_patch(
    previous: Option<&DiscoveredSourceFiles>,
    merged: &DiscoveredSourceFiles,
) -> SourceFilesMaterializationPatch {
    let previous_roots = previous
        .map(|data| {
            data.roots()
                .iter()
                .map(|entry| entry.root().id().clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let next_roots = merged
        .roots()
        .iter()
        .map(|entry| entry.root().id().clone())
        .collect::<BTreeSet<_>>();
    let removed_roots = previous_roots.difference(&next_roots).cloned().collect();
    let previous_root_entries = previous
        .map(|data| {
            data.roots()
                .iter()
                .map(|entry| (entry.root().id().clone(), entry.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let changed_roots = merged
        .roots()
        .iter()
        .filter(|entry| previous_root_entries.get(entry.root().id()) != Some(*entry))
        .cloned()
        .collect();

    let previous_files = previous
        .map(|data| {
            data.files()
                .iter()
                .map(|file| (file.path().to_owned(), file.root().clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let next_files = merged
        .files()
        .iter()
        .map(|file| (file.path().to_owned(), file.root().clone()))
        .collect::<BTreeMap<_, _>>();

    let upserted_files = merged
        .files()
        .iter()
        .filter(|file| previous_files.get(file.path()) != Some(file.root()))
        .cloned()
        .collect();
    let removed_files = previous_files
        .keys()
        .filter(|path| !next_files.contains_key(*path))
        .cloned()
        .collect();

    SourceFilesMaterializationPatch {
        changed_roots,
        removed_roots,
        upserted_files,
        removed_files,
        summary: merged.summary(),
    }
}

fn assign_longest_prefix_owners(
    files: &[DiscoveredSourceFile],
    roots: &[SourceRoot],
) -> Vec<DiscoveredSourceFile> {
    let mut by_path = BTreeMap::<Utf8PathBuf, DiscoveredSourceFile>::new();
    for file in files {
        let Some(owner) = longest_prefix_root(file.path(), roots) else {
            continue;
        };
        let owned_file = DiscoveredSourceFile::new(file.path().to_owned(), owner.id().clone());
        by_path.insert(file.path().to_owned(), owned_file);
    }
    by_path.into_values().collect()
}

fn longest_prefix_root<'a>(path: &Utf8Path, roots: &'a [SourceRoot]) -> Option<&'a SourceRoot> {
    roots
        .iter()
        .filter(|root| path.starts_with(root.path()))
        .max_by_key(|root| root.path().as_str().len())
}

fn project_issue_from_workspace_issue(issue: &WorkspaceRootIssue) -> SourceFilesIssue {
    match issue {
        WorkspaceRootIssue::MissingRoot { root, path } => SourceFilesIssue::MissingRoot {
            root: root.clone(),
            path: path.clone(),
        },
        WorkspaceRootIssue::UnreadableRoot {
            root,
            path,
            error_kind,
        } => SourceFilesIssue::WalkFailed {
            root: root.clone(),
            path: path.clone(),
            error_kind: *error_kind,
        },
    }
}

#[cfg(test)]
mod tests {
    use djls_source::LoadedSourceFile;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_workspace::load_files_for_roots;

    use super::*;

    fn utf8(path: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap()
    }

    fn root(path: &str) -> SourceRoot {
        let path = Utf8PathBuf::from(path);
        SourceRoot::new(SourceRootId::new(path.clone()), path, FileRootKind::Project)
    }

    fn root_path(path: Utf8PathBuf) -> SourceRoot {
        SourceRoot::new(SourceRootId::new(path.clone()), path, FileRootKind::Project)
    }

    fn discovered(path: &str, root: &SourceRoot) -> DiscoveredSourceFile {
        DiscoveredSourceFile::new(Utf8PathBuf::from(path), root.id().clone())
    }

    fn load_first_party_files(
        plan: SourceRootsPlan,
    ) -> (Vec<SourceFilesIssue>, FilesForRootsResult) {
        let (root_issues, request) =
            first_party_discovery_files_request(first_party_source_files_load_request(plan));
        (root_issues, load_files_for_roots(request))
    }

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: djls_source::SourceFiles,
        project: std::sync::Mutex<Option<crate::Project>>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &djls_source::SourceFiles {
            &self.files
        }

        fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn project(&self) -> crate::Project {
            self.project
                .lock()
                .unwrap()
                .expect("test database should initialize project")
        }
    }

    impl TestDb {
        fn with_project() -> Self {
            let db = Self::default();
            let project = crate::Project::virtual_project(&db);
            *db.project.lock().unwrap() = Some(project);
            db
        }
    }

    fn materialized_source_files(
        db: &TestDb,
        roots: Vec<SourceRootEntry>,
        files: Vec<DiscoveredSourceFile>,
    ) -> SourceFileSetMaterialized {
        let loaded = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                LoadedSourceFile::from_discovered(
                    file.clone(),
                    djls_source::File::new(
                        db,
                        file.path().to_owned(),
                        u64::try_from(index).expect("test file index should fit in u64"),
                    ),
                )
            })
            .collect::<Vec<_>>();
        let data = SourceFileSetData::new(roots.clone(), loaded)
            .expect("materialized source files should be coherent");
        SourceFileSetMaterialized::new(
            SourceFileSet::new(db, data),
            roots,
            files,
            SourceFileHandleChanges::default(),
            Vec::new(),
        )
    }

    fn materialized_for_update(
        db: &TestDb,
        update: &SourceFilesUpdate,
    ) -> SourceFileSetMaterialized {
        let discovered = update.partitions().merged_discovered_files();
        materialized_source_files(db, discovered.roots().to_vec(), discovered.files().to_vec())
    }

    fn ready_files_for_update(db: &TestDb, update: SourceFilesUpdate) -> ReadySourceFiles {
        let materialized = materialized_for_update(db, &update);
        let decision = update.decide_apply(None, materialized);
        let SourceFilesApplyResult::Applied(applied) = decision.into_result() else {
            panic!("source files should apply");
        };
        applied.files().clone()
    }

    #[test]
    fn decide_apply_rejects_mismatched_materialized_source_file_set() {
        let db = TestDb::with_project();
        let update_root = root("/workspace");
        let update_file = discovered("/workspace/models.py", &update_root);
        let patch = FirstPartySourceFilePatch {
            partition: FileSetPartition::first_party(),
            roots: vec![update_root],
            files: vec![update_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };
        let update = merge_first_party_source_file_patch(None, patch);

        let other_root = root("/other");
        let other_file = discovered("/other/other.py", &other_root);
        let loaded = LoadedSourceFile::from_discovered(
            other_file.clone(),
            djls_source::File::new(&db, Utf8PathBuf::from("/other/other.py"), 0),
        );
        let roots = vec![SourceRootEntry::new(other_root)];
        let files = vec![other_file];
        let data = SourceFileSetData::new(roots.clone(), vec![loaded])
            .expect("mismatched source file set should be internally coherent");
        let materialized = SourceFileSetMaterialized::new(
            SourceFileSet::new(&db, data),
            roots,
            files,
            SourceFileHandleChanges::default(),
            Vec::new(),
        );

        let decision = update.decide_apply(None, materialized);

        assert!(matches!(
            decision.result(),
            SourceFilesApplyResult::Failed { .. }
        ));
        assert!(matches!(
            decision.next_inventory(),
            Some(SourceFileInventory::Unavailable { .. })
        ));
    }

    #[test]
    fn decide_apply_applied_result_publishes_ready_inventory() {
        let db = TestDb::with_project();
        let update_root = root("/workspace");
        let update_file = discovered("/workspace/models.py", &update_root);
        let patch = FirstPartySourceFilePatch {
            partition: FileSetPartition::first_party(),
            roots: vec![update_root],
            files: vec![update_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };
        let update = merge_first_party_source_file_patch(None, patch);
        let materialized = materialized_for_update(&db, &update);

        let decision = update.decide_apply(None, materialized);

        let SourceFilesApplyResult::Applied(applied) = decision.result() else {
            panic!("matching materialization should apply");
        };
        assert_eq!(
            decision.next_inventory(),
            Some(&SourceFileInventory::Ready(applied.files().clone()))
        );
    }

    #[test]
    fn decide_apply_mismatch_with_previous_ready_preserves_previous_inventory() {
        let db = TestDb::with_project();
        let previous_root = root("/workspace");
        let previous_file = discovered("/workspace/models.py", &previous_root);
        let previous_update = merge_first_party_source_file_patch(
            None,
            FirstPartySourceFilePatch {
                partition: FileSetPartition::first_party(),
                roots: vec![previous_root],
                files: vec![previous_file],
                summary: FileSetSummary::new(1),
                issues: Vec::new(),
            },
        );
        let previous = ready_files_for_update(&db, previous_update);
        let update_root = root("/new");
        let update_file = discovered("/new/models.py", &update_root);
        let update = merge_first_party_source_file_patch(
            Some(&previous),
            FirstPartySourceFilePatch {
                partition: FileSetPartition::first_party(),
                roots: vec![update_root],
                files: vec![update_file],
                summary: FileSetSummary::new(1),
                issues: Vec::new(),
            },
        );
        let other_root = root("/other");
        let other_file = discovered("/other/other.py", &other_root);
        let materialized = materialized_source_files(
            &db,
            vec![SourceRootEntry::new(other_root)],
            vec![other_file],
        );

        let decision = update.decide_apply(Some(previous.clone()), materialized);

        let SourceFilesApplyResult::Failed { previous: kept, .. } = decision.result() else {
            panic!("mismatched materialization should fail");
        };
        assert_eq!(kept, &Some(previous));
        assert_eq!(decision.next_inventory(), None);
    }

    #[test]
    fn decide_apply_terminal_issue_with_previous_ready_preserves_previous_inventory() {
        let db = TestDb::with_project();
        let previous_root = root("/workspace");
        let previous_file = discovered("/workspace/models.py", &previous_root);
        let previous_update = merge_first_party_source_file_patch(
            None,
            FirstPartySourceFilePatch {
                partition: FileSetPartition::first_party(),
                roots: vec![previous_root],
                files: vec![previous_file],
                summary: FileSetSummary::new(1),
                issues: Vec::new(),
            },
        );
        let previous = ready_files_for_update(&db, previous_update);
        let missing_root = root("/missing");
        let missing_issue = SourceFilesIssue::MissingRoot {
            root: missing_root.id().clone(),
            path: missing_root.path().to_owned(),
        };
        let update = merge_first_party_source_file_patch(
            Some(&previous),
            FirstPartySourceFilePatch {
                partition: FileSetPartition::first_party(),
                roots: vec![missing_root],
                files: Vec::new(),
                summary: FileSetSummary::new(0),
                issues: vec![missing_issue],
            },
        );
        let materialized = materialized_for_update(&db, &update);

        let decision = update.decide_apply(Some(previous.clone()), materialized);

        let SourceFilesApplyResult::Unavailable { previous: kept, .. } = decision.result() else {
            panic!("terminal update issue should be unavailable");
        };
        assert_eq!(kept, &Some(previous));
        assert_eq!(decision.next_inventory(), None);
    }

    #[test]
    fn partition_patch_set_publishes_successful_siblings_with_root_issues() {
        let db = TestDb::with_project();
        let first_party_root = root("/workspace");
        let previous_update = merge_first_party_source_file_patch(
            None,
            FirstPartySourceFilePatch {
                partition: FileSetPartition::first_party(),
                roots: vec![first_party_root],
                files: Vec::new(),
                summary: FileSetSummary::new(0),
                issues: Vec::new(),
            },
        );
        let previous = ready_files_for_update(&db, previous_update);
        let ok_dir = tempfile::tempdir().expect("tempdir should be created");
        let ok_root = root_path(utf8(ok_dir.path()).join("templates"));
        std::fs::create_dir_all(ok_root.path()).expect("template root should be created");
        std::fs::write(ok_root.path().join("index.html"), "").expect("template should be written");
        let missing_root = root_path(utf8(ok_dir.path()).join("missing"));
        let result = load_files_for_roots(FilesForRootsRequest::new(
            vec![ok_root, missing_root],
            Box::new(|_| true),
            WalkOptions::default(),
        ));
        let update = merge_partitioned_source_file_patch_set(
            Some(&previous),
            PartitionedSourceFilePatchSet::configured_template_directories(result),
        );
        let materialized = materialized_for_update(&db, &update);

        let decision = update.decide_apply(Some(previous), materialized);

        let SourceFilesApplyResult::Applied(applied) = decision.result() else {
            panic!("partition root issues should not abort the whole patch set");
        };
        assert!(matches!(
            applied.transition().readiness(),
            SourceFilePartitionReadiness::Unavailable { .. }
        ));
        assert!(matches!(
            decision.next_inventory(),
            Some(SourceFileInventory::Ready(_))
        ));
    }

    #[test]
    fn roots_builder_deduplicates_duplicate_roots_and_reports_issue() {
        let plan = build_source_roots([
            Utf8PathBuf::from("/workspace"),
            Utf8PathBuf::from("/workspace"),
        ]);

        assert_eq!(plan.roots().len(), 1);
        assert_eq!(
            plan.issues(),
            &[SourceFilesIssue::DuplicateRoot {
                root: SourceRootId::new(Utf8PathBuf::from("/workspace")),
                duplicate_path: Utf8PathBuf::from("/workspace"),
            }]
        );
    }

    #[test]
    fn roots_builder_deduplicates_canonical_root_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        let plan = build_source_roots([root.clone(), root.join(".")]);

        assert_eq!(plan.roots().len(), 1);
        assert_eq!(
            plan.issues(),
            &[SourceFilesIssue::DuplicateRoot {
                root: SourceRootId::new(root.clone()),
                duplicate_path: root.join("."),
            }]
        );
    }

    #[test]
    fn roots_builder_preserves_missing_root_fallback_identity() {
        let dir = tempfile::tempdir().unwrap();
        let missing = utf8(dir.path()).join("missing");
        let plan = build_source_roots([missing.clone()]);

        assert_eq!(plan.roots().len(), 1);
        assert_eq!(plan.roots()[0].id(), &SourceRootId::new(missing.clone()));
        assert_eq!(plan.roots()[0].path(), missing.as_path());
        assert!(plan.issues().is_empty());
    }

    #[test]
    fn duplicate_root_issue_flows_through_first_party_update() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        let plan = build_source_roots([root.clone(), root.clone()]);
        let (root_issues, result) = load_first_party_files(plan);

        let update = merge_first_party_source_file_patch(
            None,
            FirstPartySourceFilePatch::first_party(root_issues, result),
        );

        assert_eq!(
            update.issues(),
            &[SourceFilesIssue::DuplicateRoot {
                root: SourceRootId::new(root.clone()),
                duplicate_path: root,
            }]
        );
    }

    #[test]
    fn first_party_request_uses_project_file_policy() {
        let request = first_party_source_files_load_request(SourceRootsPlan {
            roots: vec![root("/workspace")],
            issues: Vec::new(),
        });
        let (_root_issues, files_request) = first_party_discovery_files_request(request);

        assert_eq!(files_request.roots().len(), 1);
        assert!((files_request.options().globs)
            .iter()
            .any(|glob| glob.contains(".venv")));
    }

    #[test]
    fn first_party_patch_maps_root_issues() {
        let dir = tempfile::tempdir().unwrap();
        let missing = root_path(utf8(dir.path()).join("missing"));
        let (root_issues, result) = load_first_party_files(SourceRootsPlan {
            roots: vec![missing.clone()],
            issues: Vec::new(),
        });

        let patch = FirstPartySourceFilePatch::first_party(root_issues, result);

        assert_eq!(
            patch.issues(),
            &[SourceFilesIssue::MissingRoot {
                root: missing.id().clone(),
                path: missing.path().to_owned(),
            }]
        );
    }

    #[test]
    fn first_party_merge_uses_longest_prefix_owner_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let parent = root_path(utf8(dir.path()));
        let child_path = parent.path().join("app");
        std::fs::create_dir_all(child_path.join("templates")).unwrap();
        std::fs::write(child_path.join("templates/index.html"), "").unwrap();
        let child = root_path(child_path);
        let (root_issues, result) = load_first_party_files(SourceRootsPlan {
            roots: vec![parent, child.clone()],
            issues: Vec::new(),
        });

        let patch = FirstPartySourceFilePatch::first_party(root_issues, result);
        let update = merge_first_party_source_file_patch(None, patch);

        assert_eq!(update.materialization().upserted_files().len(), 1);
        assert_eq!(
            update.materialization().upserted_files()[0].root(),
            child.id()
        );
        assert_eq!(update.materialization().summary(), FileSetSummary::new(1));
    }

    #[test]
    fn merge_patch_records_root_removal_by_source_root_id() {
        #[salsa::db]
        #[derive(Default)]
        struct TestDb {
            storage: salsa::Storage<Self>,
            files: djls_source::SourceFiles,
        }

        #[salsa::db]
        impl salsa::Database for TestDb {}

        #[salsa::db]
        impl djls_source::Db for TestDb {
            fn files(&self) -> &djls_source::SourceFiles {
                &self.files
            }

            fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
                Ok(String::new())
            }
        }

        let db = TestDb::default();
        let removed = root("/removed");
        let kept_dir = tempfile::tempdir().unwrap();
        let kept = root_path(utf8(kept_dir.path()));
        let previous_file = discovered("/removed/a.html", &removed);
        let loaded_file = LoadedSourceFile::from_discovered(
            previous_file.clone(),
            djls_source::File::new(&db, previous_file.path().to_owned(), 0),
        );
        let set_data = SourceFileSetData::new(
            vec![SourceRootEntry::new(removed.clone())],
            vec![loaded_file],
        )
        .unwrap();
        let previous_partition = SourceFileSetPartitionSnapshot::new(
            FileSetPartition::first_party(),
            vec![removed.clone()],
            vec![previous_file],
            SourceFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            },
        );
        let previous = ReadySourceFiles::materialized_for_test(
            SourceFileSetPartitions::with_first_party(previous_partition),
            SourceFileSet::new(&db, set_data),
        );
        let (root_issues, result) = load_first_party_files(SourceRootsPlan {
            roots: vec![kept.clone()],
            issues: Vec::new(),
        });
        let patch = FirstPartySourceFilePatch::first_party(root_issues, result);

        let update = merge_first_party_source_file_patch(Some(&previous), patch);

        assert_eq!(
            update.materialization().removed_roots(),
            &[removed.id().clone()]
        );
        assert_eq!(
            update.materialization().removed_files(),
            &[Utf8PathBuf::from("/removed/a.html")]
        );
    }

    #[test]
    fn materialization_patch_reports_only_changed_roots() {
        #[salsa::db]
        #[derive(Default)]
        struct TestDb {
            storage: salsa::Storage<Self>,
            files: djls_source::SourceFiles,
        }

        #[salsa::db]
        impl salsa::Database for TestDb {}

        #[salsa::db]
        impl djls_source::Db for TestDb {
            fn files(&self) -> &djls_source::SourceFiles {
                &self.files
            }

            fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
                Ok(String::new())
            }
        }

        let db = TestDb::default();
        let kept = root("/kept");
        let removed = root("/removed");
        let added = root("/added");
        let kept_file = discovered("/kept/a.html", &kept);
        let loaded_file = LoadedSourceFile::from_discovered(
            kept_file.clone(),
            djls_source::File::new(&db, kept_file.path().to_owned(), 0),
        );
        let set_data = SourceFileSetData::new(
            vec![
                SourceRootEntry::new(kept.clone()),
                SourceRootEntry::new(removed.clone()),
            ],
            vec![loaded_file],
        )
        .unwrap();
        let previous_partition = SourceFileSetPartitionSnapshot::new(
            FileSetPartition::first_party(),
            vec![kept.clone(), removed.clone()],
            vec![kept_file.clone()],
            SourceFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            },
        );
        let previous = ReadySourceFiles::materialized_for_test(
            SourceFileSetPartitions::with_first_party(previous_partition),
            SourceFileSet::new(&db, set_data),
        );
        let result = load_first_party_files(SourceRootsPlan {
            roots: vec![kept.clone(), added.clone()],
            issues: Vec::new(),
        });
        let update = merge_first_party_source_file_patch(
            Some(&previous),
            FirstPartySourceFilePatch::first_party(result.0, result.1),
        );

        assert_eq!(
            update.materialization().changed_roots(),
            &[SourceRootEntry::new(added)]
        );
        assert_eq!(
            update.materialization().removed_roots(),
            &[removed.id().clone()]
        );
        assert!(update.materialization().upserted_files().is_empty());
    }

    #[test]
    fn missing_root_issue_produces_unavailable_readiness() {
        let dir = tempfile::tempdir().unwrap();
        let missing = root_path(utf8(dir.path()).join("missing"));
        let (root_issues, result) = load_first_party_files(SourceRootsPlan {
            roots: vec![missing.clone()],
            issues: Vec::new(),
        });

        let update = merge_first_party_source_file_patch(
            None,
            FirstPartySourceFilePatch::first_party(root_issues, result),
        );

        assert_eq!(
            update.applied_transition().readiness(),
            &SourceFilePartitionReadiness::Unavailable {
                issue: SourceFilesIssue::MissingRoot {
                    root: missing.id().clone(),
                    path: missing.path().to_owned(),
                },
                previous: None,
            }
        );
    }

    #[test]
    fn loading_template_files_merge_prefers_first_party_over_lower_partitions() {
        let first_party = root("/workspace");
        let installed = SourceRoot::new(
            SourceRootId::new(Utf8PathBuf::from("/site-packages/blog")),
            Utf8PathBuf::from("/site-packages/blog"),
            FileRootKind::LibrarySearchPath,
        );
        let shared_path = Utf8PathBuf::from("/workspace/templates/shared.html");
        let first_party_file =
            DiscoveredSourceFile::new(shared_path.clone(), first_party.id().clone());
        let installed_file = DiscoveredSourceFile::new(shared_path.clone(), installed.id().clone());
        let first_party_patch = FirstPartySourceFilePatch {
            partition: FileSetPartition::first_party(),
            roots: vec![first_party.clone()],
            files: vec![first_party_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };
        let first_update = merge_first_party_source_file_patch(None, first_party_patch);
        let previous = ReadySourceFiles::materialized_for_test(
            first_update.partitions().clone(),
            empty_source_file_set(),
        );
        let installed_patch = PartitionedSourceFilePatch {
            partition: FileSetPartition::installed_app(installed.id().clone()),
            roots: vec![installed.clone()],
            files: vec![installed_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };

        let update = merge_partitioned_source_file_patch(Some(&previous), installed_patch);

        assert_eq!(update.materialization().summary(), FileSetSummary::new(1));
        assert!(update
            .issues()
            .contains(&SourceFilesIssue::PartitionConflict {
                path: shared_path,
                winner: FileSetPartitionId::FirstParty,
                shadowed: FileSetPartitionId::InstalledApp(installed.id().clone()),
            }));
    }

    #[test]
    fn loading_template_files_first_party_update_preserves_lower_partition_for_resurrection() {
        let first_party = root("/workspace");
        let installed = SourceRoot::new(
            SourceRootId::new(Utf8PathBuf::from("/site-packages/blog")),
            Utf8PathBuf::from("/site-packages/blog"),
            FileRootKind::LibrarySearchPath,
        );
        let shared_path = Utf8PathBuf::from("/workspace/templates/shared.html");
        let first_party_file =
            DiscoveredSourceFile::new(shared_path.clone(), first_party.id().clone());
        let installed_file = DiscoveredSourceFile::new(shared_path.clone(), installed.id().clone());
        let first_party_patch = FirstPartySourceFilePatch {
            partition: FileSetPartition::first_party(),
            roots: vec![first_party.clone()],
            files: vec![first_party_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };
        let first_update = merge_first_party_source_file_patch(None, first_party_patch);
        let with_first = ReadySourceFiles::materialized_for_test(
            first_update.partitions().clone(),
            empty_source_file_set(),
        );
        let installed_patch = PartitionedSourceFilePatch {
            partition: FileSetPartition::installed_app(installed.id().clone()),
            roots: vec![installed.clone()],
            files: vec![installed_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };
        let installed_update =
            merge_partitioned_source_file_patch(Some(&with_first), installed_patch);
        let with_both = ReadySourceFiles::materialized_for_test(
            installed_update.partitions().clone(),
            empty_source_file_set(),
        );
        let remove_first_party = FirstPartySourceFilePatch {
            partition: FileSetPartition::first_party(),
            roots: vec![first_party],
            files: Vec::new(),
            summary: FileSetSummary::new(0),
            issues: Vec::new(),
        };

        let update = merge_first_party_source_file_patch(Some(&with_both), remove_first_party);

        assert_eq!(
            update.materialization().upserted_files()[0].root(),
            installed.id()
        );
        assert_eq!(
            update.materialization().upserted_files()[0].path(),
            shared_path.as_path()
        );
    }

    #[test]
    fn loading_template_files_resurrects_lower_precedence_file_after_higher_partition_removal() {
        let configured = root("/workspace/templates");
        let installed = SourceRoot::new(
            SourceRootId::new(Utf8PathBuf::from("/site-packages/blog")),
            Utf8PathBuf::from("/site-packages/blog"),
            FileRootKind::LibrarySearchPath,
        );
        let shared_path = Utf8PathBuf::from("/workspace/templates/shared.html");
        let configured_file =
            DiscoveredSourceFile::new(shared_path.clone(), configured.id().clone());
        let installed_file = DiscoveredSourceFile::new(shared_path.clone(), installed.id().clone());
        let configured_patch = PartitionedSourceFilePatch {
            partition: FileSetPartition::configured_template_directory(configured.id().clone()),
            roots: vec![configured.clone()],
            files: vec![configured_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };
        let configured_update = merge_partitioned_source_file_patch(None, configured_patch);
        let with_configured = ReadySourceFiles::materialized_for_test(
            configured_update.partitions().clone(),
            empty_source_file_set(),
        );
        let installed_patch = PartitionedSourceFilePatch {
            partition: FileSetPartition::installed_app(installed.id().clone()),
            roots: vec![installed.clone()],
            files: vec![installed_file],
            summary: FileSetSummary::new(1),
            issues: Vec::new(),
        };
        let installed_update =
            merge_partitioned_source_file_patch(Some(&with_configured), installed_patch);
        let with_both = ReadySourceFiles::materialized_for_test(
            installed_update.partitions().clone(),
            empty_source_file_set(),
        );
        let remove_configured = PartitionedSourceFilePatch {
            partition: FileSetPartition::configured_template_directory(configured.id().clone()),
            roots: vec![configured],
            files: Vec::new(),
            summary: FileSetSummary::new(0),
            issues: Vec::new(),
        };

        let update = merge_partitioned_source_file_patch(Some(&with_both), remove_configured);

        assert_eq!(
            update.materialization().upserted_files()[0].root(),
            installed.id()
        );
        assert_eq!(
            update.materialization().upserted_files()[0].path(),
            shared_path.as_path()
        );
    }

    fn empty_source_file_set() -> SourceFileSet {
        #[salsa::db]
        #[derive(Default)]
        struct TestDb {
            storage: salsa::Storage<Self>,
            files: djls_source::SourceFiles,
        }

        #[salsa::db]
        impl salsa::Database for TestDb {}

        #[salsa::db]
        impl djls_source::Db for TestDb {
            fn files(&self) -> &djls_source::SourceFiles {
                &self.files
            }

            fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
                Ok(String::new())
            }
        }

        let db = TestDb::default();
        SourceFileSet::new(&db, SourceFileSetData::new(Vec::new(), Vec::new()).unwrap())
    }

    #[test]
    fn transition_preserves_partition_readiness_for_status_projection() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_path(utf8(dir.path()));
        std::fs::write(root.path().join("a.html"), "").unwrap();
        let (root_issues, result) = load_first_party_files(SourceRootsPlan {
            roots: vec![root],
            issues: Vec::new(),
        });

        let update = merge_first_party_source_file_patch(
            None,
            FirstPartySourceFilePatch::first_party(root_issues, result),
        );

        assert_eq!(
            update
                .applied_transition()
                .partition()
                .expect("single partition transition")
                .id(),
            &FileSetPartitionId::FirstParty
        );
        assert_eq!(
            update.applied_transition().readiness(),
            &SourceFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            }
        );
        assert_eq!(
            update.partitions().first_party_readiness(),
            Some(&SourceFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            })
        );
    }
}
