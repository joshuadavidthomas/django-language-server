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

use crate::Db;
use crate::ProjectSourceFilesAvailability;
use crate::ProjectSourceFilesIssue;
use crate::ReadyProjectSourceFiles;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceRootsPlan {
    roots: Vec<SourceRoot>,
    issues: Vec<ProjectSourceFilesIssue>,
}

impl SourceRootsPlan {
    #[must_use]
    pub fn roots(&self) -> &[SourceRoot] {
        &self.roots
    }

    #[must_use]
    pub fn issues(&self) -> &[ProjectSourceFilesIssue] {
        &self.issues
    }
}

#[must_use]
pub fn build_source_roots(raw_roots: impl IntoIterator<Item = Utf8PathBuf>) -> SourceRootsPlan {
    let mut roots = Vec::new();
    let mut issues = Vec::new();
    let mut seen = BTreeSet::new();

    for raw_path in raw_roots {
        let path = canonical_root_path(&raw_path);
        let id = SourceRootId::new(path.clone());
        if !seen.insert(id.clone()) {
            issues.push(ProjectSourceFilesIssue::DuplicateRoot {
                root: id,
                duplicate_path: raw_path,
            });
            continue;
        }

        roots.push(SourceRoot::new(id, path, FileRootKind::Project));
    }

    SourceRootsPlan { roots, issues }
}

fn canonical_root_path(path: &Utf8Path) -> Utf8PathBuf {
    dunce::canonicalize(path)
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| path.to_owned())
}

pub struct SourceFilesLoadRequest {
    roots: Vec<SourceRoot>,
    root_issues: Vec<ProjectSourceFilesIssue>,
    predicate: FileLoadPredicate,
    options: WalkOptions,
}

impl SourceFilesLoadRequest {
    fn new(
        roots: Vec<SourceRoot>,
        root_issues: Vec<ProjectSourceFilesIssue>,
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

    #[must_use]
    pub fn roots(&self) -> &[SourceRoot] {
        &self.roots
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
pub fn first_party_source_files_load_request(plan: SourceRootsPlan) -> SourceFilesLoadRequest {
    SourceFilesLoadRequest::new(
        plan.roots,
        plan.issues,
        first_party_file_predicate(),
        first_party_walk_options(),
    )
}

#[must_use]
pub fn first_party_discovery_files_request(
    request: SourceFilesLoadRequest,
) -> (Vec<ProjectSourceFilesIssue>, FilesForRootsRequest) {
    let files_request =
        FilesForRootsRequest::new(request.roots, request.predicate, request.options);
    (request.root_issues, files_request)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FileSetPartitionId {
    FirstParty,
    ConfiguredTemplateDirectory(SourceRootId),
    InstalledApp(SourceRootId),
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
    pub fn id(&self) -> &FileSetPartitionId {
        &self.id
    }

    #[must_use]
    pub fn precedence(&self) -> u16 {
        self.precedence
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectFilePartitionReadiness {
    Loading,
    Ready {
        summary: FileSetSummary,
    },
    Deferred {
        issue: ProjectSourceFilesIssue,
        previous: Option<FileSetSummary>,
    },
    Skipped {
        issue: ProjectSourceFilesIssue,
        previous: Option<FileSetSummary>,
    },
    Unavailable {
        issue: ProjectSourceFilesIssue,
        previous: Option<FileSetSummary>,
    },
    Stale {
        previous: Option<FileSetSummary>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectFileSetPartitionSnapshot {
    partition: FileSetPartition,
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
    readiness: ProjectFilePartitionReadiness,
}

impl ProjectFileSetPartitionSnapshot {
    fn new(
        partition: FileSetPartition,
        roots: Vec<SourceRoot>,
        files: Vec<DiscoveredSourceFile>,
        readiness: ProjectFilePartitionReadiness,
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
pub(crate) struct ProjectFileSetPartitions {
    partitions: Vec<ProjectFileSetPartitionSnapshot>,
}

impl ProjectFileSetPartitions {
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    fn with_first_party(snapshot: ProjectFileSetPartitionSnapshot) -> Self {
        Self {
            partitions: vec![snapshot],
        }
    }

    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn first_party_readiness(&self) -> Option<&ProjectFilePartitionReadiness> {
        self.partitions
            .iter()
            .find(|partition| partition.partition.id() == &FileSetPartitionId::FirstParty)
            .map(|partition| &partition.readiness)
    }

    #[must_use]
    pub(crate) fn merged_discovered_data(&self) -> MergedDiscoveredSourceFileSetData {
        let roots = self
            .partitions
            .iter()
            .flat_map(|partition| partition.roots.iter().cloned())
            .map(SourceRootEntry::new)
            .collect::<Vec<_>>();
        let files = self
            .partitions
            .iter()
            .flat_map(|partition| partition.files.iter().cloned())
            .collect::<Vec<_>>();
        let summary = FileSetSummary::new(files.len());
        MergedDiscoveredSourceFileSetData {
            roots,
            files,
            summary,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirstPartySourceFilePatch {
    partition: FileSetPartition,
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
    issues: Vec<ProjectSourceFilesIssue>,
}

impl FirstPartySourceFilePatch {
    #[must_use]
    pub fn first_party(
        root_plan_issues: Vec<ProjectSourceFilesIssue>,
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

    #[must_use]
    pub fn summary(&self) -> FileSetSummary {
        self.summary
    }

    #[must_use]
    pub fn issues(&self) -> &[ProjectSourceFilesIssue] {
        &self.issues
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MergedDiscoveredSourceFileSetData {
    roots: Vec<SourceRootEntry>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
}

impl MergedDiscoveredSourceFileSetData {
    #[must_use]
    pub fn roots(&self) -> &[SourceRootEntry] {
        &self.roots
    }

    #[must_use]
    pub fn files(&self) -> &[DiscoveredSourceFile] {
        &self.files
    }

    #[must_use]
    pub fn summary(&self) -> FileSetSummary {
        self.summary
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSourceFilesMaterializationPatch {
    changed_roots: Vec<SourceRootEntry>,
    removed_roots: Vec<SourceRootId>,
    upserted_files: Vec<DiscoveredSourceFile>,
    removed_files: Vec<Utf8PathBuf>,
    summary: FileSetSummary,
}

impl ProjectSourceFilesMaterializationPatch {
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
pub struct ProjectFileLoadingTransition {
    partition: FileSetPartition,
    readiness: ProjectFilePartitionReadiness,
}

impl ProjectFileLoadingTransition {
    #[must_use]
    pub fn partition(&self) -> &FileSetPartition {
        &self.partition
    }

    #[must_use]
    pub fn readiness(&self) -> &ProjectFilePartitionReadiness {
        &self.readiness
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSourceFilesUpdate {
    partitions: ProjectFileSetPartitions,
    materialization: ProjectSourceFilesMaterializationPatch,
    applied_transition: ProjectFileLoadingTransition,
    issues: Vec<ProjectSourceFilesIssue>,
}

impl ProjectSourceFilesUpdate {
    #[must_use]
    pub fn materialization(&self) -> &ProjectSourceFilesMaterializationPatch {
        &self.materialization
    }

    #[must_use]
    pub fn applied_transition(&self) -> &ProjectFileLoadingTransition {
        &self.applied_transition
    }

    #[must_use]
    pub fn issues(&self) -> &[ProjectSourceFilesIssue] {
        &self.issues
    }

    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn partitions(&self) -> &ProjectFileSetPartitions {
        &self.partitions
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFileSetMaterialized {
    source_file_set: SourceFileSet,
    handle_changes: SourceFileHandleChanges,
    issues: Vec<SourceFileMaterializationIssue>,
}

impl SourceFileSetMaterialized {
    #[must_use]
    pub fn new(
        source_file_set: SourceFileSet,
        handle_changes: SourceFileHandleChanges,
        issues: Vec<SourceFileMaterializationIssue>,
    ) -> Self {
        Self {
            source_file_set,
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
pub enum ProjectSourceFilesApplyResult {
    Applied(ProjectSourceFilesApplied),
    Deferred {
        transition: ProjectFileLoadingTransition,
        issue: ProjectSourceFilesIssue,
        previous: Option<ReadyProjectSourceFiles>,
    },
    Unavailable {
        transition: ProjectFileLoadingTransition,
        issue: ProjectSourceFilesIssue,
        previous: Option<ReadyProjectSourceFiles>,
    },
    Failed {
        transition: ProjectFileLoadingTransition,
        issue: ProjectSourceFilesIssue,
        previous: Option<ReadyProjectSourceFiles>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSourceFilesApplied {
    files: ReadyProjectSourceFiles,
    transition: ProjectFileLoadingTransition,
    issues: Vec<ProjectSourceFilesIssue>,
}

impl ProjectSourceFilesApplied {
    #[must_use]
    pub fn files(&self) -> &ReadyProjectSourceFiles {
        &self.files
    }

    #[must_use]
    pub fn transition(&self) -> &ProjectFileLoadingTransition {
        &self.transition
    }

    #[must_use]
    pub fn issues(&self) -> &[ProjectSourceFilesIssue] {
        &self.issues
    }
}

pub fn finalize_project_source_files(
    db: &mut dyn Db,
    previous: Option<ReadyProjectSourceFiles>,
    update: ProjectSourceFilesUpdate,
    materialized: SourceFileSetMaterialized,
) -> ProjectSourceFilesApplyResult {
    if let Some(issue) = update.issues.first().cloned() {
        return terminal_source_files_apply_result(
            db,
            update.applied_transition,
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
        return terminal_source_files_apply_result(
            db,
            update.applied_transition,
            issue,
            previous,
            TerminalSourceFilesAvailability::Failed,
        );
    }

    let files = ReadyProjectSourceFiles::new(update.partitions, materialized.source_file_set);
    db.set_project_source_files_availability(ProjectSourceFilesAvailability::Ready(files.clone()));
    ProjectSourceFilesApplyResult::Applied(ProjectSourceFilesApplied {
        files,
        transition: update.applied_transition,
        issues: update.issues,
    })
}

#[allow(dead_code)]
enum TerminalSourceFilesAvailability {
    Deferred,
    Unavailable,
    Failed,
}

fn terminal_source_files_apply_result(
    db: &mut dyn Db,
    transition: ProjectFileLoadingTransition,
    issue: ProjectSourceFilesIssue,
    previous: Option<ReadyProjectSourceFiles>,
    availability: TerminalSourceFilesAvailability,
) -> ProjectSourceFilesApplyResult {
    match availability {
        TerminalSourceFilesAvailability::Deferred => {
            db.set_project_source_files_availability(ProjectSourceFilesAvailability::Deferred {
                issue: issue.clone(),
                previous: previous.clone(),
            });
            ProjectSourceFilesApplyResult::Deferred {
                transition,
                issue,
                previous,
            }
        }
        TerminalSourceFilesAvailability::Unavailable => {
            db.set_project_source_files_availability(ProjectSourceFilesAvailability::Unavailable {
                issue: issue.clone(),
                previous: previous.clone(),
            });
            ProjectSourceFilesApplyResult::Unavailable {
                transition,
                issue,
                previous,
            }
        }
        TerminalSourceFilesAvailability::Failed => {
            db.set_project_source_files_availability(ProjectSourceFilesAvailability::Failed {
                issue: issue.clone(),
                previous: previous.clone(),
            });
            ProjectSourceFilesApplyResult::Failed {
                transition,
                issue,
                previous,
            }
        }
    }
}

fn project_issue_from_materialization_issue(
    issue: &SourceFileMaterializationIssue,
) -> ProjectSourceFilesIssue {
    match issue {
        SourceFileMaterializationIssue::MissingRoot { root } => {
            ProjectSourceFilesIssue::MissingRoot {
                root: root.clone(),
                path: root.as_path().to_owned(),
            }
        }
        SourceFileMaterializationIssue::MaterializationFailed { path, error_kind } => {
            ProjectSourceFilesIssue::MaterializationFailed {
                path: path.clone(),
                error_kind: *error_kind,
            }
        }
    }
}

#[must_use]
pub fn merge_first_party_source_file_patch(
    current: Option<&ReadyProjectSourceFiles>,
    patch: FirstPartySourceFilePatch,
) -> ProjectSourceFilesUpdate {
    let readiness = first_party_readiness(current, &patch);
    let snapshot = ProjectFileSetPartitionSnapshot::new(
        patch.partition.clone(),
        patch.roots.clone(),
        patch.files.clone(),
        readiness.clone(),
    );
    let partitions = ProjectFileSetPartitions::with_first_party(snapshot);
    let merged = merged_first_party_data(&patch.roots, &patch.files);
    let previous = current.map(ReadyProjectSourceFiles::discovered_data);
    let materialization = materialization_patch(previous.as_ref(), &merged);
    let applied_transition = ProjectFileLoadingTransition {
        partition: patch.partition,
        readiness,
    };

    ProjectSourceFilesUpdate {
        partitions,
        materialization,
        applied_transition,
        issues: patch.issues,
    }
}

fn first_party_readiness(
    current: Option<&ReadyProjectSourceFiles>,
    patch: &FirstPartySourceFilePatch,
) -> ProjectFilePartitionReadiness {
    if let Some(issue) = patch.issues.first() {
        return ProjectFilePartitionReadiness::Unavailable {
            issue: issue.clone(),
            previous: current.map(|files| files.discovered_data().summary()),
        };
    }

    ProjectFilePartitionReadiness::Ready {
        summary: patch.summary,
    }
}

pub(crate) fn merged_first_party_data(
    roots: &[SourceRoot],
    files: &[DiscoveredSourceFile],
) -> MergedDiscoveredSourceFileSetData {
    let roots = roots
        .iter()
        .cloned()
        .map(SourceRootEntry::new)
        .collect::<Vec<_>>();
    let files = files.to_vec();
    let summary = FileSetSummary::new(files.len());
    MergedDiscoveredSourceFileSetData {
        roots,
        files,
        summary,
    }
}

fn materialization_patch(
    previous: Option<&MergedDiscoveredSourceFileSetData>,
    merged: &MergedDiscoveredSourceFileSetData,
) -> ProjectSourceFilesMaterializationPatch {
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

    ProjectSourceFilesMaterializationPatch {
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
        let owned = DiscoveredSourceFile::new(file.path().to_owned(), owner.id().clone());
        by_path.insert(file.path().to_owned(), owned);
    }
    by_path.into_values().collect()
}

fn longest_prefix_root<'a>(path: &Utf8Path, roots: &'a [SourceRoot]) -> Option<&'a SourceRoot> {
    roots
        .iter()
        .filter(|root| path.starts_with(root.path()))
        .max_by_key(|root| root.path().as_str().len())
}

fn project_issue_from_workspace_issue(issue: &WorkspaceRootIssue) -> ProjectSourceFilesIssue {
    match issue {
        WorkspaceRootIssue::MissingRoot { root, path } => ProjectSourceFilesIssue::MissingRoot {
            root: root.clone(),
            path: path.clone(),
        },
        WorkspaceRootIssue::UnreadableRoot {
            root,
            path,
            error_kind,
        } => ProjectSourceFilesIssue::WalkFailed {
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
    ) -> (Vec<ProjectSourceFilesIssue>, FilesForRootsResult) {
        let (root_issues, request) =
            first_party_discovery_files_request(first_party_source_files_load_request(plan));
        (root_issues, load_files_for_roots(request))
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
            &[ProjectSourceFilesIssue::DuplicateRoot {
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
            &[ProjectSourceFilesIssue::DuplicateRoot {
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
            &[ProjectSourceFilesIssue::DuplicateRoot {
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
            &[ProjectSourceFilesIssue::MissingRoot {
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
        let previous_partition = ProjectFileSetPartitionSnapshot::new(
            FileSetPartition::first_party(),
            vec![removed.clone()],
            vec![previous_file],
            ProjectFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            },
        );
        let previous = ReadyProjectSourceFiles::materialized_for_test(
            ProjectFileSetPartitions::with_first_party(previous_partition),
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
        let previous_partition = ProjectFileSetPartitionSnapshot::new(
            FileSetPartition::first_party(),
            vec![kept.clone(), removed.clone()],
            vec![kept_file.clone()],
            ProjectFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            },
        );
        let previous = ReadyProjectSourceFiles::materialized_for_test(
            ProjectFileSetPartitions::with_first_party(previous_partition),
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
            &ProjectFilePartitionReadiness::Unavailable {
                issue: ProjectSourceFilesIssue::MissingRoot {
                    root: missing.id().clone(),
                    path: missing.path().to_owned(),
                },
                previous: None,
            }
        );
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
            update.applied_transition().partition().id(),
            &FileSetPartitionId::FirstParty
        );
        assert_eq!(
            update.applied_transition().readiness(),
            &ProjectFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            }
        );
        assert_eq!(
            update.partitions().first_party_readiness(),
            Some(&ProjectFilePartitionReadiness::Ready {
                summary: FileSetSummary::new(1),
            })
        );
    }
}
