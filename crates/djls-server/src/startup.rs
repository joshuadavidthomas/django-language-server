#![allow(dead_code)]

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_source::File;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::Mutex;

use crate::session::Session;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StartupGeneration(u64);

#[derive(Debug)]
pub(crate) struct StartupController {
    next: AtomicU64,
    active: Arc<AtomicU64>,
    generation_lock: Arc<AsyncMutex<()>>,
}

impl Default for StartupController {
    fn default() -> Self {
        Self::new()
    }
}

impl StartupController {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            active: Arc::new(AtomicU64::new(0)),
            generation_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    pub(crate) async fn start_generation(&self) -> GenerationGuard {
        let _generation_lock = self.generation_lock.lock().await;
        let generation = StartupGeneration(self.next.fetch_add(1, Ordering::SeqCst));
        self.active.store(generation.0, Ordering::SeqCst);
        GenerationGuard {
            generation,
            active: Arc::clone(&self.active),
            generation_lock: Arc::clone(&self.generation_lock),
        }
    }

    #[must_use]
    pub(crate) fn guard_for_active_generation(&self) -> Option<GenerationGuard> {
        let generation = StartupGeneration(self.active.load(Ordering::SeqCst));
        (generation.0 != 0).then(|| GenerationGuard {
            generation,
            active: Arc::clone(&self.active),
            generation_lock: Arc::clone(&self.generation_lock),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GenerationGuard {
    generation: StartupGeneration,
    active: Arc<AtomicU64>,
    generation_lock: Arc<AsyncMutex<()>>,
}

impl GenerationGuard {
    #[must_use]
    pub(crate) fn generation(&self) -> StartupGeneration {
        self.generation
    }

    #[must_use]
    pub(crate) fn is_current(&self) -> bool {
        self.generation.0 != 0 && self.active.load(Ordering::SeqCst) == self.generation.0
    }

    pub(crate) async fn apply<T>(
        &self,
        session: &Arc<Mutex<Session>>,
        apply: impl FnOnce(&mut Session) -> Result<T, ApplyRejection>,
    ) -> ApplyOutcome<T> {
        let _generation_lock = self.generation_lock.lock().await;
        if !self.is_current() {
            return ApplyOutcome::Superseded;
        }

        let mut session = session.lock().await;

        match apply(&mut session) {
            Ok(value) => ApplyOutcome::Applied(value),
            Err(reason) => ApplyOutcome::Rejected { reason },
        }
    }

    pub(crate) async fn observe<T>(
        &self,
        session: &Arc<Mutex<Session>>,
        observe: impl FnOnce(&Session) -> T,
    ) -> ObservationOutcome<T> {
        let _generation_lock = self.generation_lock.lock().await;
        if !self.is_current() {
            return ObservationOutcome::Superseded;
        }

        let session = session.lock().await;

        ObservationOutcome::Observed(observe(&session))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ApplyOutcome<T> {
    Applied(T),
    Superseded,
    Rejected { reason: ApplyRejection },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ObservationOutcome<T> {
    Observed(T),
    Superseded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ApplyRejection {
    StaleDocument {
        file: File,
        path: Utf8PathBuf,
        captured: CapturedDocumentState,
        current: CapturedDocumentState,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DocumentVersion(i32);

impl DocumentVersion {
    #[must_use]
    pub(crate) fn new(version: i32) -> Self {
        Self(version)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapturedDocumentState {
    Open {
        version: DocumentVersion,
        epoch: DocumentEpoch,
    },
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DocumentEpoch(u64);

impl DocumentEpoch {
    #[must_use]
    pub(crate) fn new(epoch: u64) -> Self {
        Self(epoch)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CapturedDocumentSnapshot {
    file: File,
    path: Utf8PathBuf,
    state: CapturedDocumentState,
}

impl CapturedDocumentSnapshot {
    #[must_use]
    pub(crate) fn file(&self) -> File {
        self.file
    }

    #[must_use]
    pub(crate) fn path(&self) -> &Utf8PathBuf {
        &self.path
    }

    #[must_use]
    pub(crate) fn state(&self) -> CapturedDocumentState {
        self.state
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectLoadingSnapshot {
    workspace_roots: Vec<Utf8PathBuf>,
    settings: Settings,
    open_documents: Vec<CapturedDocumentSnapshot>,
}

impl ProjectLoadingSnapshot {
    #[must_use]
    pub(crate) fn capture(session: &Session) -> Self {
        let open_documents = session
            .open_documents()
            .into_iter()
            .map(|document| CapturedDocumentSnapshot {
                file: document.file(),
                path: document.path(session.db()).to_owned(),
                state: CapturedDocumentState::Open {
                    version: DocumentVersion::new(document.version()),
                    epoch: DocumentEpoch::new(
                        session
                            .open_document_freshness(document.path(session.db()))
                            .map_or(0, |(_version, epoch)| epoch),
                    ),
                },
            })
            .collect();

        Self {
            workspace_roots: session.workspace_roots().to_vec(),
            settings: session.db().settings().clone(),
            open_documents,
        }
    }

    #[must_use]
    pub(crate) fn workspace_roots(&self) -> &[Utf8PathBuf] {
        &self.workspace_roots
    }

    #[must_use]
    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    #[must_use]
    pub(crate) fn open_documents(&self) -> &[CapturedDocumentSnapshot] {
        &self.open_documents
    }

    #[must_use]
    pub(crate) fn stale_document_rejection(&self, session: &Session) -> Option<ApplyRejection> {
        self.open_documents.iter().find_map(|captured| {
            let current = session.open_document_freshness(captured.path()).map_or(
                CapturedDocumentState::Closed,
                |(version, epoch)| CapturedDocumentState::Open {
                    version: DocumentVersion::new(version),
                    epoch: DocumentEpoch::new(epoch),
                },
            );
            (current != captured.state()).then(|| ApplyRejection::StaleDocument {
                file: captured.file(),
                path: captured.path().clone(),
                captured: captured.state(),
                current,
            })
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct StartupRunInputs {
    snapshot: ProjectLoadingSnapshot,
    guard: GenerationGuard,
}

impl StartupRunInputs {
    #[must_use]
    pub(crate) fn capture(session: &Session, guard: GenerationGuard) -> Self {
        Self {
            snapshot: ProjectLoadingSnapshot::capture(session),
            guard,
        }
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> &ProjectLoadingSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub(crate) fn guard(&self) -> &GenerationGuard {
        &self.guard
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StartupRunOutcome {
    Succeeded,
    Failed,
    Superseded { generation: StartupGeneration },
}

#[cfg(test)]
mod startup_generation {
    use djls_project::Db as _;
    use tower_lsp_server::ls_types;

    use super::*;

    fn initialized_session() -> Session {
        Session::new(&ls_types::InitializeParams::default())
    }

    fn text_document(path: &str, version: i32, text: &str) -> ls_types::TextDocumentItem {
        ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_file_path(path).expect("test path should convert to URI"),
            language_id: "django-html".to_string(),
            version,
            text: text.to_string(),
        }
    }

    fn versioned_identifier(path: &str, version: i32) -> ls_types::VersionedTextDocumentIdentifier {
        ls_types::VersionedTextDocumentIdentifier {
            uri: ls_types::Uri::from_file_path(path).expect("test path should convert to URI"),
            version,
        }
    }

    fn identifier(path: &str) -> ls_types::TextDocumentIdentifier {
        ls_types::TextDocumentIdentifier {
            uri: ls_types::Uri::from_file_path(path).expect("test path should convert to URI"),
        }
    }

    #[tokio::test]
    async fn startup_run_inputs_capture_immutable_roots_settings_and_open_documents() {
        let mut session = initialized_session();
        session.open_document(&text_document(
            "/workspace/templates/index.html",
            3,
            "before",
        ));
        let controller = StartupController::new();
        let inputs = StartupRunInputs::capture(&session, controller.start_generation().await);

        session.open_document(&text_document(
            "/workspace/templates/other.html",
            1,
            "after",
        ));

        assert_eq!(inputs.snapshot().open_documents().len(), 1);
        assert_eq!(
            inputs.snapshot().open_documents()[0].state(),
            CapturedDocumentState::Open {
                version: DocumentVersion::new(3),
                epoch: DocumentEpoch::new(1),
            }
        );
        assert!(!inputs.snapshot().workspace_roots().is_empty());
        assert_eq!(inputs.guard().generation(), StartupGeneration(1));
    }

    #[tokio::test]
    async fn active_generation_is_absent_before_first_start() {
        let controller = StartupController::new();

        assert!(controller.guard_for_active_generation().is_none());
    }

    #[tokio::test]
    async fn default_controller_starts_at_first_real_generation() {
        let controller = StartupController::default();

        let guard = controller.start_generation().await;

        assert_eq!(guard.generation(), StartupGeneration(1));
    }

    #[tokio::test]
    async fn guarded_apply_returns_superseded_before_locking_session() {
        let session = Arc::new(Mutex::new(initialized_session()));
        let controller = StartupController::new();
        let old = controller.start_generation().await;
        let _new = controller.start_generation().await;

        let outcome = old.apply(&session, |_session| Ok(())).await;

        assert_eq!(outcome, ApplyOutcome::Superseded);
    }

    #[tokio::test]
    async fn guarded_observation_returns_superseded() {
        let session = Arc::new(Mutex::new(initialized_session()));
        let controller = StartupController::new();
        let old = controller.start_generation().await;
        let _new = controller.start_generation().await;

        let outcome = old.observe(&session, |_session| 1usize).await;

        assert_eq!(outcome, ObservationOutcome::Superseded);
    }

    #[tokio::test]
    async fn guarded_apply_rejects_stale_changed_document_with_evidence() {
        let session = Arc::new(Mutex::new(initialized_session()));
        let controller = StartupController::new();
        let guard = controller.start_generation().await;
        let path = "/workspace/templates/index.html";
        let inputs = {
            let mut session = session.lock().await;
            session.open_document(&text_document(path, 1, "before"));
            StartupRunInputs::capture(&session, guard.clone())
        };
        {
            let mut session = session.lock().await;
            session.update_document(
                &versioned_identifier(path, 2),
                vec![ls_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "after".to_string(),
                }],
            );
        }

        let outcome = inputs
            .guard()
            .apply(&session, |session| {
                inputs
                    .snapshot()
                    .stale_document_rejection(session)
                    .map_or(Ok(()), Err)
            })
            .await;

        let ApplyOutcome::Rejected {
            reason:
                ApplyRejection::StaleDocument {
                    path: rejected_path,
                    captured,
                    current,
                    ..
                },
        } = outcome
        else {
            panic!("expected stale document rejection");
        };
        assert_eq!(rejected_path, Utf8PathBuf::from(path));
        assert_eq!(
            captured,
            CapturedDocumentState::Open {
                version: DocumentVersion::new(1),
                epoch: DocumentEpoch::new(1),
            }
        );
        assert_eq!(
            current,
            CapturedDocumentState::Open {
                version: DocumentVersion::new(2),
                epoch: DocumentEpoch::new(2),
            }
        );
    }

    #[tokio::test]
    async fn guarded_apply_rejects_stale_closed_document_with_evidence() {
        let session = Arc::new(Mutex::new(initialized_session()));
        let controller = StartupController::new();
        let guard = controller.start_generation().await;
        let path = "/workspace/templates/index.html";
        let inputs = {
            let mut session = session.lock().await;
            session.open_document(&text_document(path, 1, "before"));
            StartupRunInputs::capture(&session, guard.clone())
        };
        {
            let mut session = session.lock().await;
            session.close_document(&identifier(path));
        }

        let outcome = inputs
            .guard()
            .apply(&session, |session| {
                inputs
                    .snapshot()
                    .stale_document_rejection(session)
                    .map_or(Ok(()), Err)
            })
            .await;

        let ApplyOutcome::Rejected {
            reason:
                ApplyRejection::StaleDocument {
                    captured, current, ..
                },
        } = outcome
        else {
            panic!("expected stale document rejection");
        };
        assert_eq!(
            captured,
            CapturedDocumentState::Open {
                version: DocumentVersion::new(1),
                epoch: DocumentEpoch::new(1),
            }
        );
        assert_eq!(current, CapturedDocumentState::Closed);
    }

    #[tokio::test]
    async fn guarded_apply_rejects_close_reopen_with_same_version() {
        let session = Arc::new(Mutex::new(initialized_session()));
        let controller = StartupController::new();
        let guard = controller.start_generation().await;
        let path = "/workspace/templates/index.html";
        let inputs = {
            let mut session = session.lock().await;
            session.open_document(&text_document(path, 1, "before"));
            StartupRunInputs::capture(&session, guard.clone())
        };
        {
            let mut session = session.lock().await;
            session.close_document(&identifier(path));
            session.open_document(&text_document(path, 1, "reopened"));
        }

        let outcome = inputs
            .guard()
            .apply(&session, |session| {
                inputs
                    .snapshot()
                    .stale_document_rejection(session)
                    .map_or(Ok(()), Err)
            })
            .await;

        let ApplyOutcome::Rejected {
            reason:
                ApplyRejection::StaleDocument {
                    captured, current, ..
                },
        } = outcome
        else {
            panic!("expected stale document rejection");
        };
        assert_eq!(
            captured,
            CapturedDocumentState::Open {
                version: DocumentVersion::new(1),
                epoch: DocumentEpoch::new(1),
            }
        );
        assert_eq!(
            current,
            CapturedDocumentState::Open {
                version: DocumentVersion::new(1),
                epoch: DocumentEpoch::new(3),
            }
        );
    }

    #[tokio::test]
    async fn supersession_waits_for_guarded_apply_linearization() {
        use std::time::Duration;

        let session = Arc::new(Mutex::new(initialized_session()));
        let session_lock = session.lock().await;
        let controller = Arc::new(StartupController::new());
        let old = controller.start_generation().await;
        let apply_session = Arc::clone(&session);
        let apply = tokio::spawn(async move { old.apply(&apply_session, |_session| Ok(())).await });
        tokio::task::yield_now().await;

        let restart_controller = Arc::clone(&controller);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let restart = tokio::spawn(async move {
            let guard = restart_controller.start_generation().await;
            let _ = tx.send(guard.generation());
        });

        assert!(tokio::time::timeout(Duration::from_millis(10), rx)
            .await
            .is_err());

        drop(session_lock);
        assert_eq!(apply.await.unwrap(), ApplyOutcome::Applied(()));
        restart.await.unwrap();
        assert_eq!(
            controller
                .guard_for_active_generation()
                .unwrap()
                .generation(),
            StartupGeneration(2)
        );
    }

    #[tokio::test]
    async fn guarded_apply_can_run_reset_intent() {
        let session = Arc::new(Mutex::new(initialized_session()));
        let controller = StartupController::new();
        let guard = controller.start_generation().await;

        let outcome = guard
            .apply(&session, |session| {
                djls_project::Db::begin_project_loading_run(session.db_mut());
                Ok(session
                    .db()
                    .project_loading_state()
                    .source_files(session.db()))
            })
            .await;

        assert!(matches!(outcome, ApplyOutcome::Applied(_)));
    }
}
