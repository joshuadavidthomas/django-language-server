#![allow(dead_code)]

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_project::build_source_roots;
use djls_project::first_party_discovery_files_request;
use djls_project::first_party_source_files_load_request;
use djls_project::merge_first_party_source_file_patch;
use djls_project::run_loading_plan;
use djls_project::Db as ProjectDb;
use djls_project::FirstPartySourceFilePatch;
use djls_project::LoadingEffects;
use djls_project::LoadingPlan;
use djls_project::LoadingRunResult;
use djls_project::NoopLoadingObserver;
use djls_project::ProjectSourceFilesApplyResult;
use djls_project::ProjectSourceFilesIssue;
use djls_source::File;
use djls_workspace::load_files_for_roots;
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

pub(crate) async fn run_startup_source_files(
    session: Arc<Mutex<Session>>,
    inputs: StartupRunInputs,
) -> StartupRunOutcome {
    run_startup_source_files_with_gate(session, inputs, None).await
}

async fn run_startup_source_files_with_gate(
    session: Arc<Mutex<Session>>,
    inputs: StartupRunInputs,
    load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
) -> StartupRunOutcome {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut effects = LspLoadingExecutor::new(handle, session, inputs, load_gate);
        let mut observer = NoopLoadingObserver;
        let result = run_loading_plan(LoadingPlan::phase3(), &mut effects, &mut observer);
        effects.finish(result)
    })
    .await
    .unwrap_or(StartupRunOutcome::Failed)
}

struct LspLoadingExecutor {
    handle: tokio::runtime::Handle,
    session: Arc<Mutex<Session>>,
    inputs: StartupRunInputs,
    roots: Vec<Utf8PathBuf>,
    outcome: Arc<StdMutex<Option<StartupRunOutcome>>>,
    load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl LspLoadingExecutor {
    fn new(
        handle: tokio::runtime::Handle,
        session: Arc<Mutex<Session>>,
        inputs: StartupRunInputs,
        load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self {
            handle,
            session,
            roots: inputs.snapshot().workspace_roots().to_vec(),
            inputs,
            outcome: Arc::new(StdMutex::new(None)),
            load_gate,
        }
    }

    fn record_outcome(&self, outcome: StartupRunOutcome) {
        let mut current = self.outcome.lock().expect("startup outcome mutex poisoned");
        if current.is_none() {
            *current = Some(outcome);
        }
    }

    fn finish(self, _result: LoadingRunResult) -> StartupRunOutcome {
        self.outcome
            .lock()
            .expect("startup outcome mutex poisoned")
            .clone()
            .unwrap_or(StartupRunOutcome::Succeeded)
    }
}

impl LoadingEffects for LspLoadingExecutor {
    fn begin_loading_run(&mut self) {
        let outcome = self
            .handle
            .block_on(self.inputs.guard().apply(&self.session, |session| {
                ProjectDb::begin_project_loading_run(session.db_mut());
                Ok(())
            }));
        match outcome {
            ApplyOutcome::Applied(()) => {}
            ApplyOutcome::Superseded => self.record_outcome(StartupRunOutcome::Superseded {
                generation: self.inputs.guard().generation(),
            }),
            ApplyOutcome::Rejected { .. } => self.record_outcome(StartupRunOutcome::Failed),
        }
    }

    fn load_source_file_set(&mut self) -> FirstPartySourceFilePatch {
        if let Some(load_gate) = &self.load_gate {
            load_gate();
        }
        let plan = build_source_roots(self.roots.clone());
        let (root_issues, request) =
            first_party_discovery_files_request(first_party_source_files_load_request(plan));
        FirstPartySourceFilePatch::first_party(root_issues, load_files_for_roots(request))
    }

    fn apply_source_file_patch(
        &mut self,
        patch: FirstPartySourceFilePatch,
    ) -> ProjectSourceFilesApplyResult {
        let fallback_patch = patch.clone();
        let outcome = self
            .handle
            .block_on(self.inputs.guard().apply(&self.session, |session| {
                if let Some(reason) = self.inputs.snapshot().stale_document_rejection(session) {
                    return Err(reason);
                }
                let current = session
                    .db()
                    .project_loading_state()
                    .source_files(session.db())
                    .ready_or_previous();
                let update = merge_first_party_source_file_patch(current.as_ref(), patch);
                Ok(session.db_mut().apply_project_source_files(update))
            }));

        match outcome {
            ApplyOutcome::Applied(applied) => applied,
            ApplyOutcome::Superseded => {
                self.record_outcome(StartupRunOutcome::Superseded {
                    generation: self.inputs.guard().generation(),
                });
                fallback_source_file_apply_result(fallback_patch, FallbackApplyResult::Deferred)
            }
            ApplyOutcome::Rejected { .. } => {
                self.record_outcome(StartupRunOutcome::Failed);
                fallback_source_file_apply_result(fallback_patch, FallbackApplyResult::Failed)
            }
        }
    }
}

enum FallbackApplyResult {
    Deferred,
    Failed,
}

fn fallback_source_file_apply_result(
    patch: FirstPartySourceFilePatch,
    result: FallbackApplyResult,
) -> ProjectSourceFilesApplyResult {
    let update = merge_first_party_source_file_patch(None, patch);
    let transition = update.applied_transition().clone();
    let issue = update
        .issues()
        .first()
        .cloned()
        .unwrap_or(ProjectSourceFilesIssue::NotLoaded);
    match result {
        FallbackApplyResult::Deferred => ProjectSourceFilesApplyResult::Deferred {
            transition,
            issue,
            previous: None,
        },
        FallbackApplyResult::Failed => ProjectSourceFilesApplyResult::Failed {
            transition,
            issue,
            previous: None,
        },
    }
}

#[cfg(test)]
mod startup_generation {
    use std::sync::atomic::AtomicBool;

    use djls_project::Db as _;
    use djls_project::ProjectSourceFilesAvailability;
    use djls_source::Db as _;
    use tower_lsp_server::ls_types;

    use super::*;

    fn initialized_session() -> Session {
        Session::new(&ls_types::InitializeParams::default())
    }

    fn initialized_session_with_root(root: &str) -> Session {
        Session::new(&ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root).expect("root should convert to URI"),
                name: root.to_string(),
            }]),
            ..ls_types::InitializeParams::default()
        })
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
    async fn startup_source_files_runs_source_file_set_through_loading_plan() {
        let session = Arc::new(Mutex::new(initialized_session_with_root(
            "/tmp/djls-startup-source-files-missing",
        )));
        let controller = StartupController::new();
        let inputs = {
            let session = session.lock().await;
            StartupRunInputs::capture(&session, controller.start_generation().await)
        };

        let outcome = run_startup_source_files(Arc::clone(&session), inputs).await;

        assert_eq!(outcome, StartupRunOutcome::Succeeded);
        let session = session.lock().await;
        assert!(matches!(
            session
                .db()
                .project_loading_state()
                .source_files(session.db()),
            ProjectSourceFilesAvailability::Unavailable { .. }
        ));
    }

    #[tokio::test]
    async fn startup_request_while_loading_does_not_wait_for_source_file_node() {
        let session = Arc::new(Mutex::new(initialized_session_with_root(
            "/tmp/djls-startup-source-files-blocked",
        )));
        let controller = StartupController::new();
        let path = "/workspace/templates/index.html";
        let inputs = {
            let mut session = session.lock().await;
            session.open_document(&text_document(path, 1, "{% if user %}hi{% endif %}"));
            StartupRunInputs::capture(&session, controller.start_generation().await)
        };
        let blocked = Arc::new(AtomicBool::new(false));
        let unblock = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let gate_blocked = Arc::clone(&blocked);
        let gate_unblock = Arc::clone(&unblock);
        let gate = Arc::new(move || {
            gate_blocked.store(true, Ordering::SeqCst);
            let (lock, cvar) = &*gate_unblock;
            let mut unblocked = lock.lock().expect("unblock mutex should not be poisoned");
            while !*unblocked {
                unblocked = cvar
                    .wait(unblocked)
                    .expect("unblock mutex should not be poisoned");
            }
        });

        let startup = tokio::spawn(run_startup_source_files_with_gate(
            Arc::clone(&session),
            inputs,
            Some(gate),
        ));
        while !blocked.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        let diagnostics = {
            let session = session.lock().await;
            let db = session.db();
            let file = db.get_or_create_file(&Utf8PathBuf::from(path));
            djls_ide::collect_diagnostics(db, file)
        };

        let (lock, cvar) = &*unblock;
        *lock.lock().expect("unblock mutex should not be poisoned") = true;
        cvar.notify_one();
        assert_eq!(startup.await.unwrap(), StartupRunOutcome::Succeeded);
        assert!(diagnostics.is_empty());
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
