//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_db::DjangoDatabase;
use djls_ide::PrimedTemplateLibraries;
use djls_project::Db as ProjectDb;
use djls_source::ChangeEvent;
use djls_source::Db as _;
use djls_source::File;
use djls_source::FileKind;
use djls_source::FileStatus;
use djls_source::Offset;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_source::path_to_file;
use tokio::sync::watch;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::InitializeParamsExt;
use crate::ext::PositionExt;
use crate::ext::TextDocumentContentChangeEventExt;
use crate::ext::TextDocumentItemExt;
use crate::ext::UriExt;
use crate::workspace::Workspace;

/// How many times snapshot-based reads retry after Salsa cancellation before
/// giving up and returning a fallback.
pub(crate) const SNAPSHOT_CANCEL_RETRIES: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CancellationRetryAction {
    Retry { attempt: usize },
    Exhausted,
}

pub(crate) struct CancellationRetryState {
    retries_remaining: usize,
}

impl CancellationRetryState {
    pub(crate) const fn new() -> Self {
        Self {
            retries_remaining: SNAPSHOT_CANCEL_RETRIES,
        }
    }

    pub(crate) const fn after_cancellation(&mut self) -> CancellationRetryAction {
        if self.retries_remaining == 0 {
            CancellationRetryAction::Exhausted
        } else {
            let attempt = SNAPSHOT_CANCEL_RETRIES - self.retries_remaining + 1;
            self.retries_remaining -= 1;
            CancellationRetryAction::Retry { attempt }
        }
    }
}

pub(crate) type IntrinsicGeneration = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectWork {
    Reprime,
    FullReload,
}

#[must_use = "document mutations can require project work to restore readiness"]
pub(crate) enum DocumentMutation {
    Ignored,
    Applied {
        document: TextDocument,
        project_work: Option<ProjectWork>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicReadinessState {
    ReadyWithoutProject,
    Unready(IntrinsicGeneration),
    Ready(IntrinsicGeneration),
    Failed(IntrinsicGeneration),
}

struct IntrinsicCoverage {
    reprime_files: Arc<[File]>,
    full_reload_files: Arc<[File]>,
}

enum IntrinsicReadiness {
    NoProject {
        generation: IntrinsicGeneration,
    },
    FullDiscovery {
        generation: IntrinsicGeneration,
    },
    Reprime {
        generation: IntrinsicGeneration,
        coverage: IntrinsicCoverage,
    },
    RetryReprime {
        generation: IntrinsicGeneration,
        coverage: IntrinsicCoverage,
        admitted_revisions: Arc<[(File, u64)]>,
    },
    Ready {
        generation: IntrinsicGeneration,
        coverage: IntrinsicCoverage,
    },
    FailedFullDiscovery {
        generation: IntrinsicGeneration,
    },
    FailedReprime {
        generation: IntrinsicGeneration,
        coverage: IntrinsicCoverage,
        admitted_revisions: Arc<[(File, u64)]>,
    },
}

impl IntrinsicReadiness {
    const fn new(has_project: bool) -> Self {
        if has_project {
            Self::FullDiscovery { generation: 0 }
        } else {
            Self::NoProject { generation: 0 }
        }
    }

    const fn desired_generation(&self) -> IntrinsicGeneration {
        match self {
            Self::NoProject { generation }
            | Self::FullDiscovery { generation }
            | Self::Reprime { generation, .. }
            | Self::RetryReprime { generation, .. }
            | Self::Ready { generation, .. }
            | Self::FailedFullDiscovery { generation }
            | Self::FailedReprime { generation, .. } => *generation,
        }
    }

    const fn watched_state(&self) -> IntrinsicReadinessState {
        match self {
            Self::NoProject { .. } => IntrinsicReadinessState::ReadyWithoutProject,
            Self::FullDiscovery { generation }
            | Self::Reprime { generation, .. }
            | Self::RetryReprime { generation, .. } => {
                IntrinsicReadinessState::Unready(*generation)
            }
            Self::Ready { generation, .. } => IntrinsicReadinessState::Ready(*generation),
            Self::FailedFullDiscovery { generation } | Self::FailedReprime { generation, .. } => {
                IntrinsicReadinessState::Failed(*generation)
            }
        }
    }

    const fn ready_generation(&self) -> Option<IntrinsicGeneration> {
        match self {
            Self::Ready { generation, .. } => Some(*generation),
            Self::NoProject { .. }
            | Self::FullDiscovery { .. }
            | Self::Reprime { .. }
            | Self::RetryReprime { .. }
            | Self::FailedFullDiscovery { .. }
            | Self::FailedReprime { .. } => None,
        }
    }

    const fn coverage(&self) -> Option<&IntrinsicCoverage> {
        match self {
            Self::Reprime { coverage, .. }
            | Self::RetryReprime { coverage, .. }
            | Self::Ready { coverage, .. }
            | Self::FailedReprime { coverage, .. } => Some(coverage),
            Self::NoProject { .. }
            | Self::FullDiscovery { .. }
            | Self::FailedFullDiscovery { .. } => None,
        }
    }

    fn mark_project_changed(&mut self, has_project: bool) -> IntrinsicGeneration {
        let generation = self.desired_generation();
        *self = if has_project {
            Self::FullDiscovery {
                generation: generation + 1,
            }
        } else {
            Self::NoProject { generation }
        };
        self.desired_generation()
    }

    fn begin_full_discovery(&mut self) -> IntrinsicGeneration {
        let generation = self.desired_generation() + 1;
        *self = Self::FullDiscovery { generation };
        generation
    }

    fn begin_reprime(&mut self, file: File, revision: u64) -> bool {
        let generation = self.desired_generation() + 1;
        let next = match self {
            Self::Reprime { coverage, .. } | Self::Ready { coverage, .. } => Self::Reprime {
                generation,
                coverage: IntrinsicCoverage {
                    reprime_files: Arc::clone(&coverage.reprime_files),
                    full_reload_files: Arc::clone(&coverage.full_reload_files),
                },
            },
            Self::RetryReprime {
                coverage,
                admitted_revisions,
                ..
            }
            | Self::FailedReprime {
                coverage,
                admitted_revisions,
                ..
            } => {
                let mut admitted_revisions = Arc::clone(admitted_revisions);
                let Some((_, admitted_revision)) = Arc::make_mut(&mut admitted_revisions)
                    .iter_mut()
                    .find(|(admitted_file, _)| *admitted_file == file)
                else {
                    return false;
                };
                if revision == *admitted_revision {
                    return false;
                }
                *admitted_revision = revision;
                Self::RetryReprime {
                    generation,
                    coverage: IntrinsicCoverage {
                        reprime_files: Arc::clone(&coverage.reprime_files),
                        full_reload_files: Arc::clone(&coverage.full_reload_files),
                    },
                    admitted_revisions,
                }
            }
            Self::NoProject { .. }
            | Self::FullDiscovery { .. }
            | Self::FailedFullDiscovery { .. } => return false,
        };
        *self = next;
        true
    }

    fn publish(&mut self, generation: IntrinsicGeneration, coverage: IntrinsicCoverage) -> bool {
        let accepts_generation = match self {
            Self::FullDiscovery {
                generation: current,
            }
            | Self::Reprime {
                generation: current,
                ..
            }
            | Self::RetryReprime {
                generation: current,
                ..
            } => *current == generation,
            Self::NoProject { .. }
            | Self::Ready { .. }
            | Self::FailedFullDiscovery { .. }
            | Self::FailedReprime { .. } => false,
        };
        if !accepts_generation {
            return false;
        }

        *self = Self::Ready {
            generation,
            coverage,
        };
        true
    }

    fn fail(
        &mut self,
        generation: IntrinsicGeneration,
        revisions: impl FnOnce(&IntrinsicCoverage) -> Arc<[(File, u64)]>,
    ) -> bool {
        let next = match self {
            Self::FullDiscovery {
                generation: current,
            } if *current == generation => Self::FailedFullDiscovery { generation },
            Self::Reprime {
                generation: current,
                coverage,
            } if *current == generation => Self::FailedReprime {
                generation,
                admitted_revisions: revisions(coverage),
                coverage: IntrinsicCoverage {
                    reprime_files: Arc::clone(&coverage.reprime_files),
                    full_reload_files: Arc::clone(&coverage.full_reload_files),
                },
            },
            Self::RetryReprime {
                generation: current,
                coverage,
                admitted_revisions,
            } if *current == generation => Self::FailedReprime {
                generation,
                coverage: IntrinsicCoverage {
                    reprime_files: Arc::clone(&coverage.reprime_files),
                    full_reload_files: Arc::clone(&coverage.full_reload_files),
                },
                admitted_revisions: Arc::clone(admitted_revisions),
            },
            Self::NoProject { .. }
            | Self::FullDiscovery { .. }
            | Self::Reprime { .. }
            | Self::RetryReprime { .. }
            | Self::Ready { .. }
            | Self::FailedFullDiscovery { .. }
            | Self::FailedReprime { .. } => return false,
        };
        *self = next;
        true
    }

    #[cfg(test)]
    fn admitted_revisions(&self) -> Option<&[(File, u64)]> {
        match self {
            Self::RetryReprime {
                admitted_revisions, ..
            }
            | Self::FailedReprime {
                admitted_revisions, ..
            } => Some(admitted_revisions),
            Self::NoProject { .. }
            | Self::FullDiscovery { .. }
            | Self::Reprime { .. }
            | Self::Ready { .. }
            | Self::FailedFullDiscovery { .. } => None,
        }
    }
}

/// LSP Session managing project-specific state and database operations.
///
/// The Session serves as the main entry point for LSP operations, managing:
/// - The Salsa database for incremental computation
/// - Client capabilities and position encoding
/// - Workspace operations (buffers and file system)
/// - All Salsa inputs (`SessionState`, Project)
///
/// Following Ruff's architecture, the concrete database lives at this level
/// and is passed down to operations that need it.
pub(crate) struct Session {
    /// Workspace for buffer and file system management
    ///
    /// This manages document buffers and file system abstraction,
    /// but not the database (which is owned directly by Session).
    workspace: Workspace,

    client_info: ClientInfo,

    /// The Salsa database for incremental computation
    db: DjangoDatabase,

    intrinsic_readiness: IntrinsicReadiness,
    readiness_tx: watch::Sender<IntrinsicReadinessState>,
}

impl Session {
    #[must_use]
    pub(crate) fn new(params: &ls_types::InitializeParams) -> Self {
        let project_path = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| folder.uri.to_utf8_path_buf())
            .or_else(|| {
                // Fall back to current directory
                std::env::current_dir()
                    .ok()
                    .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            });

        let client_options = params.client_options();

        let client_settings = client_options.settings.clone();

        let workspace = Workspace::new();
        let db = DjangoDatabase::new(
            workspace.overlay(),
            &client_settings,
            project_path.as_deref(),
        );

        let client_info = ClientInfo::new(
            &params.capabilities,
            params.client_info.as_ref(),
            client_options,
        );

        let intrinsic_readiness = IntrinsicReadiness::new(db.project().is_some());
        let (readiness_tx, _readiness_rx) = watch::channel(intrinsic_readiness.watched_state());

        Self {
            workspace,
            client_info,
            db,
            intrinsic_readiness,
            readiness_tx,
        }
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot::new(
            self.db.clone(),
            self.client_info.clone(),
            self.ready_generation(),
        )
    }

    pub(crate) fn readiness_receiver(&self) -> watch::Receiver<IntrinsicReadinessState> {
        self.readiness_tx.subscribe()
    }

    pub(crate) const fn readiness_state(&self) -> IntrinsicReadinessState {
        self.intrinsic_readiness.watched_state()
    }

    pub(crate) const fn desired_generation(&self) -> IntrinsicGeneration {
        self.intrinsic_readiness.desired_generation()
    }

    pub(crate) fn mark_project_changed(&mut self) -> IntrinsicGeneration {
        let generation = self
            .intrinsic_readiness
            .mark_project_changed(self.db.project().is_some());
        self.publish_readiness();
        generation
    }

    pub(crate) fn publish_intrinsic_readiness(
        &mut self,
        generation: IntrinsicGeneration,
        primed: &PrimedTemplateLibraries,
    ) -> bool {
        let published = self.intrinsic_readiness.publish(
            generation,
            IntrinsicCoverage {
                reprime_files: primed.reprime_files().into(),
                full_reload_files: primed.full_reload_files().into(),
            },
        );
        if published {
            self.publish_readiness();
        }
        published
    }

    pub(crate) fn fail_intrinsic_readiness(&mut self, generation: IntrinsicGeneration) -> bool {
        let db = &self.db;
        let failed = self.intrinsic_readiness.fail(generation, |coverage| {
            coverage
                .reprime_files
                .iter()
                .map(|file| (*file, file.revision(db)))
                .collect::<Vec<_>>()
                .into()
        });
        if failed {
            self.publish_readiness();
        }
        failed
    }

    #[cfg(test)]
    pub(crate) fn install_ready_coverage_for_test(
        &mut self,
        reprime_files: impl Into<Arc<[File]>>,
        full_reload_files: impl Into<Arc<[File]>>,
    ) {
        self.intrinsic_readiness = IntrinsicReadiness::Ready {
            generation: self.desired_generation(),
            coverage: IntrinsicCoverage {
                reprime_files: reprime_files.into(),
                full_reload_files: full_reload_files.into(),
            },
        };
        self.publish_readiness();
    }

    fn ready_generation(&self) -> Option<IntrinsicGeneration> {
        self.intrinsic_readiness.ready_generation()
    }

    fn publish_readiness(&self) {
        self.readiness_tx
            .send_replace(self.intrinsic_readiness.watched_state());
    }

    fn mark_intrinsic_change(
        &mut self,
        change: &ChangeEvent,
        kind: FileKind,
    ) -> Option<ProjectWork> {
        if self.db.project().is_none() || kind != FileKind::Python {
            return None;
        }

        let path = match change {
            ChangeEvent::Opened(path)
            | ChangeEvent::BecameVisible(path)
            | ChangeEvent::ContentChanged(path)
            | ChangeEvent::Deleted(path) => path,
            ChangeEvent::Rescan => return None,
        };
        let changed_membership = matches!(
            change,
            ChangeEvent::BecameVisible(_) | ChangeEvent::Deleted(_)
        );

        let coverage = self.intrinsic_readiness.coverage();
        let reprime_file = coverage
            .and_then(|coverage| {
                coverage
                    .reprime_files
                    .iter()
                    .find(|file| file.path(&self.db) == path)
            })
            .copied();
        let work = if changed_membership
            || coverage.is_some_and(|coverage| {
                coverage
                    .full_reload_files
                    .iter()
                    .any(|file| file.path(&self.db) == path)
            }) {
            ProjectWork::FullReload
        } else if reprime_file.is_some() {
            ProjectWork::Reprime
        } else if coverage.is_none() {
            // Until current coverage publishes, every Python source is a
            // possible settings or catalog dependency. Full discovery is the
            // only operation that can safely classify it.
            ProjectWork::FullReload
        } else {
            return None;
        };

        match work {
            ProjectWork::Reprime => {
                let reprime_file = reprime_file?;
                let revision = reprime_file.revision(&self.db);
                if !self
                    .intrinsic_readiness
                    .begin_reprime(reprime_file, revision)
                {
                    return None;
                }
            }
            ProjectWork::FullReload => {
                self.intrinsic_readiness.begin_full_discovery();
            }
        }
        self.publish_readiness();
        Some(work)
    }

    pub(crate) fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub(crate) fn db_mut(&mut self) -> &mut DjangoDatabase {
        &mut self.db
    }

    /// Open a document in the session.
    ///
    /// Updates the workspace buffer first, then applies the project-visible
    /// file event against the overlay-backed database.
    pub(crate) fn open_document(
        &mut self,
        text_document: &ls_types::TextDocumentItem,
    ) -> DocumentMutation {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip opening non-file URI: {}", text_document.uri.as_str());
            return DocumentMutation::Ignored;
        };

        let kind = text_document.language_id_to_file_kind(self.client_info.client());
        let change = self.open_document_change(&path);
        let document =
            self.workspace
                .open_document(&path, &text_document.text, text_document.version, kind);
        SourceChanges::new([change.clone()]).apply(&mut self.db);
        let project_work = self.mark_intrinsic_change(&change, kind);
        DocumentMutation::Applied {
            document,
            project_work,
        }
    }

    pub(crate) fn save_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> DocumentMutation {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip saving non-file URI: {}", text_document.uri.as_str());
            return DocumentMutation::Ignored;
        };

        let Some(document) = self.workspace.save_document(&path) else {
            return DocumentMutation::Ignored;
        };
        let change = ChangeEvent::ContentChanged(path);
        SourceChanges::new([change.clone()]).apply(&mut self.db);
        let project_work = self.mark_intrinsic_change(&change, document.kind());
        DocumentMutation::Applied {
            document,
            project_work,
        }
    }

    pub(crate) fn update_document(
        &mut self,
        text_document: &ls_types::VersionedTextDocumentIdentifier,
        changes: Vec<ls_types::TextDocumentContentChangeEvent>,
    ) -> DocumentMutation {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip updating non-file URI: {}", text_document.uri.as_str());
            return DocumentMutation::Ignored;
        };

        let change = if self.workspace.get_document(&path).is_some() {
            ChangeEvent::ContentChanged(path.clone())
        } else {
            self.open_document_change(&path)
        };
        let Some(document) = self.workspace.update_document(
            &path,
            changes.to_document_changes(),
            text_document.version,
            self.client_info.position_encoding(),
        ) else {
            return DocumentMutation::Ignored;
        };
        SourceChanges::new([change.clone()]).apply(&mut self.db);
        let project_work = self.mark_intrinsic_change(&change, document.kind());
        DocumentMutation::Applied {
            document,
            project_work,
        }
    }

    /// Close a document.
    ///
    /// Removes the document from workspace buffers, invalidates cached source state,
    /// and lets future reads fall back to disk.
    pub(crate) fn close_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> DocumentMutation {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip closing non-file URI: {}", text_document.uri.as_str());
            return DocumentMutation::Ignored;
        };

        let change = self.close_document_change(&path);
        let Some(document) = self.workspace.close_document(&path) else {
            return DocumentMutation::Ignored;
        };
        SourceChanges::new([change.clone()]).apply(&mut self.db);
        let project_work = self.mark_intrinsic_change(&change, document.kind());

        DocumentMutation::Applied {
            document,
            project_work,
        }
    }

    fn open_document_change(&self, path: &Utf8Path) -> ChangeEvent {
        if !self.workspace.disk_is_file(path) {
            return ChangeEvent::BecameVisible(path.to_path_buf());
        }

        match self
            .db
            .files()
            .try_file(path)
            .map(|file| file.status(&self.db))
        {
            Some(FileStatus::Exists) => ChangeEvent::Opened(path.to_path_buf()),
            Some(FileStatus::IsADirectory | FileStatus::NotFound) | None => {
                ChangeEvent::BecameVisible(path.to_path_buf())
            }
        }
    }

    fn close_document_change(&self, path: &Utf8Path) -> ChangeEvent {
        if self.workspace.disk_is_file(path) {
            ChangeEvent::ContentChanged(path.to_path_buf())
        } else {
            ChangeEvent::Deleted(path.to_path_buf())
        }
    }

    /// Get a document from the buffer if it's open.
    #[cfg(test)]
    fn get_document(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.workspace.get_document(path)
    }

    /// Get all currently open documents.
    pub(crate) fn open_documents(&self) -> Vec<TextDocument> {
        self.workspace.open_documents()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(&ls_types::InitializeParams::default())
    }
}

/// Immutable snapshot of session state.
#[derive(Clone)]
pub(crate) struct SessionSnapshot {
    db: DjangoDatabase,
    client_info: ClientInfo,
    intrinsic_generation: Option<IntrinsicGeneration>,
}

impl SessionSnapshot {
    fn new(
        db: DjangoDatabase,
        client_info: ClientInfo,
        intrinsic_generation: Option<IntrinsicGeneration>,
    ) -> Self {
        Self {
            db,
            client_info,
            intrinsic_generation,
        }
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub(crate) fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    pub(crate) const fn intrinsic_generation(&self) -> Option<IntrinsicGeneration> {
        self.intrinsic_generation
    }

    /// Resolve an LSP document request to the tracked file for that URI.
    ///
    /// Open editor buffers are exposed to Salsa through the workspace overlay,
    /// so feature code should read current text through [`File::source`]
    /// instead of reaching back into [`TextDocument`] state.
    pub(crate) fn file_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        request: &str,
    ) -> Option<File> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!(
                "Skipping non-file URI in {} request: {}",
                request,
                text_document.uri.as_str()
            );
            return None;
        };

        path_to_file(&self.db, &path).ok()
    }

    /// Resolve an LSP positioned document request to a tracked file and byte offset.
    pub(crate) fn position_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        position: ls_types::Position,
        request: &str,
    ) -> Option<(File, Offset)> {
        let file = self.file_for_document_request(text_document, request)?;
        let source = file.try_source(&self.db).ok()?;
        let line_index = file.line_index(&self.db);
        let offset = position.to_offset(
            source.as_str(),
            line_index,
            self.client_info.position_encoding(),
        );

        Some((file, offset))
    }

    /// Resolve an LSP ranged document request to a tracked file and byte span.
    pub(crate) fn range_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        range: ls_types::Range,
        request: &str,
    ) -> Option<(File, Span)> {
        let file = self.file_for_document_request(text_document, request)?;
        let source = file.try_source(&self.db).ok()?;
        let line_index = file.line_index(&self.db);
        let start = range.start.to_offset(
            source.as_str(),
            line_index,
            self.client_info.position_encoding(),
        );
        let end = range.end.to_offset(
            source.as_str(),
            line_index,
            self.client_info.position_encoding(),
        );
        let span = Span::saturating_from_bounds_usize(start.get() as usize, end.get() as usize);

        Some((file, span))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::time::Duration;

    use djls_ide::prime_template_library_products;
    use djls_project::Db as ProjectDb;
    use djls_project::Interpreter;
    use tempfile::tempdir;
    use tokio::spawn;
    use tokio::task::yield_now;
    use tokio::time::timeout;

    use super::*;

    // Helper function to create a test file path and URI that works on all platforms
    fn test_file_uri(filename: &str) -> (Utf8PathBuf, ls_types::Uri) {
        // Use an absolute path that's valid on the platform
        #[cfg(windows)]
        let path = Utf8PathBuf::from(format!("C:\\temp\\{filename}"));
        #[cfg(not(windows))]
        let path = Utf8PathBuf::from(format!("/tmp/{filename}"));

        let uri =
            ls_types::Uri::from_file_path(path.as_std_path()).expect("Failed to create file URI");
        (path, uri)
    }

    #[test]
    fn test_session_document_lifecycle() {
        let mut session = Session::default();
        let non_file_uri = ls_types::Uri::from_str("untitled:Untitled-1").expect("valid URI");
        let non_file_identifier = ls_types::TextDocumentIdentifier {
            uri: non_file_uri.clone(),
        };
        assert!(matches!(
            session.open_document(&ls_types::TextDocumentItem {
                uri: non_file_uri.clone(),
                language_id: "python".to_string(),
                version: 1,
                text: String::new(),
            }),
            DocumentMutation::Ignored
        ));
        assert!(matches!(
            session.save_document(&non_file_identifier),
            DocumentMutation::Ignored
        ));
        assert!(matches!(
            session.update_document(
                &ls_types::VersionedTextDocumentIdentifier {
                    uri: non_file_uri,
                    version: 2,
                },
                Vec::new(),
            ),
            DocumentMutation::Ignored
        ));
        assert!(matches!(
            session.close_document(&non_file_identifier),
            DocumentMutation::Ignored
        ));
        let (_, missing_uri) = test_file_uri("missing.py");
        let missing_identifier = ls_types::TextDocumentIdentifier {
            uri: missing_uri.clone(),
        };
        assert!(matches!(
            session.save_document(&missing_identifier),
            DocumentMutation::Ignored
        ));
        assert!(matches!(
            session.update_document(
                &ls_types::VersionedTextDocumentIdentifier {
                    uri: missing_uri,
                    version: 1,
                },
                Vec::new(),
            ),
            DocumentMutation::Ignored
        ));
        assert!(matches!(
            session.close_document(&missing_identifier),
            DocumentMutation::Ignored
        ));

        let (template_path, template_uri) = test_file_uri("test.html");
        let (template, project_work) = match session.open_document(&ls_types::TextDocumentItem {
            uri: template_uri,
            language_id: "django-html".to_string(),
            version: 1,
            text: String::new(),
        }) {
            DocumentMutation::Applied {
                document,
                project_work,
            } => Some((document, project_work)),
            DocumentMutation::Ignored => None,
        }
        .expect("template test document should open");
        assert_eq!(template.path(), template_path);
        assert_eq!(project_work, None);

        let (path, uri) = test_file_uri("test.py");
        let text_document = ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "print('hello')".to_string(),
        };
        let opened = match session.open_document(&text_document) {
            DocumentMutation::Applied { document, .. } => Some(document),
            DocumentMutation::Ignored => None,
        }
        .expect("Python test document should open");

        assert_eq!(opened.path(), path);
        assert!(session.get_document(&path).is_some());

        let db = session.db();
        let file = path_to_file(db, &path).expect("open buffer should be visible to the overlay");
        let content = file
            .try_source(db)
            .expect("open buffer should be readable")
            .to_string();
        assert_eq!(content, "print('hello')");

        let close_doc = ls_types::TextDocumentIdentifier { uri };
        let closed = match session.close_document(&close_doc) {
            DocumentMutation::Applied { document, .. } => Some(document),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python test document should close");
        assert_eq!(closed.path(), path);
        assert!(session.get_document(&path).is_none());
    }

    #[test]
    fn test_session_document_update() {
        let mut session = Session::default();
        let (path, uri) = test_file_uri("test.py");

        let text_document = ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "initial".to_string(),
        };
        match session.open_document(&text_document) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("Python test document should open before update");

        let changes = vec![ls_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "updated".to_string(),
        }];
        let versioned_document = ls_types::VersionedTextDocumentIdentifier { uri, version: 2 };
        let updated = match session.update_document(&versioned_document, changes) {
            DocumentMutation::Applied { document, .. } => Some(document),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python test document should update");

        assert_eq!(updated.path(), path);
        let doc = session
            .get_document(&path)
            .expect("updated document should remain open");
        assert_eq!(doc.content(), "updated");
        assert_eq!(doc.version(), 2);

        let db = session.db();
        let file = path_to_file(db, &path).expect("open buffer should be visible to the overlay");
        let content = file
            .try_source(db)
            .expect("open buffer should be readable")
            .to_string();
        assert_eq!(content, "updated");
    }

    #[test]
    fn document_mutations_return_work_when_they_stale_readiness() {
        let mut session = Session::default();
        let path = Utf8Path::new("/tmp/mutation-outcome.py");
        let uri = ls_types::Uri::from_file_path(path.as_std_path())
            .expect("mutation test path should convert to a file URI");
        let (document, project_work) = match session.open_document(&ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: String::new(),
        }) {
            DocumentMutation::Applied {
                document,
                project_work,
            } => Some((document, project_work)),
            DocumentMutation::Ignored => None,
        }
        .expect("Python mutation test document should open");
        assert_eq!(document.path(), path);
        assert_eq!(project_work, Some(ProjectWork::FullReload));

        let file = path_to_file(session.db(), path)
            .expect("open mutation test document should be interned");
        session.install_ready_coverage_for_test(vec![file], Vec::new());

        let (document, project_work) =
            match session.save_document(&ls_types::TextDocumentIdentifier { uri: uri.clone() }) {
                DocumentMutation::Applied {
                    document,
                    project_work,
                } => Some((document, project_work)),
                DocumentMutation::Ignored => None,
            }
            .expect("open Python mutation test document should save");
        assert_eq!(document.path(), path);
        assert_eq!(project_work, Some(ProjectWork::Reprime));

        session.install_ready_coverage_for_test(vec![file], Vec::new());
        let (document, project_work) = match session.update_document(
            &ls_types::VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "changed".to_string(),
            }],
        ) {
            DocumentMutation::Applied {
                document,
                project_work,
            } => Some((document, project_work)),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python mutation test document should update");
        assert_eq!(document.path(), path);
        assert_eq!(project_work, Some(ProjectWork::Reprime));

        session.install_ready_coverage_for_test(vec![file], Vec::new());
        let (document, project_work) =
            match session.close_document(&ls_types::TextDocumentIdentifier { uri }) {
                DocumentMutation::Applied {
                    document,
                    project_work,
                } => Some((document, project_work)),
                DocumentMutation::Ignored => None,
            }
            .expect("open Python mutation test document should close");
        assert_eq!(document.path(), path);
        assert_eq!(project_work, Some(ProjectWork::FullReload));
    }

    #[test]
    fn test_snapshot_creation() {
        let session = Session::default();
        let snapshot = session.snapshot();

        assert_eq!(
            session.client_info().position_encoding(),
            snapshot.client_info().position_encoding()
        );
        assert_eq!(
            session.db().project().is_some(),
            snapshot.db().project().is_some()
        );
    }

    #[test]
    fn no_project_readiness_stays_ready_without_advancing_generation() {
        let mut readiness = IntrinsicReadiness::new(false);

        assert_eq!(readiness.desired_generation(), 0);
        assert_eq!(
            readiness.watched_state(),
            IntrinsicReadinessState::ReadyWithoutProject
        );
        assert_eq!(readiness.ready_generation(), None);
        assert_eq!(readiness.mark_project_changed(false), 0);
        assert_eq!(readiness.desired_generation(), 0);
        assert_eq!(
            readiness.watched_state(),
            IntrinsicReadinessState::ReadyWithoutProject
        );
        assert!(readiness.coverage().is_none());
        assert!(readiness.admitted_revisions().is_none());
    }

    #[test]
    fn cancellation_retries_reach_a_stable_terminal_state() {
        let mut retry_state = CancellationRetryState::new();

        for attempt in 1..=SNAPSHOT_CANCEL_RETRIES {
            assert_eq!(
                retry_state.after_cancellation(),
                CancellationRetryAction::Retry { attempt }
            );
        }
        assert_eq!(
            retry_state.after_cancellation(),
            CancellationRetryAction::Exhausted
        );
        assert_eq!(
            retry_state.after_cancellation(),
            CancellationRetryAction::Exhausted
        );
    }

    #[test]
    fn begin_reprime_rejects_states_without_complete_coverage() {
        let session = Session::default();
        let file = File::new(
            session.db(),
            Utf8PathBuf::from("/tmp/invalid-reprime.py"),
            1,
            FileStatus::Exists,
        );
        let invalid_states = [
            (
                IntrinsicReadiness::NoProject { generation: 3 },
                IntrinsicReadinessState::ReadyWithoutProject,
            ),
            (
                IntrinsicReadiness::FullDiscovery { generation: 4 },
                IntrinsicReadinessState::Unready(4),
            ),
            (
                IntrinsicReadiness::FailedFullDiscovery { generation: 5 },
                IntrinsicReadinessState::Failed(5),
            ),
        ];
        for (mut readiness, expected_state) in invalid_states {
            assert!(!readiness.begin_reprime(file, 1));
            assert_eq!(readiness.watched_state(), expected_state);
        }
    }

    #[test]
    fn intrinsic_readiness_is_generation_scoped_and_stale_results_do_not_publish() {
        let mut session = Session::default();
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(0)
        );
        let stale_prime =
            prime_template_library_products(session.db()).expect("default session has a Project");

        let generation = session.mark_project_changed();
        assert_eq!(generation, 1);
        assert!(!session.publish_intrinsic_readiness(0, &stale_prime));
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(1)
        );

        let current_prime =
            prime_template_library_products(session.db()).expect("default session has a Project");
        assert!(session.publish_intrinsic_readiness(generation, &current_prime));
        assert_eq!(session.readiness_state(), IntrinsicReadinessState::Ready(1));
        assert_eq!(session.snapshot().intrinsic_generation(), Some(1));
    }

    #[test]
    fn python_edits_before_coverage_are_conservative_but_template_edits_never_stale() {
        let mut session = Session::default();
        let readiness = session.readiness_receiver();
        let project_work = session.mark_intrinsic_change(
            &ChangeEvent::ContentChanged("/tmp/unrelated.py".into()),
            FileKind::Python,
        );
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(1)
        );
        assert_eq!(project_work, Some(ProjectWork::FullReload));
        assert!(
            readiness
                .has_changed()
                .expect("readiness sender should remain live")
        );

        let generation = session.desired_generation();
        let project_work = session.mark_intrinsic_change(
            &ChangeEvent::ContentChanged("/tmp/page.html".into()),
            FileKind::Template,
        );
        assert_eq!(session.desired_generation(), generation);
        assert_eq!(project_work, None);
    }

    #[test]
    fn final_state_matrix_09_only_covered_python_edits_stale_readiness() {
        let mut session = Session::default();
        let covered_path = Utf8Path::new("/tmp/covered.py");
        let covered_document = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_file_path(covered_path.as_std_path())
                .expect("covered Python path should convert to a file URI"),
            language_id: "python".to_string(),
            version: 1,
            text: String::new(),
        };
        match session.open_document(&covered_document) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("covered Python test document should open");
        let covered = path_to_file(session.db(), covered_path)
            .expect("covered Python document should be interned");
        session.install_ready_coverage_for_test(vec![covered], Vec::new());
        let generation = session.desired_generation();

        let unrelated_work = session.mark_intrinsic_change(
            &ChangeEvent::ContentChanged("/tmp/other.py".into()),
            FileKind::Python,
        );
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Ready(generation)
        );
        assert_eq!(unrelated_work, None);

        let covered_work = session.mark_intrinsic_change(
            &ChangeEvent::ContentChanged(covered_path.to_path_buf()),
            FileKind::Python,
        );
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(generation + 1)
        );
        assert_eq!(covered_work, Some(ProjectWork::Reprime));
    }

    #[test]
    fn failed_intrinsic_generation_records_covered_source_revisions() {
        let mut session = Session::default();
        let path = Utf8Path::new("/tmp/templatetags/failed.py");
        match session.open_document(&ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_file_path(path.as_std_path())
                .expect("failed source path should convert to a file URI"),
            language_id: "python".to_string(),
            version: 1,
            text: "failed source".to_string(),
        }) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("failed-source Python test document should open");
        let file =
            path_to_file(session.db(), path).expect("failed source document should be interned");
        session.install_ready_coverage_for_test(vec![file], Vec::new());
        assert_eq!(
            session.mark_intrinsic_change(
                &ChangeEvent::ContentChanged(path.to_path_buf()),
                FileKind::Python,
            ),
            Some(ProjectWork::Reprime)
        );
        let generation = session.desired_generation();
        let revision = file.revision(session.db());

        assert!(session.fail_intrinsic_readiness(generation));
        assert_eq!(
            session.intrinsic_readiness.admitted_revisions(),
            Some([(file, revision)].as_slice())
        );
    }

    #[test]
    fn failed_intrinsic_generation_retries_for_a_changed_source_revision() {
        let mut session = Session::default();
        let path = Utf8Path::new("/tmp/templatetags/current.py");
        let uri = ls_types::Uri::from_file_path(path.as_std_path())
            .expect("current Python path should convert to a file URI");
        match session.open_document(&ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "initial".to_string(),
        }) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("current-generation Python test document should open");
        let file =
            path_to_file(session.db(), path).expect("current Python document should be interned");
        session.install_ready_coverage_for_test(vec![file], Vec::new());
        assert_eq!(
            session.mark_intrinsic_change(
                &ChangeEvent::ContentChanged(path.to_path_buf()),
                FileKind::Python,
            ),
            Some(ProjectWork::Reprime)
        );
        let failed_generation = session.desired_generation();
        let failed_revision = file.revision(session.db());
        assert!(session.fail_intrinsic_readiness(failed_generation));
        let identical_work = match session.update_document(
            &ls_types::VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "initial".to_string(),
            }],
        ) {
            DocumentMutation::Applied { project_work, .. } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python test document should accept an identical update");
        assert_eq!(file.revision(session.db()), failed_revision);
        assert_eq!(identical_work, None);
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Failed(failed_generation)
        );

        let changed_work = match session.update_document(
            &ls_types::VersionedTextDocumentIdentifier { uri, version: 3 },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "changed".to_string(),
            }],
        ) {
            DocumentMutation::Applied { project_work, .. } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python test document should accept a changed update");
        let changed_revision = file.revision(session.db());
        assert!(changed_revision > failed_revision);
        assert_eq!(changed_work, Some(ProjectWork::Reprime));
        assert_eq!(
            session.intrinsic_readiness.admitted_revisions(),
            Some([(file, changed_revision)].as_slice())
        );
        let retry_generation = failed_generation + 1;
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(retry_generation)
        );

        let current_prime = prime_template_library_products(session.db())
            .expect("default session should have a project");
        assert!(session.publish_intrinsic_readiness(retry_generation, &current_prime));
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Ready(retry_generation)
        );
        assert!(session.intrinsic_readiness.admitted_revisions().is_none());
    }

    #[test]
    fn failed_intrinsic_generation_recovery_suppresses_unchanged_save_and_newer_source_supersedes()
    {
        let mut session = Session::default();
        let path = Utf8Path::new("/tmp/templatetags/recovery.py");
        let uri = ls_types::Uri::from_file_path(path.as_std_path())
            .expect("recovery Python path should convert to a file URI");
        match session.open_document(&ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "failed".to_string(),
        }) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("recovery Python test document should open");
        let file =
            path_to_file(session.db(), path).expect("recovery Python document should be interned");
        session.install_ready_coverage_for_test(vec![file], Vec::new());
        assert_eq!(
            session.mark_intrinsic_change(
                &ChangeEvent::ContentChanged(path.to_path_buf()),
                FileKind::Python,
            ),
            Some(ProjectWork::Reprime)
        );
        let failed_generation = session.desired_generation();
        assert!(session.fail_intrinsic_readiness(failed_generation));

        let changed_work = match session.update_document(
            &ls_types::VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "changed".to_string(),
            }],
        ) {
            DocumentMutation::Applied { project_work, .. } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("recovery Python test document should accept a changed update");
        let changed_revision = file.revision(session.db());
        let retry_generation = failed_generation + 1;
        assert_eq!(changed_work, Some(ProjectWork::Reprime));
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(retry_generation)
        );

        let unchanged_save_work =
            match session.save_document(&ls_types::TextDocumentIdentifier { uri: uri.clone() }) {
                DocumentMutation::Applied { project_work, .. } => Some(project_work),
                DocumentMutation::Ignored => None,
            }
            .expect("open recovery Python test document should save");
        assert_eq!(file.revision(session.db()), changed_revision);
        assert_eq!(unchanged_save_work, None);
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(retry_generation)
        );

        let newer_work = match session.update_document(
            &ls_types::VersionedTextDocumentIdentifier { uri, version: 3 },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "newer".to_string(),
            }],
        ) {
            DocumentMutation::Applied { project_work, .. } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("recovery Python test document should accept a newer update");
        let newer_revision = file.revision(session.db());
        assert!(newer_revision > changed_revision);
        assert_eq!(newer_work, Some(ProjectWork::Reprime));
        assert_eq!(
            session.intrinsic_readiness.admitted_revisions(),
            Some([(file, newer_revision)].as_slice())
        );
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(failed_generation + 2)
        );
        assert!(!session.fail_intrinsic_readiness(retry_generation));
    }

    #[test]
    fn retry_reprime_failure_retains_admission_and_accepts_newer_revision() {
        let mut session = Session::default();
        let path = Utf8Path::new("/tmp/templatetags/retry-failure.py");
        let uri = ls_types::Uri::from_file_path(path.as_std_path())
            .expect("retry Python path should convert to a file URI");
        match session.open_document(&ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "failed".to_string(),
        }) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("retry Python test document should open");
        let file =
            path_to_file(session.db(), path).expect("retry Python document should be interned");
        session.install_ready_coverage_for_test(vec![file], Vec::new());
        assert_eq!(
            session.mark_intrinsic_change(
                &ChangeEvent::ContentChanged(path.to_path_buf()),
                FileKind::Python,
            ),
            Some(ProjectWork::Reprime)
        );
        let failed_generation = session.desired_generation();
        assert!(session.fail_intrinsic_readiness(failed_generation));

        let changed_work = match session.update_document(
            &ls_types::VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "changed".to_string(),
            }],
        ) {
            DocumentMutation::Applied { project_work, .. } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("retry Python test document should accept a changed update");
        let changed_revision = file.revision(session.db());
        let retry_generation = failed_generation + 1;
        assert_eq!(changed_work, Some(ProjectWork::Reprime));
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(retry_generation)
        );

        assert!(session.fail_intrinsic_readiness(retry_generation));
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Failed(retry_generation)
        );
        assert_eq!(
            session.intrinsic_readiness.admitted_revisions(),
            Some([(file, changed_revision)].as_slice())
        );

        let unchanged_save_work =
            match session.save_document(&ls_types::TextDocumentIdentifier { uri: uri.clone() }) {
                DocumentMutation::Applied { project_work, .. } => Some(project_work),
                DocumentMutation::Ignored => None,
            }
            .expect("open retry Python test document should save");
        assert_eq!(file.revision(session.db()), changed_revision);
        assert_eq!(unchanged_save_work, None);
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Failed(retry_generation)
        );
        assert_eq!(
            session.intrinsic_readiness.admitted_revisions(),
            Some([(file, changed_revision)].as_slice())
        );

        let newer_work = match session.update_document(
            &ls_types::VersionedTextDocumentIdentifier { uri, version: 3 },
            vec![ls_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "newer".to_string(),
            }],
        ) {
            DocumentMutation::Applied { project_work, .. } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("retry Python test document should accept a newer update");
        let newer_revision = file.revision(session.db());
        assert!(newer_revision > changed_revision);
        assert_eq!(newer_work, Some(ProjectWork::Reprime));
        assert_eq!(
            session.intrinsic_readiness.admitted_revisions(),
            Some([(file, newer_revision)].as_slice())
        );
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(retry_generation + 1)
        );
    }

    #[test]
    fn new_python_membership_requests_discovery_even_with_published_coverage() {
        for path in [
            "/tmp/app/templatetags/new_library.py",
            "/tmp/configured_settings.py",
            "/tmp/missing_settings_candidate.py",
        ] {
            let mut session = Session::default();
            session.install_ready_coverage_for_test(Vec::new(), Vec::new());

            let project_work = session
                .mark_intrinsic_change(&ChangeEvent::BecameVisible(path.into()), FileKind::Python);

            assert_eq!(
                project_work,
                Some(ProjectWork::FullReload),
                "new candidate {path} must trigger discovery"
            );
        }
    }

    #[test]
    fn full_reload_clears_coverage_and_classifies_conservatively_until_publish() {
        let mut session = Session::default();
        let settings_path = Utf8Path::new("/tmp/settings.py");
        match session.open_document(&ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_file_path(settings_path.as_std_path())
                .expect("settings path should convert to a file URI"),
            language_id: "python".to_string(),
            version: 1,
            text: String::new(),
        }) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("settings Python test document should open");
        let settings_file = path_to_file(session.db(), settings_path)
            .expect("open settings document should be interned");
        session.install_ready_coverage_for_test(Vec::new(), vec![settings_file]);

        let settings_work = session.mark_intrinsic_change(
            &ChangeEvent::ContentChanged(settings_path.to_path_buf()),
            FileKind::Python,
        );
        assert!(session.intrinsic_readiness.coverage().is_none());
        assert_eq!(settings_work, Some(ProjectWork::FullReload));

        let conservative_work = session.mark_intrinsic_change(
            &ChangeEvent::ContentChanged("/tmp/not-previously-covered.py".into()),
            FileKind::Python,
        );
        assert_eq!(conservative_work, Some(ProjectWork::FullReload));
    }

    #[tokio::test]
    async fn readiness_watch_has_no_lost_wakeup_between_check_and_wait() {
        let mut session = Session::default();
        let mut readiness = session.readiness_receiver();
        let waiter = spawn(async move {
            loop {
                let state = *readiness.borrow_and_update();
                if !matches!(state, IntrinsicReadinessState::Unready(_)) {
                    return state;
                }
                readiness
                    .changed()
                    .await
                    .expect("readiness sender should remain live");
            }
        });

        yield_now().await;
        assert!(session.fail_intrinsic_readiness(0));
        let observed = timeout(Duration::from_secs(1), waiter)
            .await
            .expect("readiness waiter should finish before the test timeout")
            .expect("readiness waiter task should finish successfully");
        assert_eq!(observed, IntrinsicReadinessState::Failed(0));
    }

    #[test]
    fn explicit_prime_failure_releases_readiness_watchers() {
        let mut session = Session::default();
        let mut readiness = session.readiness_receiver();
        assert!(session.fail_intrinsic_readiness(0));
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Failed(0)
        );
        assert!(
            readiness
                .has_changed()
                .expect("readiness sender should remain live")
        );
        assert_eq!(
            *readiness.borrow_and_update(),
            IntrinsicReadinessState::Failed(0)
        );
        assert!(session.intrinsic_readiness.coverage().is_none());
        assert!(session.intrinsic_readiness.admitted_revisions().is_none());

        assert_eq!(
            session.mark_intrinsic_change(
                &ChangeEvent::ContentChanged("/tmp/unknown.py".into()),
                FileKind::Python,
            ),
            Some(ProjectWork::FullReload)
        );
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(1)
        );
    }

    #[test]
    fn session_new_uses_initial_project_until_django_discovery_loads_settings() {
        let tempdir = tempdir().expect("temporary project directory should be created");
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary project path should be valid UTF-8");
        let config_extra_path = root.join("config_extra");
        let client_extra_path = root.join("client_extra");
        let venv_path = root.join(".venv");
        std::fs::create_dir_all(config_extra_path.as_std_path())
            .expect("config Python path fixture should be created");
        std::fs::create_dir_all(client_extra_path.as_std_path())
            .expect("client Python path fixture should be created");
        std::fs::write(
            root.join(".env").as_std_path(),
            "FROM_ENV=should_not_load\n",
        )
        .expect("environment fixture should be written");
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            format!(
                r#"
django_settings_module = "config.settings"
pythonpath = ["{config_extra_path}"]
"#
            ),
        )
        .expect("project settings fixture should be written");

        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path())
                    .expect("project root should convert to a file URI"),
                name: "test_project".to_string(),
            }]),
            initialization_options: Some(serde_json::json!({
                "django_settings_module": "client.settings",
                "pythonpath": [client_extra_path.to_string()],
                "venv_path": venv_path.to_string(),
            })),
            ..Default::default()
        };

        let session = Session::new(&params);
        let db = session.db();
        let project = db.project().expect("initialize should create a project");

        assert_eq!(project.root(db), root.as_path());
        assert_eq!(
            project
                .django_settings_module(db)
                .as_ref()
                .map(djls_project::PythonModuleName::as_str),
            Some("client.settings")
        );
        assert_eq!(project.pythonpath(db), &vec![client_extra_path]);
        assert_eq!(project.interpreter(db), &Interpreter::VenvPath(venv_path));
        assert!(project.env_vars(db).is_empty());

        let search_paths: Vec<_> = project
            .search_paths(db)
            .iter()
            .map(|search_path| search_path.path().to_path_buf())
            .collect();
        assert_eq!(search_paths, vec![root]);
    }
}
