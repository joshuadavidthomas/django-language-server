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
use djls_project::DjangoEnvironmentCandidatesOutcome;
use djls_project::FirstPartySourceFilePatch;
use djls_project::LoadingApplyOutcome;
use djls_project::LoadingEffects;
use djls_project::LoadingExecutionOutcome;
use djls_project::LoadingObservationOutcome;
use djls_project::LoadingObserver;
use djls_project::LoadingPlan;
use djls_project::LoadingRunControl;
use djls_project::LoadingRunResult;
use djls_project::NodeId;
use djls_project::NodeTerminalStatus;
use djls_project::ProjectDiscoveryApplyResult;
use djls_project::ProjectDiscoveryLoadRequest;
use djls_project::ProjectDiscoverySetData;
use djls_project::ProjectSourceFilesApplyResult;
use djls_project::PythonSourceIndexOutcome;
use djls_source::File;
use djls_workspace::load_files_for_roots;
use tokio::sync::mpsc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::Mutex;
use tower_lsp_server::ls_types;
use tower_lsp_server::ls_types::notification::Progress as ProgressNotification;
use tower_lsp_server::Client;

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
        let generation = StartupGeneration(self.next.fetch_add(1, Ordering::SeqCst));
        self.active.store(generation.0, Ordering::SeqCst);
        let _generation_lock = self.generation_lock.lock().await;
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
        if !self.is_current() {
            return ApplyOutcome::Superseded;
        }

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

impl ApplyRejection {
    #[must_use]
    pub(crate) fn path(&self) -> &Utf8PathBuf {
        match self {
            Self::StaleDocument { path, .. } => path,
        }
    }
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
    progress: StartupProgress,
}

impl StartupRunInputs {
    #[must_use]
    pub(crate) fn capture(session: &Session, guard: GenerationGuard) -> Self {
        Self::capture_with_progress(session, guard, StartupProgress::log_fallback())
    }

    #[must_use]
    pub(crate) fn capture_with_progress(
        session: &Session,
        guard: GenerationGuard,
        progress: StartupProgress,
    ) -> Self {
        Self {
            snapshot: ProjectLoadingSnapshot::capture(session),
            guard,
            progress,
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

    #[must_use]
    pub(crate) fn progress(&self) -> StartupProgress {
        self.progress.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StartupRunOutcome {
    Succeeded,
    Failed,
    Superseded { generation: StartupGeneration },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StartupProgressEvent {
    Begin,
    NodeStarted(NodeId),
    NodeFinished {
        node: NodeId,
        status: NodeTerminalStatus,
    },
    Finish(StartupRunOutcome),
}

#[derive(Clone, Debug)]
pub(crate) struct StartupProgress {
    reporter: Arc<dyn StartupProgressReporter>,
}

impl StartupProgress {
    #[must_use]
    pub(crate) fn log_fallback() -> Self {
        Self::new(LogStartupProgressReporter)
    }

    #[must_use]
    pub(crate) fn for_client(
        client: Client,
        work_done_progress: bool,
        generation: StartupGeneration,
    ) -> Self {
        if work_done_progress {
            Self::new(WorkDoneStartupProgressReporter::new(client, generation))
        } else {
            Self::log_fallback()
        }
    }

    #[cfg(test)]
    #[must_use]
    fn recording(events: Arc<StdMutex<Vec<StartupProgressEvent>>>) -> Self {
        Self::new(RecordingStartupProgressReporter { events })
    }

    #[must_use]
    fn new(reporter: impl StartupProgressReporter + 'static) -> Self {
        Self {
            reporter: Arc::new(reporter),
        }
    }

    fn begin(&self, handle: &tokio::runtime::Handle) {
        self.reporter.begin(handle);
    }

    fn report_node_started(&self, handle: &tokio::runtime::Handle, node: NodeId) {
        self.reporter.node_started(handle, node);
    }

    fn report_node_finished(
        &self,
        handle: &tokio::runtime::Handle,
        node: NodeId,
        status: NodeTerminalStatus,
    ) {
        self.reporter.node_finished(handle, node, status);
    }

    fn finish(&self, handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome) {
        self.reporter.finish(handle, outcome);
    }
}

trait StartupProgressReporter: Send + Sync + std::fmt::Debug {
    fn begin(&self, handle: &tokio::runtime::Handle);
    fn node_started(&self, handle: &tokio::runtime::Handle, node: NodeId);
    fn node_finished(
        &self,
        handle: &tokio::runtime::Handle,
        node: NodeId,
        status: NodeTerminalStatus,
    );
    fn finish(&self, handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome);
}

#[derive(Debug)]
struct LogStartupProgressReporter;

impl StartupProgressReporter for LogStartupProgressReporter {
    fn begin(&self, _handle: &tokio::runtime::Handle) {
        tracing::info!("Starting Django project loading");
    }

    fn node_started(&self, _handle: &tokio::runtime::Handle, node: NodeId) {
        tracing::info!(node = ?node, "Started Django project loading task");
    }

    fn node_finished(
        &self,
        _handle: &tokio::runtime::Handle,
        node: NodeId,
        status: NodeTerminalStatus,
    ) {
        tracing::info!(node = ?node, status = ?status, "Finished Django project loading task");
    }

    fn finish(&self, _handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome) {
        tracing::info!(outcome = ?outcome, "Finished Django project loading");
    }
}

#[derive(Debug)]
struct WorkDoneStartupProgressReporter {
    client: Client,
    token: ls_types::ProgressToken,
    sender: StdMutex<Option<mpsc::UnboundedSender<WorkDoneProgressCommand>>>,
    finished: StdMutex<bool>,
}

impl WorkDoneStartupProgressReporter {
    fn new(client: Client, generation: StartupGeneration) -> Self {
        Self {
            client,
            token: work_done_progress_token(generation),
            sender: StdMutex::new(None),
            finished: StdMutex::new(false),
        }
    }

    fn message_for_outcome(outcome: &StartupRunOutcome) -> &'static str {
        match outcome {
            StartupRunOutcome::Succeeded => "Django project loading complete",
            StartupRunOutcome::Failed => "Django project loading failed",
            StartupRunOutcome::Superseded { .. } => "Django project loading superseded",
        }
    }

    fn send(&self, command: WorkDoneProgressCommand) {
        let sender = self
            .sender
            .lock()
            .expect("startup progress mutex poisoned")
            .clone();
        if let Some(sender) = sender {
            let _ = sender.send(command);
        }
    }
}

impl StartupProgressReporter for WorkDoneStartupProgressReporter {
    fn begin(&self, handle: &tokio::runtime::Handle) {
        let (sender, receiver) = mpsc::unbounded_channel();
        *self.sender.lock().expect("startup progress mutex poisoned") = Some(sender.clone());
        spawn_work_done_progress_dispatcher(
            handle,
            self.client.clone(),
            self.token.clone(),
            receiver,
        );
        let _ = sender.send(WorkDoneProgressCommand::Begin);
    }

    fn node_started(&self, _handle: &tokio::runtime::Handle, node: NodeId) {
        self.send(WorkDoneProgressCommand::Report(format!("Started {node:?}")));
    }

    fn node_finished(
        &self,
        _handle: &tokio::runtime::Handle,
        node: NodeId,
        status: NodeTerminalStatus,
    ) {
        self.send(WorkDoneProgressCommand::Report(format!(
            "Finished {node:?}: {status:?}"
        )));
    }

    fn finish(&self, _handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome) {
        let mut finished = self
            .finished
            .lock()
            .expect("startup progress mutex poisoned");
        if *finished {
            return;
        }
        *finished = true;
        self.send(WorkDoneProgressCommand::End(
            Self::message_for_outcome(outcome).to_string(),
        ));
    }
}

#[derive(Debug)]
enum WorkDoneProgressCommand {
    Begin,
    Report(String),
    End(String),
}

#[derive(Debug)]
struct WorkDoneProgressMachine {
    token: ls_types::ProgressToken,
    active: bool,
    finished: bool,
}

impl WorkDoneProgressMachine {
    fn new(token: ls_types::ProgressToken) -> Self {
        Self {
            token,
            active: false,
            finished: false,
        }
    }

    fn begin(&mut self, created: bool) -> Option<ls_types::ProgressParams> {
        if !created {
            self.active = false;
            return None;
        }
        self.active = true;
        Some(ls_types::ProgressParams {
            token: self.token.clone(),
            value: ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::Begin(
                ls_types::WorkDoneProgressBegin {
                    title: "Loading Django project".to_string(),
                    cancellable: Some(false),
                    message: Some("Starting".to_string()),
                    percentage: None,
                },
            )),
        })
    }

    fn report(&self, message: String) -> Option<ls_types::ProgressParams> {
        (self.active && !self.finished).then(|| ls_types::ProgressParams {
            token: self.token.clone(),
            value: ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::Report(
                ls_types::WorkDoneProgressReport {
                    message: Some(message),
                    ..Default::default()
                },
            )),
        })
    }

    fn finish(&mut self, message: String) -> Option<ls_types::ProgressParams> {
        if !self.active || self.finished {
            return None;
        }
        self.finished = true;
        Some(ls_types::ProgressParams {
            token: self.token.clone(),
            value: ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::End(
                ls_types::WorkDoneProgressEnd {
                    message: Some(message),
                },
            )),
        })
    }
}

fn work_done_progress_token(generation: StartupGeneration) -> ls_types::ProgressToken {
    ls_types::ProgressToken::String(format!("djls/startup/{}", generation.0))
}

fn spawn_work_done_progress_dispatcher(
    handle: &tokio::runtime::Handle,
    client: Client,
    token: ls_types::ProgressToken,
    mut receiver: mpsc::UnboundedReceiver<WorkDoneProgressCommand>,
) {
    handle.spawn(async move {
        let mut machine = WorkDoneProgressMachine::new(token.clone());
        while let Some(command) = receiver.recv().await {
            match command {
                WorkDoneProgressCommand::Begin => {
                    let created = client
                        .create_work_done_progress(token.clone())
                        .await
                        .is_ok();
                    if let Some(params) = machine.begin(created) {
                        client
                            .send_notification::<ProgressNotification>(params)
                            .await;
                    } else {
                        tracing::debug!(token = ?token, "Work-done progress unavailable");
                    }
                }
                WorkDoneProgressCommand::Report(message) => {
                    if let Some(params) = machine.report(message) {
                        client
                            .send_notification::<ProgressNotification>(params)
                            .await;
                    }
                }
                WorkDoneProgressCommand::End(message) => {
                    if let Some(params) = machine.finish(message) {
                        client
                            .send_notification::<ProgressNotification>(params)
                            .await;
                    }
                    break;
                }
            }
        }
    });
}

struct GuardedStartupProgressObserver {
    progress: StartupProgress,
    guard: GenerationGuard,
}

impl LoadingObserver for GuardedStartupProgressObserver {
    fn node_started(&mut self, node: NodeId) {
        if self.guard.is_current() {
            self.progress
                .report_node_started(&tokio::runtime::Handle::current(), node);
        }
    }

    fn node_finished(&mut self, node: NodeId, status: NodeTerminalStatus) {
        if self.guard.is_current() || status == NodeTerminalStatus::Superseded {
            self.progress
                .report_node_finished(&tokio::runtime::Handle::current(), node, status);
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
struct RecordingStartupProgressReporter {
    events: Arc<StdMutex<Vec<StartupProgressEvent>>>,
}

#[cfg(test)]
impl StartupProgressReporter for RecordingStartupProgressReporter {
    fn begin(&self, _handle: &tokio::runtime::Handle) {
        self.events
            .lock()
            .expect("startup progress events mutex poisoned")
            .push(StartupProgressEvent::Begin);
    }

    fn node_started(&self, _handle: &tokio::runtime::Handle, node: NodeId) {
        self.events
            .lock()
            .expect("startup progress events mutex poisoned")
            .push(StartupProgressEvent::NodeStarted(node));
    }

    fn node_finished(
        &self,
        _handle: &tokio::runtime::Handle,
        node: NodeId,
        status: NodeTerminalStatus,
    ) {
        self.events
            .lock()
            .expect("startup progress events mutex poisoned")
            .push(StartupProgressEvent::NodeFinished { node, status });
    }

    fn finish(&self, _handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome) {
        self.events
            .lock()
            .expect("startup progress events mutex poisoned")
            .push(StartupProgressEvent::Finish(outcome.clone()));
    }
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
    run_startup_source_files_with_gates(session, inputs, load_gate, None).await
}

async fn run_startup_source_files_with_gates(
    session: Arc<Mutex<Session>>,
    inputs: StartupRunInputs,
    load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    python_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
) -> StartupRunOutcome {
    run_startup_source_files_with_all_gates(session, inputs, load_gate, python_observe_gate, None)
        .await
}

async fn run_startup_source_files_with_all_gates(
    session: Arc<Mutex<Session>>,
    inputs: StartupRunInputs,
    load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    python_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    environment_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
) -> StartupRunOutcome {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let progress = inputs.progress();
        let observer_guard = inputs.guard().clone();
        progress.begin(&handle);
        let mut effects = LspLoadingExecutor::new(
            handle.clone(),
            session,
            inputs,
            load_gate,
            python_observe_gate,
            environment_observe_gate,
        );
        let mut observer = GuardedStartupProgressObserver {
            progress: progress.clone(),
            guard: observer_guard,
        };
        let result = run_loading_plan(LoadingPlan::phase3(), &mut effects, &mut observer);
        let outcome = effects.finish(result);
        progress.finish(&handle, &outcome);
        outcome
    })
    .await
    .unwrap_or(StartupRunOutcome::Failed)
}

struct LspLoadingExecutor {
    handle: tokio::runtime::Handle,
    session: Arc<Mutex<Session>>,
    inputs: StartupRunInputs,
    roots: Vec<Utf8PathBuf>,
    load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    python_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    environment_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl LspLoadingExecutor {
    fn new(
        handle: tokio::runtime::Handle,
        session: Arc<Mutex<Session>>,
        inputs: StartupRunInputs,
        load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
        python_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
        environment_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self {
            handle,
            session,
            roots: inputs.snapshot().workspace_roots().to_vec(),
            inputs,
            load_gate,
            python_observe_gate,
            environment_observe_gate,
        }
    }

    fn finish(self, result: LoadingRunResult) -> StartupRunOutcome {
        if !self.inputs.guard().is_current() {
            return StartupRunOutcome::Superseded {
                generation: self.inputs.guard().generation(),
            };
        }
        match result.execution_outcome() {
            Some(LoadingExecutionOutcome::Superseded) => StartupRunOutcome::Superseded {
                generation: self.inputs.guard().generation(),
            },
            Some(LoadingExecutionOutcome::RejectedApply) => StartupRunOutcome::Failed,
            None => StartupRunOutcome::Succeeded,
        }
    }
}

impl LoadingEffects for LspLoadingExecutor {
    fn begin_loading_run(&mut self) -> LoadingRunControl {
        if self.inputs.guard().is_current() {
            LoadingRunControl::Continue
        } else {
            LoadingRunControl::Abort(LoadingExecutionOutcome::Superseded)
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
    ) -> LoadingApplyOutcome<ProjectSourceFilesApplyResult> {
        let outcome = self
            .handle
            .block_on(self.inputs.guard().apply(&self.session, |session| {
                let current = ProjectDb::project(session.db())
                    .source_inventory(session.db())
                    .ready();
                if let Some(reason) = self.inputs.snapshot().stale_document_rejection(session) {
                    return Err(reason);
                }
                let update = merge_first_party_source_file_patch(current.as_ref(), patch);
                Ok(session.db_mut().apply_project_source_files(update))
            }));

        match outcome {
            ApplyOutcome::Applied(applied) => LoadingApplyOutcome::Applied(applied),
            ApplyOutcome::Superseded => LoadingApplyOutcome::Superseded,
            ApplyOutcome::Rejected { .. } => LoadingApplyOutcome::RejectedApply,
        }
    }

    fn load_project_discovery_set(&mut self) -> ProjectDiscoverySetData {
        let roots = build_source_roots(self.roots.clone())
            .roots()
            .iter()
            .map(|root| root.path().to_owned())
            .collect();
        djls_project::build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
            roots,
            self.inputs.snapshot().settings().clone(),
        ))
    }

    fn apply_project_discovery_data(
        &mut self,
        data: ProjectDiscoverySetData,
    ) -> LoadingApplyOutcome<ProjectDiscoveryApplyResult> {
        let outcome = self
            .handle
            .block_on(self.inputs.guard().apply(&self.session, |session| {
                Ok(session.db_mut().apply_project_discovery_data(data))
            }));

        match outcome {
            ApplyOutcome::Applied(applied) => LoadingApplyOutcome::Applied(applied),
            ApplyOutcome::Superseded => LoadingApplyOutcome::Superseded,
            ApplyOutcome::Rejected { .. } => LoadingApplyOutcome::RejectedApply,
        }
    }

    fn observe_python_source_index(
        &mut self,
    ) -> LoadingObservationOutcome<PythonSourceIndexOutcome> {
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        if let Some(gate) = &self.python_observe_gate {
            gate();
        }
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        let db = self.handle.block_on(async {
            let session = self.session.lock().await;
            session.project_db_snapshot_for_observation()
        });
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        let project = ProjectDb::project(&db);
        let outcome = djls_project::python_source_index(&db, project).clone();
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        LoadingObservationOutcome::Observed(outcome)
    }

    fn observe_django_environment_candidates(
        &mut self,
    ) -> LoadingObservationOutcome<DjangoEnvironmentCandidatesOutcome> {
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        if let Some(gate) = &self.environment_observe_gate {
            gate();
        }
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        let db = self.handle.block_on(async {
            let session = self.session.lock().await;
            session.project_db_snapshot_for_observation()
        });
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        let project = ProjectDb::project(&db);
        let outcome = djls_project::django_environment_candidates(&db, project).clone();
        if !self.inputs.guard().is_current() {
            return LoadingObservationOutcome::Superseded;
        }
        LoadingObservationOutcome::Observed(outcome)
    }
}

#[cfg(test)]
mod startup_generation {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;

    use djls_project::ProjectDiscovery;
    use djls_project::ProjectSourceInventory;
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

    async fn seed_ready_source_inventory(
        session: Arc<Mutex<Session>>,
        controller: &StartupController,
    ) -> ProjectSourceInventory {
        let inputs = {
            let session = session.lock().await;
            StartupRunInputs::capture(&session, controller.start_generation().await)
        };
        assert_eq!(
            run_startup_source_files(Arc::clone(&session), inputs).await,
            StartupRunOutcome::Succeeded
        );
        let session = session.lock().await;
        let inventory = ProjectDb::project(session.db()).source_inventory(session.db());
        assert!(matches!(inventory, ProjectSourceInventory::Ready(_)));
        inventory
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
    async fn supersession_marks_active_before_waiting_for_guarded_apply_linearization() {
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
        assert_eq!(
            controller
                .guard_for_active_generation()
                .unwrap()
                .generation(),
            StartupGeneration(2)
        );

        drop(session_lock);
        assert_eq!(apply.await.unwrap(), ApplyOutcome::Superseded);
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
            ProjectDb::project(session.db()).source_inventory(session.db()),
            ProjectSourceInventory::Unavailable { .. }
        ));
        assert!(matches!(
            ProjectDb::project(session.db()).discovery(session.db()),
            ProjectDiscovery::Ready(_)
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
    async fn environment_discovery_request_while_running_does_not_wait() {
        static NEXT_TEST_ROOT: AtomicUsize = AtomicUsize::new(0);
        let root = Utf8PathBuf::from(format!(
            "/tmp/djls-environment-discovery-blocked-{}-{}",
            std::process::id(),
            NEXT_TEST_ROOT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(root.join("templates"))
            .expect("templates directory should be created");
        std::fs::write(root.join("settings.py"), "SECRET_KEY = 'x'\n")
            .expect("settings file should be written");
        std::fs::write(
            root.join("templates/index.html"),
            "{% if user %}hi{% endif %}",
        )
        .expect("template file should be written");
        let session = Arc::new(Mutex::new(initialized_session_with_root(root.as_str())));
        let controller = StartupController::new();
        let path = root.join("templates/index.html").to_string();
        let inputs = {
            let mut session = session.lock().await;
            session.open_document(&text_document(&path, 1, "{% if user %}hi{% endif %}"));
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

        let startup = tokio::spawn(run_startup_source_files_with_all_gates(
            Arc::clone(&session),
            inputs,
            None,
            None,
            Some(gate),
        ));
        while !blocked.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        let diagnostics = {
            let session = session.lock().await;
            let db = session.db();
            let file = db.get_or_create_file(&Utf8PathBuf::from(path.as_str()));
            djls_ide::collect_diagnostics(db, file)
        };

        let (lock, cvar) = &*unblock;
        *lock.lock().expect("unblock mutex should not be poisoned") = true;
        cvar.notify_one();
        assert_eq!(startup.await.unwrap(), StartupRunOutcome::Succeeded);
        assert!(diagnostics.is_empty());
    }

    #[tokio::test]
    async fn python_source_models_request_while_running_does_not_wait() {
        static NEXT_TEST_ROOT: AtomicUsize = AtomicUsize::new(0);
        let root = Utf8PathBuf::from(format!(
            "/tmp/djls-python-source-models-blocked-{}-{}",
            std::process::id(),
            NEXT_TEST_ROOT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(root.join("templates"))
            .expect("templates directory should be created");
        std::fs::write(root.join("models.py"), "class Book:\n    pass\n")
            .expect("python file should be written");
        std::fs::write(
            root.join("templates/index.html"),
            "{% if user %}hi{% endif %}",
        )
        .expect("template file should be written");
        let session = Arc::new(Mutex::new(initialized_session_with_root(root.as_str())));
        let controller = StartupController::new();
        let path = root.join("templates/index.html").to_string();
        let inputs = {
            let mut session = session.lock().await;
            session.open_document(&text_document(&path, 1, "{% if user %}hi{% endif %}"));
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

        let startup = tokio::spawn(run_startup_source_files_with_gates(
            Arc::clone(&session),
            inputs,
            None,
            Some(gate),
        ));
        while !blocked.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        let diagnostics = {
            let session = session.lock().await;
            let db = session.db();
            let file = db.get_or_create_file(&Utf8PathBuf::from(path.as_str()));
            djls_ide::collect_diagnostics(db, file)
        };

        let (lock, cvar) = &*unblock;
        *lock.lock().expect("unblock mutex should not be poisoned") = true;
        cvar.notify_one();
        assert_eq!(startup.await.unwrap(), StartupRunOutcome::Succeeded);
        assert!(diagnostics.is_empty());
    }

    #[tokio::test]
    async fn startup_source_files_superseded_reset_stops_before_loading() {
        let root = Utf8PathBuf::from(format!(
            "/tmp/djls-startup-source-files-superseded-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("root directory should be created");
        std::fs::write(root.join("models.py"), "").expect("seed file should be written");
        let session = Arc::new(Mutex::new(initialized_session_with_root(root.as_str())));
        let controller = StartupController::new();
        let before = seed_ready_source_inventory(Arc::clone(&session), &controller).await;
        let inputs = {
            let session = session.lock().await;
            StartupRunInputs::capture(&session, controller.start_generation().await)
        };
        let _newer = controller.start_generation().await;
        let load_count = Arc::new(AtomicUsize::new(0));
        let gate_count = Arc::clone(&load_count);
        let gate = Arc::new(move || {
            gate_count.fetch_add(1, Ordering::SeqCst);
        });

        let outcome =
            run_startup_source_files_with_gate(Arc::clone(&session), inputs, Some(gate)).await;

        assert_eq!(
            outcome,
            StartupRunOutcome::Superseded {
                generation: StartupGeneration(2),
            }
        );
        assert_eq!(load_count.load(Ordering::SeqCst), 0);
        let session = session.lock().await;
        assert_eq!(
            ProjectDb::project(session.db()).source_inventory(session.db()),
            before
        );
    }

    #[tokio::test]
    async fn startup_source_files_stale_document_rejection_leaves_project_facts_unchanged() {
        let root = Utf8PathBuf::from(format!(
            "/tmp/djls-startup-source-files-stale-document-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("templates"))
            .expect("templates directory should be created");
        std::fs::write(root.join("templates/index.html"), "before")
            .expect("seed file should be written");
        let session = Arc::new(Mutex::new(initialized_session_with_root(root.as_str())));
        let controller = StartupController::new();
        let before = seed_ready_source_inventory(Arc::clone(&session), &controller).await;
        let path = root.join("templates/index.html").to_string();
        let inputs = {
            let mut session = session.lock().await;
            session.open_document(&text_document(&path, 1, "before"));
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
        {
            let mut session = session.lock().await;
            session.update_document(
                &versioned_identifier(&path, 2),
                vec![ls_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "after".to_string(),
                }],
            );
        }

        let (lock, cvar) = &*unblock;
        *lock.lock().expect("unblock mutex should not be poisoned") = true;
        cvar.notify_one();

        assert_eq!(startup.await.unwrap(), StartupRunOutcome::Failed);
        let session = session.lock().await;
        assert_eq!(
            ProjectDb::project(session.db()).source_inventory(session.db()),
            before
        );
    }

    #[tokio::test]
    async fn configuration_restart_supersedes_older_apply_without_mutating_project_facts() {
        let root = Utf8PathBuf::from(format!(
            "/tmp/djls-configuration-restart-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("root directory should be created");
        std::fs::write(root.join("models.py"), "").expect("seed file should be written");
        let session = Arc::new(Mutex::new(initialized_session_with_root(root.as_str())));
        let controller = StartupController::new();
        let before = seed_ready_source_inventory(Arc::clone(&session), &controller).await;
        let discovery_before = {
            let session = session.lock().await;
            ProjectDb::project(session.db())
                .discovery(session.db())
                .clone()
        };

        let inputs = {
            let session = session.lock().await;
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
        let _restart = controller.start_generation().await;
        let (lock, cvar) = &*unblock;
        *lock.lock().expect("unblock mutex should not be poisoned") = true;
        cvar.notify_one();

        assert_eq!(
            startup.await.unwrap(),
            StartupRunOutcome::Superseded {
                generation: StartupGeneration(2),
            }
        );
        let session = session.lock().await;
        assert_eq!(
            ProjectDb::project(session.db()).source_inventory(session.db()),
            before
        );
        assert_eq!(
            ProjectDb::project(session.db()).discovery(session.db()),
            &discovery_before
        );
    }

    #[test]
    fn startup_progress_tokens_are_generation_scoped() {
        assert_ne!(
            work_done_progress_token(StartupGeneration(1)),
            work_done_progress_token(StartupGeneration(2)),
        );
        assert_eq!(
            work_done_progress_token(StartupGeneration(42)),
            ls_types::ProgressToken::String("djls/startup/42".to_string()),
        );
    }

    #[test]
    fn work_done_progress_create_failure_suppresses_notifications() {
        let mut machine =
            WorkDoneProgressMachine::new(work_done_progress_token(StartupGeneration(1)));

        assert_eq!(machine.begin(false), None);
        assert_eq!(machine.report("started".to_string()), None);
        assert_eq!(machine.finish("done".to_string()), None);
    }

    #[test]
    fn work_done_progress_success_emits_begin_report_end_in_order() {
        let token = work_done_progress_token(StartupGeneration(7));
        let mut machine = WorkDoneProgressMachine::new(token.clone());

        let begin = machine.begin(true).expect("begin should be emitted");
        assert_eq!(begin.token, token);
        assert!(matches!(
            begin.value,
            ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::Begin(_)),
        ));

        let report = machine
            .report("started".to_string())
            .expect("report should be emitted");
        assert_eq!(report.token, token);
        assert!(matches!(
            report.value,
            ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::Report(_)),
        ));

        let end = machine
            .finish("done".to_string())
            .expect("end should be emitted");
        assert_eq!(end.token, token);
        assert!(matches!(
            end.value,
            ls_types::ProgressParamsValue::WorkDone(ls_types::WorkDoneProgress::End(_)),
        ));
        assert_eq!(machine.finish("again".to_string()), None);
    }

    #[tokio::test]
    async fn python_source_models_startup_progress_reports_lifecycle_over_loading_events() {
        let session = Arc::new(Mutex::new(initialized_session_with_root(
            "/tmp/djls-startup-progress",
        )));
        let controller = StartupController::new();
        let events = Arc::new(StdMutex::new(Vec::new()));
        let inputs = {
            let session = session.lock().await;
            StartupRunInputs::capture_with_progress(
                &session,
                controller.start_generation().await,
                StartupProgress::recording(Arc::clone(&events)),
            )
        };

        let outcome = run_startup_source_files(Arc::clone(&session), inputs).await;

        assert_eq!(outcome, StartupRunOutcome::Succeeded);
        assert_eq!(
            *events
                .lock()
                .expect("startup progress events mutex poisoned"),
            vec![
                StartupProgressEvent::Begin,
                StartupProgressEvent::NodeStarted(NodeId::SourceFileSet),
                StartupProgressEvent::NodeFinished {
                    node: NodeId::SourceFileSet,
                    status: NodeTerminalStatus::Unavailable,
                },
                StartupProgressEvent::NodeStarted(NodeId::ProjectDiscoverySet),
                StartupProgressEvent::NodeFinished {
                    node: NodeId::ProjectDiscoverySet,
                    status: NodeTerminalStatus::Succeeded,
                },
                StartupProgressEvent::NodeStarted(NodeId::PythonSourceModels),
                StartupProgressEvent::NodeFinished {
                    node: NodeId::PythonSourceModels,
                    status: NodeTerminalStatus::Unavailable,
                },
                StartupProgressEvent::NodeStarted(NodeId::EnvironmentDiscovery),
                StartupProgressEvent::NodeFinished {
                    node: NodeId::EnvironmentDiscovery,
                    status: NodeTerminalStatus::Unavailable,
                },
                StartupProgressEvent::Finish(StartupRunOutcome::Succeeded),
            ]
        );
    }

    #[tokio::test]
    async fn startup_progress_finishes_once_for_superseded_run() {
        let session = Arc::new(Mutex::new(initialized_session_with_root(
            "/tmp/djls-startup-progress-superseded",
        )));
        let controller = StartupController::new();
        let events = Arc::new(StdMutex::new(Vec::new()));
        let inputs = {
            let session = session.lock().await;
            StartupRunInputs::capture_with_progress(
                &session,
                controller.start_generation().await,
                StartupProgress::recording(Arc::clone(&events)),
            )
        };
        let _newer = controller.start_generation().await;

        let outcome = run_startup_source_files(Arc::clone(&session), inputs).await;

        assert_eq!(
            outcome,
            StartupRunOutcome::Superseded {
                generation: StartupGeneration(1),
            }
        );
        assert_eq!(
            *events
                .lock()
                .expect("startup progress events mutex poisoned"),
            vec![
                StartupProgressEvent::Begin,
                StartupProgressEvent::Finish(StartupRunOutcome::Superseded {
                    generation: StartupGeneration(1),
                }),
            ]
        );
    }

    #[tokio::test]
    async fn guarded_apply_can_observe_project_facts_without_run_start_mutation() {
        let session = Arc::new(Mutex::new(initialized_session()));
        let controller = StartupController::new();
        let guard = controller.start_generation().await;

        let outcome = guard
            .apply(&session, |session| {
                Ok(ProjectDb::project(session.db()).source_inventory(session.db()))
            })
            .await;

        assert!(matches!(
            outcome,
            ApplyOutcome::Applied(ProjectSourceInventory::Unavailable { .. })
        ));
    }
}
