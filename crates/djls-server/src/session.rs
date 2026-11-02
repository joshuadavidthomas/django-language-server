//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_db::DjangoDatabase;
use djls_ide::PrimedTemplateLibraries;
use djls_lsp::Analysis;
use djls_lsp::ClientInfo;
use djls_lsp::DocumentUpdate;
use djls_lsp::ProjectCoverage;
pub(crate) use djls_lsp::ProjectWork;
use djls_lsp::SessionCore;
use djls_lsp::TextDocument;
use djls_lsp::UriExt;
use djls_lsp::Workspace;
use djls_project::Db as ProjectDb;
use djls_source::Db as _;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use tokio::sync::watch;
use tower_lsp_server::ls_types;

use crate::diagnostics::DiagnosticQueue;

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

#[must_use = "document mutations can require project work to restore readiness"]
#[derive(Clone, Copy)]
pub(crate) enum DocumentMutation {
    Ignored,
    Applied { project_work: Option<ProjectWork> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicReadinessState {
    ReadyWithoutProject,
    Unready(IntrinsicGeneration),
    Ready(IntrinsicGeneration),
    Failed(IntrinsicGeneration),
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
        coverage: ProjectCoverage,
    },
    RetryReprime {
        generation: IntrinsicGeneration,
        coverage: ProjectCoverage,
        admitted_revisions: Arc<[(File, u64)]>,
    },
    Ready {
        generation: IntrinsicGeneration,
        coverage: ProjectCoverage,
    },
    FailedFullDiscovery {
        generation: IntrinsicGeneration,
    },
    FailedReprime {
        generation: IntrinsicGeneration,
        coverage: ProjectCoverage,
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

    const fn coverage(&self) -> Option<&ProjectCoverage> {
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
                // Lazy rule helpers need not appear in published structural coverage. Keep
                // edited helpers until publication so a failure records their revisions too.
                coverage: coverage.with_reprime_file(file),
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
                let mut admitted_revisions = admitted_revisions.to_vec();
                if let Some((_, admitted_revision)) = admitted_revisions
                    .iter_mut()
                    .find(|(admitted_file, _)| *admitted_file == file)
                {
                    if revision == *admitted_revision {
                        return false;
                    }
                    *admitted_revision = revision;
                } else {
                    admitted_revisions.push((file, revision));
                }
                Self::RetryReprime {
                    generation,
                    coverage: coverage.with_reprime_file(file),
                    admitted_revisions: admitted_revisions.into(),
                }
            }
            Self::NoProject { .. }
            | Self::FullDiscovery { .. }
            | Self::FailedFullDiscovery { .. } => return false,
        };
        *self = next;
        true
    }

    fn publish(&mut self, generation: IntrinsicGeneration, coverage: ProjectCoverage) -> bool {
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
        revisions: impl FnOnce(&ProjectCoverage) -> Arc<[(File, u64)]>,
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
                coverage: coverage.clone(),
            },
            Self::RetryReprime {
                generation: current,
                coverage,
                admitted_revisions,
            } if *current == generation => Self::FailedReprime {
                generation,
                coverage: coverage.clone(),
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

/// The native server's session: the shared [`SessionCore`] plus intrinsic readiness and the
/// push-diagnostics queue. Document mutations run under the server's `tokio::Mutex`; Project
/// work they require is admitted into readiness here and scheduled by the reload worker.
pub(crate) struct Session {
    core: SessionCore,
    intrinsic_readiness: IntrinsicReadiness,
    readiness_tx: watch::Sender<IntrinsicReadinessState>,
    pub(crate) diagnostics: DiagnosticQueue,
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

        let core = SessionCore::new(Workspace::new(), project_path.as_deref(), params);
        let intrinsic_readiness = IntrinsicReadiness::new(core.db().project().is_some());
        let (readiness_tx, _readiness_rx) = watch::channel(intrinsic_readiness.watched_state());

        Self {
            core,
            intrinsic_readiness,
            readiness_tx,
            diagnostics: DiagnosticQueue::default(),
        }
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            analysis: self.core.snapshot(),
            intrinsic_generation: self.ready_generation(),
        }
    }

    pub(crate) fn reprime_snapshot(&self) -> Option<(IntrinsicGeneration, SessionSnapshot)> {
        // A dequeued re-prime can wait behind a mutation that requires full discovery.
        // Admit it against current readiness while the caller still holds the snapshot lock.
        let generation = match &self.intrinsic_readiness {
            IntrinsicReadiness::Reprime { generation, .. }
            | IntrinsicReadiness::RetryReprime { generation, .. } => *generation,
            IntrinsicReadiness::NoProject { .. }
            | IntrinsicReadiness::FullDiscovery { .. }
            | IntrinsicReadiness::Ready { .. }
            | IntrinsicReadiness::FailedFullDiscovery { .. }
            | IntrinsicReadiness::FailedReprime { .. } => return None,
        };
        Some((generation, self.snapshot()))
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
            .mark_project_changed(self.core.db().project().is_some());
        self.publish_readiness();
        generation
    }

    pub(crate) fn publish_intrinsic_readiness(
        &mut self,
        generation: IntrinsicGeneration,
        primed: &PrimedTemplateLibraries,
    ) -> bool {
        let published = self
            .intrinsic_readiness
            .publish(generation, ProjectCoverage::from(primed));
        if published {
            self.publish_readiness();
        }
        published
    }

    pub(crate) fn fail_intrinsic_readiness(&mut self, generation: IntrinsicGeneration) -> bool {
        let db = self.core.db();
        let failed = self.intrinsic_readiness.fail(generation, |coverage| {
            coverage
                .reprime_files()
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
            coverage: ProjectCoverage::new(reprime_files, full_reload_files),
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

    #[cfg(test)]
    fn mark_intrinsic_change(
        &mut self,
        change: &djls_source::ChangeEvent,
        kind: djls_source::FileKind,
    ) -> Option<ProjectWork> {
        let work = self
            .core
            .project_work(change, kind, self.intrinsic_readiness.coverage());
        let path = match change {
            djls_source::ChangeEvent::Opened(path)
            | djls_source::ChangeEvent::BecameVisible(path)
            | djls_source::ChangeEvent::ContentChanged(path)
            | djls_source::ChangeEvent::Deleted(path) => path,
            djls_source::ChangeEvent::Rescan => return None,
        };
        self.admit_project_work(work, path)
    }

    /// Move readiness to the generation `work` on `path` produces; `None` when readiness
    /// already accounts for it.
    fn admit_project_work(
        &mut self,
        work: Option<ProjectWork>,
        path: &Utf8Path,
    ) -> Option<ProjectWork> {
        match work? {
            ProjectWork::Reprime => {
                let db = self.core.db();
                let file = db.files().try_file(path)?;
                let revision = file.revision(db);
                if !self.intrinsic_readiness.begin_reprime(file, revision) {
                    return None;
                }
            }
            ProjectWork::FullReload => {
                self.intrinsic_readiness.begin_full_discovery();
            }
        }
        self.publish_readiness();
        work
    }

    pub(crate) fn client_info(&self) -> &ClientInfo {
        self.core.client_info()
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        self.core.db()
    }

    pub(crate) fn db_mut(&mut self) -> &mut DjangoDatabase {
        self.core.db_mut()
    }

    pub(crate) fn open_document(
        &mut self,
        text_document: &ls_types::TextDocumentItem,
    ) -> DocumentMutation {
        let Some(path) = file_uri_path(&text_document.uri, "Skip opening document") else {
            return DocumentMutation::Ignored;
        };
        let update =
            self.core
                .open_document(&path, text_document, self.intrinsic_readiness.coverage());
        self.apply_document_update(&update)
    }

    pub(crate) fn save_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> DocumentMutation {
        let Some(path) = file_uri_path(&text_document.uri, "Skip saving document") else {
            return DocumentMutation::Ignored;
        };
        let Some(update) = self
            .core
            .save_document(&path, self.intrinsic_readiness.coverage())
        else {
            return DocumentMutation::Ignored;
        };
        self.apply_document_update(&update)
    }

    pub(crate) fn update_document(
        &mut self,
        text_document: &ls_types::VersionedTextDocumentIdentifier,
        changes: Vec<ls_types::TextDocumentContentChangeEvent>,
    ) -> DocumentMutation {
        let Some(path) = file_uri_path(&text_document.uri, "Skip updating document") else {
            return DocumentMutation::Ignored;
        };
        let Some(update) = self.core.update_document(
            &path,
            text_document.version,
            changes,
            self.intrinsic_readiness.coverage(),
        ) else {
            return DocumentMutation::Ignored;
        };
        self.apply_document_update(&update)
    }

    /// Close a document, letting future reads fall back to disk.
    pub(crate) fn close_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> DocumentMutation {
        let Some(path) = file_uri_path(&text_document.uri, "Skip closing document") else {
            return DocumentMutation::Ignored;
        };

        self.diagnostics.close(&path);
        let Some(update) = self
            .core
            .close_document(&path, self.intrinsic_readiness.coverage())
        else {
            return DocumentMutation::Ignored;
        };
        let project_work = self.admit_project_work(update.project_work, &path);
        DocumentMutation::Applied { project_work }
    }

    fn apply_document_update(&mut self, update: &DocumentUpdate) -> DocumentMutation {
        let project_work = self.admit_project_work(update.project_work, update.document.path());
        self.queue_document_diagnostics(&update.document);
        DocumentMutation::Applied { project_work }
    }

    /// Get a document from the buffer if it's open.
    #[cfg(test)]
    fn get_document(&self, path: &Utf8Path) -> Option<Arc<TextDocument>> {
        self.core.workspace().get_document(path)
    }

    fn queue_document_diagnostics(&mut self, document: &TextDocument) {
        // Diagnostic eligibility belongs to the IDE's path-derived source kind,
        // not the client language kind used for Project invalidation.
        if !self.client_info().supports_pull_diagnostics()
            || self.diagnostics.has_outstanding(document.path())
        {
            self.diagnostics.schedule(document);
        }
    }

    pub(crate) fn queue_all_diagnostics(&mut self) {
        for document in self.core.workspace().open_documents() {
            self.diagnostics.schedule(&document);
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(&ls_types::InitializeParams::default())
    }
}

/// The server handles only `file://` documents.
fn file_uri_path(uri: &ls_types::Uri, skipped: &'static str) -> Option<Utf8PathBuf> {
    let path = uri.to_utf8_path_buf();
    if path.is_none() {
        tracing::debug!(outcome = "skipped", reason = "non_file_uri", "{skipped}");
    }
    path
}

/// Immutable snapshot of session state, tagged with the intrinsic generation it was
/// captured at when readiness was current.
#[derive(Clone)]
pub(crate) struct SessionSnapshot {
    analysis: Analysis,
    intrinsic_generation: Option<IntrinsicGeneration>,
}

impl SessionSnapshot {
    pub(crate) fn analysis(&self) -> &Analysis {
        &self.analysis
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        self.analysis.db()
    }

    pub(crate) fn client_info(&self) -> &ClientInfo {
        self.analysis.client_info()
    }

    pub(crate) const fn intrinsic_generation(&self) -> Option<IntrinsicGeneration> {
        self.intrinsic_generation
    }

    /// The database path of a request's `file://` document.
    pub(crate) fn document_path(
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<Utf8PathBuf> {
        file_uri_path(&text_document.uri, "Skipping document request")
    }

    pub(crate) fn file_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<File> {
        self.analysis.file(&Self::document_path(text_document)?)
    }

    pub(crate) fn template_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<File> {
        self.analysis.template(&Self::document_path(text_document)?)
    }

    pub(crate) fn template_position_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        position: ls_types::Position,
    ) -> Option<(File, Offset)> {
        self.analysis
            .template_position(&Self::document_path(text_document)?, position)
    }

    pub(crate) fn template_range_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        range: ls_types::Range,
    ) -> Option<(File, Span)> {
        self.analysis
            .template_range(&Self::document_path(text_document)?, range)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::future::poll_fn;
    use std::str::FromStr;
    use std::task::Poll;
    use std::time::Duration;

    use camino::Utf8Path;
    use djls_ide::prime_template_library_products;
    use djls_project::Db as ProjectDb;
    use djls_project::PythonEnvironment;
    use djls_source::ChangeEvent;
    use djls_source::FileKind;
    use djls_source::FileStatus;
    use djls_source::path_to_file;
    use tempfile::tempdir;
    use tokio::spawn;
    use tokio::sync::Mutex;
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
        let project_work = match session.open_document(&ls_types::TextDocumentItem {
            uri: template_uri,
            language_id: "django-html".to_string(),
            version: 1,
            text: String::new(),
        }) {
            DocumentMutation::Applied { project_work } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("template test document should open");
        assert!(session.get_document(&template_path).is_some());
        assert_eq!(project_work, None);

        let (path, uri) = test_file_uri("test.py");
        let text_document = ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "print('hello')".to_string(),
        };
        match session.open_document(&text_document) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("Python test document should open");

        assert!(session.get_document(&path).is_some());

        let db = session.db();
        let file = path_to_file(db, &path).expect("open buffer should be visible to the overlay");
        let content = file
            .try_source(db)
            .expect("open buffer should be readable")
            .to_string();
        assert_eq!(content, "print('hello')");

        let close_doc = ls_types::TextDocumentIdentifier { uri };
        match session.close_document(&close_doc) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python test document should close");
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
        match session.update_document(&versioned_document, changes) {
            DocumentMutation::Applied { .. } => Some(()),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python test document should update");

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
        let project_work = match session.open_document(&ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: String::new(),
        }) {
            DocumentMutation::Applied { project_work } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("Python mutation test document should open");
        assert_eq!(project_work, Some(ProjectWork::FullReload));

        let file = path_to_file(session.db(), path)
            .expect("open mutation test document should be interned");
        session.install_ready_coverage_for_test(vec![file], Vec::new());

        let project_work =
            match session.save_document(&ls_types::TextDocumentIdentifier { uri: uri.clone() }) {
                DocumentMutation::Applied { project_work } => Some(project_work),
                DocumentMutation::Ignored => None,
            }
            .expect("open Python mutation test document should save");
        assert_eq!(project_work, Some(ProjectWork::Reprime));

        session.install_ready_coverage_for_test(vec![file], Vec::new());
        let project_work = match session.update_document(
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
            DocumentMutation::Applied { project_work } => Some(project_work),
            DocumentMutation::Ignored => None,
        }
        .expect("open Python mutation test document should update");
        assert_eq!(project_work, Some(ProjectWork::Reprime));

        session.install_ready_coverage_for_test(vec![file], Vec::new());
        let project_work =
            match session.close_document(&ls_types::TextDocumentIdentifier { uri }) {
                DocumentMutation::Applied { project_work } => Some(project_work),
                DocumentMutation::Ignored => None,
            }
            .expect("open Python mutation test document should close");
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
    fn reprime_snapshot_requires_pending_reprime_work() {
        let mut session = Session::default();
        let file = File::new(
            session.db(),
            Utf8PathBuf::from("/tmp/reprime-admission.py"),
            1,
            FileStatus::Exists,
        );
        session.intrinsic_readiness = IntrinsicReadiness::new(false);
        assert!(session.reprime_snapshot().is_none());
        let generation = session.mark_project_changed();
        assert!(session.reprime_snapshot().is_none());
        assert!(session.fail_intrinsic_readiness(generation));
        assert!(session.reprime_snapshot().is_none());

        let generation = session.mark_project_changed();
        let primed = prime_template_library_products(session.db())
            .expect("default session should have a Project");
        assert!(session.publish_intrinsic_readiness(generation, &primed));
        assert!(session.reprime_snapshot().is_none());

        for revision in [1, 2] {
            assert!(session.intrinsic_readiness.begin_reprime(file, revision));
            let generation = session.desired_generation();
            assert_eq!(
                session.reprime_snapshot().map(|(generation, _)| generation),
                Some(generation)
            );
            assert!(session.fail_intrinsic_readiness(generation));
            assert!(session.reprime_snapshot().is_none());
        }
    }

    #[tokio::test]
    async fn waiting_reprime_cannot_adopt_a_full_discovery_generation() {
        let session = Arc::new(Mutex::new(Session::default()));
        let mut mutation = Arc::clone(&session).lock_owned().await;
        mutation.intrinsic_readiness = IntrinsicReadiness::Reprime {
            generation: 1,
            coverage: ProjectCoverage::new([], []),
        };
        assert_eq!(
            mutation
                .reprime_snapshot()
                .map(|(generation, _)| generation),
            Some(1)
        );

        // Poll the dequeued job until it is waiting on the mutation's mutex, without sleeps.
        let mut reprime = std::pin::pin!(async {
            let session = session.lock().await;
            session.reprime_snapshot()
        });
        assert!(
            poll_fn(|cx| Poll::Ready(reprime.as_mut().poll(cx)))
                .await
                .is_pending()
        );

        let generation = mutation.mark_project_changed();
        assert_eq!(generation, 2);
        drop(mutation);
        assert!(
            reprime.await.is_none(),
            "a dequeued re-prime must not capture the newer full-discovery generation"
        );
        let mut session = session.lock().await;
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Unready(generation)
        );
        let primed = prime_template_library_products(session.db())
            .expect("default session should have a Project");
        assert!(
            session.publish_intrinsic_readiness(generation, &primed),
            "the queued full reload must retain the right to publish its generation"
        );
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Ready(generation)
        );
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
    fn uncovered_content_edits_reprime_only_within_registered_python_roots() {
        for (path, kind, expected) in [
            (
                "/tmp/lazy-project/helper.py",
                FileKind::Python,
                Some(ProjectWork::Reprime),
            ),
            (
                "/tmp/lazy-search/helper.py",
                FileKind::Python,
                Some(ProjectWork::Reprime),
            ),
            (
                "/tmp/lazy-project-sibling/helper.py",
                FileKind::Python,
                None,
            ),
            ("/tmp/outside/helper.py", FileKind::Python, None),
            ("/tmp/lazy-project/page.html", FileKind::Template, None),
        ] {
            let mut session = Session::default();
            for (root, kind) in [
                ("/tmp/lazy-project", djls_source::FileRootKind::Project),
                ("/tmp/lazy-search", djls_source::FileRootKind::SearchPath),
            ] {
                session
                    .db()
                    .files()
                    .try_add_root(session.db(), root.into(), kind);
            }
            let uri = ls_types::Uri::from_file_path(path).expect("fixture URI");
            assert!(matches!(
                session.open_document(&ls_types::TextDocumentItem {
                    uri: uri.clone(),
                    language_id: if kind == FileKind::Python {
                        "python"
                    } else {
                        "django-html"
                    }
                    .to_string(),
                    version: 1,
                    text: "initial".to_string(),
                }),
                DocumentMutation::Applied { .. }
            ));
            if expected.is_some() {
                // A rooted file still requires full discovery until coverage has published.
                assert_eq!(
                    session.mark_intrinsic_change(&ChangeEvent::ContentChanged(path.into()), kind),
                    Some(ProjectWork::FullReload)
                );
            }
            session.install_ready_coverage_for_test(Vec::new(), Vec::new());
            let generation = session.desired_generation();
            let mutation = session.update_document(
                &ls_types::VersionedTextDocumentIdentifier { uri, version: 2 },
                vec![ls_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "changed".to_string(),
                }],
            );
            let DocumentMutation::Applied { project_work, .. } = mutation else {
                panic!("fixture should update");
            };
            assert_eq!(project_work, expected, "{path}");
            assert_eq!(
                session.desired_generation(),
                generation + u64::from(expected.is_some()),
                "{path}"
            );
            if expected.is_some() {
                // Obsolete registered roots must not remain fallback dependency coverage.
                session.db().files().replace_roots(session.db(), Vec::new());
                session.install_ready_coverage_for_test(Vec::new(), Vec::new());
                assert_eq!(
                    session.mark_intrinsic_change(&ChangeEvent::ContentChanged(path.into()), kind),
                    None
                );
            }
        }
    }

    #[test]
    fn uncovered_helper_reprime_failure_tracks_revisions_and_gates_publication() {
        let mut session = Session::default();
        let path = Utf8Path::new("/tmp/lazy-retry/helper.py");
        session.db().files().try_add_root(
            session.db(),
            "/tmp/lazy-retry".into(),
            djls_source::FileRootKind::Project,
        );
        let uri = ls_types::Uri::from_file_path(path.as_std_path()).expect("fixture URI");
        assert!(matches!(
            session.open_document(&ls_types::TextDocumentItem {
                uri: uri.clone(),
                language_id: "python".to_string(),
                version: 1,
                text: "initial".to_string(),
            }),
            DocumentMutation::Applied { .. }
        ));
        let file = path_to_file(session.db(), path).expect("fixture file");
        session.install_ready_coverage_for_test(Vec::new(), Vec::new());
        let ready_generation = session.desired_generation();
        for (version, text) in [(2, "changed"), (3, "newer")] {
            let mutation = session.update_document(
                &ls_types::VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                vec![ls_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.to_string(),
                }],
            );
            assert!(matches!(
                mutation,
                DocumentMutation::Applied {
                    project_work: Some(ProjectWork::Reprime),
                    ..
                }
            ));
            let generation = session.desired_generation();
            assert_eq!(
                generation,
                ready_generation + u64::try_from(version - 1).expect("positive version")
            );
            assert_eq!(
                session.readiness_state(),
                IntrinsicReadinessState::Unready(generation)
            );
            assert!(
                session
                    .intrinsic_readiness
                    .coverage()
                    .expect("temporary coverage")
                    .reprime_files()
                    .contains(&file)
            );
            if version == 2 {
                assert!(session.fail_intrinsic_readiness(generation));
                assert_eq!(
                    session.readiness_state(),
                    IntrinsicReadinessState::Failed(generation)
                );
                assert_eq!(
                    session.intrinsic_readiness.admitted_revisions(),
                    Some([(file, file.revision(session.db()))].as_slice())
                );
                assert_eq!(
                    session.mark_intrinsic_change(
                        &ChangeEvent::ContentChanged(path.to_path_buf()),
                        FileKind::Python
                    ),
                    None
                );
                assert_eq!(
                    session.readiness_state(),
                    IntrinsicReadinessState::Failed(generation)
                );
            }
        }
        let current = session.desired_generation();
        let primed = prime_template_library_products(session.db()).expect("fixture project");
        assert!(!session.publish_intrinsic_readiness(current - 1, &primed));
        assert!(!session.fail_intrinsic_readiness(current - 1));
        assert!(session.publish_intrinsic_readiness(current, &primed));
        assert_eq!(
            session.readiness_state(),
            IntrinsicReadinessState::Ready(current)
        );
        assert!(
            !session
                .intrinsic_readiness
                .coverage()
                .expect("published coverage")
                .reprime_files()
                .contains(&file)
        );
    }

    #[test]
    fn failed_and_retry_reprime_admit_previously_uncovered_helpers() {
        let mut session = Session::default();
        session.db().files().try_add_root(
            session.db(),
            "/tmp/lazy-admission".into(),
            djls_source::FileRootKind::Project,
        );
        for name in ["first", "second", "third"] {
            let path = Utf8PathBuf::from(format!("/tmp/lazy-admission/{name}.py"));
            assert!(matches!(
                session.open_document(&ls_types::TextDocumentItem {
                    uri: ls_types::Uri::from_file_path(path.as_std_path()).expect("fixture URI"),
                    language_id: "python".to_string(),
                    version: 1,
                    text: "initial".to_string(),
                }),
                DocumentMutation::Applied { .. }
            ));
        }
        session.install_ready_coverage_for_test(Vec::new(), Vec::new());
        for (index, name) in ["first", "second", "third"].into_iter().enumerate() {
            let path = Utf8PathBuf::from(format!("/tmp/lazy-admission/{name}.py"));
            let generation = session.desired_generation();
            assert!(matches!(
                session.update_document(
                    &ls_types::VersionedTextDocumentIdentifier {
                        uri: ls_types::Uri::from_file_path(path.as_std_path())
                            .expect("fixture URI"),
                        version: 2
                    },
                    vec![ls_types::TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: "changed".to_string()
                    }],
                ),
                DocumentMutation::Applied {
                    project_work: Some(ProjectWork::Reprime),
                    ..
                }
            ));
            assert_eq!(session.desired_generation(), generation + 1);
            if index == 0 {
                assert!(session.fail_intrinsic_readiness(generation + 1));
            }
            let file = path_to_file(session.db(), &path).expect("fixture file");
            let revisions = session
                .intrinsic_readiness
                .admitted_revisions()
                .expect("failed/retry admission");
            assert_eq!(revisions.len(), index + 1);
            assert!(revisions.contains(&(file, file.revision(session.db()))));
            assert_eq!(
                session.mark_intrinsic_change(&ChangeEvent::ContentChanged(path), FileKind::Python),
                None
            );
            assert_eq!(session.desired_generation(), generation + 1);
        }
        let generation = session.desired_generation();
        assert!(session.fail_intrinsic_readiness(generation));
        assert_eq!(
            session
                .intrinsic_readiness
                .admitted_revisions()
                .expect("retained admission")
                .len(),
            3
        );
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
        assert_eq!(
            project.python_environment(db),
            &PythonEnvironment::Path(venv_path)
        );
        assert!(project.env_vars(db).is_empty());

        let search_paths: Vec<_> = project
            .search_paths(db)
            .iter()
            .map(|search_path| search_path.path().to_path_buf())
            .collect();
        assert_eq!(search_paths, vec![root]);
    }
}
