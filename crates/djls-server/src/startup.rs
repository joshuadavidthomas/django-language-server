use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_project::run_django_discovery;
use djls_project::Db as ProjectDb;
use djls_project::DiscoveryApply;
use djls_project::DiscoveryCancellation;
use djls_project::DiscoveryExecutionOutcome;
use djls_project::DiscoveryHost;
use djls_project::DiscoveryMilestone;
use djls_project::DiscoveryMilestoneStatus;
use djls_project::DiscoveryObservation;
use djls_project::DiscoveryObserver;
use djls_project::DiscoveryRunResult;
use djls_project::DiscoveryStage;
use djls_project::DiscoveryStageStatus;
use djls_project::DjangoDiscoveryRequest;
use djls_project::DjangoEnvironmentCandidatesOutcome;
use djls_project::InstalledAppFileRootsOutcome;
use djls_project::ProjectEnrichment;
use djls_project::ProjectRootDiscoveryApplyResult;
use djls_project::ProjectRootDiscoveryUpdate;
use djls_project::PythonSourceIndexOutcome;
use djls_project::ReadySourceFiles;
use djls_project::SourceFilesApplyResult;
use djls_project::SourceFilesUpdate;
use djls_project::TemplateDirectoryFileRootsOutcome;
use djls_source::File;
use djls_workspace::load_files_for_roots;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;
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

    #[cfg(test)]
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

    #[cfg(test)]
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

#[cfg(test)]
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
pub(crate) struct DjangoDiscoverySnapshot {
    workspace_roots: Vec<Utf8PathBuf>,
    settings: Settings,
    open_documents: Vec<CapturedDocumentSnapshot>,
}

impl DjangoDiscoverySnapshot {
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

    #[cfg(test)]
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
    snapshot: DjangoDiscoverySnapshot,
    guard: GenerationGuard,
    progress: StartupProgress,
}

impl StartupRunInputs {
    #[cfg(test)]
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
            snapshot: DjangoDiscoverySnapshot::capture(session),
            guard,
            progress,
        }
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> &DjangoDiscoverySnapshot {
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

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StartupProgressEvent {
    Begin,
    StageStarted(DiscoveryStage),
    StageFinished {
        stage: DiscoveryStage,
        status: DiscoveryStageStatus,
    },
    MilestoneReached {
        milestone: DiscoveryMilestone,
        status: DiscoveryMilestoneStatus,
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

    fn report_stage_started(&self, handle: &tokio::runtime::Handle, stage: DiscoveryStage) {
        self.reporter.stage_started(handle, stage);
    }

    fn report_stage_finished(
        &self,
        handle: &tokio::runtime::Handle,
        stage: DiscoveryStage,
        status: DiscoveryStageStatus,
    ) {
        self.reporter.stage_finished(handle, stage, status);
    }

    fn report_milestone_reached(
        &self,
        handle: &tokio::runtime::Handle,
        milestone: DiscoveryMilestone,
        status: DiscoveryMilestoneStatus,
    ) {
        self.reporter.milestone_reached(handle, milestone, status);
    }

    fn finish(&self, handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome) {
        self.reporter.finish(handle, outcome);
    }
}

trait StartupProgressReporter: Send + Sync + std::fmt::Debug {
    fn begin(&self, handle: &tokio::runtime::Handle);
    fn stage_started(&self, handle: &tokio::runtime::Handle, stage: DiscoveryStage);
    fn stage_finished(
        &self,
        handle: &tokio::runtime::Handle,
        stage: DiscoveryStage,
        status: DiscoveryStageStatus,
    );
    fn milestone_reached(
        &self,
        handle: &tokio::runtime::Handle,
        milestone: DiscoveryMilestone,
        status: DiscoveryMilestoneStatus,
    );
    fn finish(&self, handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome);
}

#[derive(Debug)]
struct LogStartupProgressReporter;

impl StartupProgressReporter for LogStartupProgressReporter {
    fn begin(&self, _handle: &tokio::runtime::Handle) {
        tracing::info!("Starting Django discovery run");
    }

    fn stage_started(&self, _handle: &tokio::runtime::Handle, stage: DiscoveryStage) {
        tracing::info!(stage = ?stage, "Started Django discovery stage");
    }

    fn stage_finished(
        &self,
        _handle: &tokio::runtime::Handle,
        stage: DiscoveryStage,
        status: DiscoveryStageStatus,
    ) {
        tracing::info!(stage = ?stage, status = ?status, "Finished Django discovery stage");
    }

    fn milestone_reached(
        &self,
        _handle: &tokio::runtime::Handle,
        milestone: DiscoveryMilestone,
        status: DiscoveryMilestoneStatus,
    ) {
        tracing::info!(milestone = ?milestone, status = ?status, "Reached Django discovery milestone");
    }

    fn finish(&self, _handle: &tokio::runtime::Handle, outcome: &StartupRunOutcome) {
        tracing::info!(outcome = ?outcome, "Finished Django discovery run");
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
            StartupRunOutcome::Succeeded => "Django discovery run complete",
            StartupRunOutcome::Failed => "Django discovery run failed",
            StartupRunOutcome::Superseded { .. } => "Django discovery run superseded",
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

    fn stage_started(&self, _handle: &tokio::runtime::Handle, stage: DiscoveryStage) {
        self.send(WorkDoneProgressCommand::Report(format!(
            "Started {stage:?}"
        )));
    }

    fn stage_finished(
        &self,
        _handle: &tokio::runtime::Handle,
        stage: DiscoveryStage,
        status: DiscoveryStageStatus,
    ) {
        self.send(WorkDoneProgressCommand::Report(format!(
            "Finished {stage:?}: {status:?}"
        )));
    }

    fn milestone_reached(
        &self,
        _handle: &tokio::runtime::Handle,
        milestone: DiscoveryMilestone,
        status: DiscoveryMilestoneStatus,
    ) {
        self.send(WorkDoneProgressCommand::Report(format!(
            "Reached {milestone:?}: {status:?}"
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
                    title: "Discovering Django project".to_string(),
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

impl DiscoveryObserver for GuardedStartupProgressObserver {
    fn stage_started(&mut self, stage: DiscoveryStage) {
        if self.guard.is_current() {
            self.progress
                .report_stage_started(&tokio::runtime::Handle::current(), stage);
        }
    }

    fn stage_finished(&mut self, stage: DiscoveryStage, status: DiscoveryStageStatus) {
        if self.guard.is_current() || status == DiscoveryStageStatus::Superseded {
            self.progress
                .report_stage_finished(&tokio::runtime::Handle::current(), stage, status);
        }
    }

    fn milestone_reached(
        &mut self,
        milestone: DiscoveryMilestone,
        status: DiscoveryMilestoneStatus,
    ) {
        if self.guard.is_current() {
            self.progress.report_milestone_reached(
                &tokio::runtime::Handle::current(),
                milestone,
                status,
            );
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

    fn stage_started(&self, _handle: &tokio::runtime::Handle, stage: DiscoveryStage) {
        self.events
            .lock()
            .expect("startup progress events mutex poisoned")
            .push(StartupProgressEvent::StageStarted(stage));
    }

    fn stage_finished(
        &self,
        _handle: &tokio::runtime::Handle,
        stage: DiscoveryStage,
        status: DiscoveryStageStatus,
    ) {
        self.events
            .lock()
            .expect("startup progress events mutex poisoned")
            .push(StartupProgressEvent::StageFinished { stage, status });
    }

    fn milestone_reached(
        &self,
        _handle: &tokio::runtime::Handle,
        milestone: DiscoveryMilestone,
        status: DiscoveryMilestoneStatus,
    ) {
        self.events
            .lock()
            .expect("startup progress events mutex poisoned")
            .push(StartupProgressEvent::MilestoneReached { milestone, status });
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
        let discovery_request = DjangoDiscoveryRequest::new(
            inputs.snapshot().workspace_roots().to_vec(),
            inputs.snapshot().settings().clone(),
        );
        let mut host = LspDiscoveryHost::new(
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
        let result = run_django_discovery(&discovery_request, &mut host, &mut observer);
        let outcome = host.finish(result);
        progress.finish(&handle, &outcome);
        outcome
    })
    .await
    .unwrap_or(StartupRunOutcome::Failed)
}

struct LspDiscoveryHost {
    handle: tokio::runtime::Handle,
    session: Arc<Mutex<Session>>,
    inputs: StartupRunInputs,
    load_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    python_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    environment_observe_gate: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl LspDiscoveryHost {
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
            inputs,
            load_gate,
            python_observe_gate,
            environment_observe_gate,
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn finish(self, result: DiscoveryRunResult) -> StartupRunOutcome {
        if !self.inputs.guard().is_current() {
            return StartupRunOutcome::Superseded {
                generation: self.inputs.guard().generation(),
            };
        }
        match result.execution_outcome() {
            Some(DiscoveryExecutionOutcome::Superseded) => StartupRunOutcome::Superseded {
                generation: self.inputs.guard().generation(),
            },
            Some(DiscoveryExecutionOutcome::StaleSnapshot) => StartupRunOutcome::Failed,
            None => StartupRunOutcome::Succeeded,
        }
    }
}

impl DiscoveryHost for LspDiscoveryHost {
    fn checkpoint(&mut self) -> Result<(), DiscoveryCancellation> {
        if self.inputs.guard().is_current() {
            Ok(())
        } else {
            Err(DiscoveryCancellation::Superseded)
        }
    }

    fn load_files_for_roots(
        &mut self,
        request: FilesForRootsRequest,
    ) -> Result<FilesForRootsResult, DiscoveryCancellation> {
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        if let Some(load_gate) = &self.load_gate {
            load_gate();
        }
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        Ok(load_files_for_roots(request))
    }

    fn current_source_files(&mut self) -> Option<ReadySourceFiles> {
        self.handle.block_on(async {
            let session = self.session.lock().await;
            ProjectDb::project(session.db())
                .source_inventory(session.db())
                .ready()
        })
    }

    fn apply_source_files(
        &mut self,
        update: SourceFilesUpdate,
    ) -> DiscoveryApply<SourceFilesApplyResult> {
        let outcome = self
            .handle
            .block_on(self.inputs.guard().apply(&self.session, |session| {
                if let Some(reason) = self.inputs.snapshot().stale_document_rejection(session) {
                    return Err(reason);
                }
                Ok(session.db_mut().apply_source_files(update))
            }));

        match outcome {
            ApplyOutcome::Applied(applied) => Ok(applied),
            ApplyOutcome::Superseded => Err(DiscoveryExecutionOutcome::Superseded),
            ApplyOutcome::Rejected { .. } => Err(DiscoveryExecutionOutcome::StaleSnapshot),
        }
    }

    fn apply_project_root_discovery(
        &mut self,
        update: ProjectRootDiscoveryUpdate,
    ) -> DiscoveryApply<ProjectRootDiscoveryApplyResult> {
        let outcome = self
            .handle
            .block_on(self.inputs.guard().apply(&self.session, |session| {
                Ok(session.db_mut().apply_project_root_discovery(update))
            }));

        match outcome {
            ApplyOutcome::Applied(applied) => Ok(applied),
            ApplyOutcome::Superseded => Err(DiscoveryExecutionOutcome::Superseded),
            ApplyOutcome::Rejected { .. } => Err(DiscoveryExecutionOutcome::StaleSnapshot),
        }
    }

    fn observe_python_source_index(&mut self) -> DiscoveryObservation<PythonSourceIndexOutcome> {
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        if let Some(gate) = &self.python_observe_gate {
            gate();
        }
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let db = self.handle.block_on(async {
            let session = self.session.lock().await;
            session.project_db_snapshot_for_observation()
        });
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let project = ProjectDb::project(&db);
        let outcome = djls_project::python_source_index(&db, project).clone();
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        Ok(outcome)
    }

    fn observe_django_environment_candidates(
        &mut self,
    ) -> DiscoveryObservation<DjangoEnvironmentCandidatesOutcome> {
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        if let Some(gate) = &self.environment_observe_gate {
            gate();
        }
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let db = self.handle.block_on(async {
            let session = self.session.lock().await;
            session.project_db_snapshot_for_observation()
        });
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let project = ProjectDb::project(&db);
        let outcome = djls_project::django_environment_candidates(&db, project).clone();
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        Ok(outcome)
    }

    fn observe_installed_app_file_roots(
        &mut self,
    ) -> DiscoveryObservation<InstalledAppFileRootsOutcome> {
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let db = self.handle.block_on(async {
            let session = self.session.lock().await;
            session.project_db_snapshot_for_observation()
        });
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let project = ProjectDb::project(&db);
        let discovery = djls_project::installed_app_file_roots_discovery(&db, project);
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        Ok(discovery)
    }

    fn observe_template_directory_file_roots(
        &mut self,
    ) -> DiscoveryObservation<TemplateDirectoryFileRootsOutcome> {
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let db = self.handle.block_on(async {
            let session = self.session.lock().await;
            session.project_db_snapshot_for_observation()
        });
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let project = ProjectDb::project(&db);
        let discovery = djls_project::template_directory_file_roots_discovery(&db, project);
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        Ok(discovery)
    }

    fn load_project_enrichment(&mut self) -> Result<ProjectEnrichment, DiscoveryCancellation> {
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let db = self.handle.block_on(async {
            let session = self.session.lock().await;
            session.project_db_snapshot_for_observation()
        });
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        let enrichment = db.load_project_enrichment();
        if !self.inputs.guard().is_current() {
            return Err(DiscoveryCancellation::Superseded);
        }
        Ok(enrichment)
    }

    fn apply_project_enrichment(
        &mut self,
        enrichment: ProjectEnrichment,
    ) -> DiscoveryApply<ProjectEnrichment> {
        let outcome = self
            .handle
            .block_on(self.inputs.guard().apply(&self.session, |session| {
                Ok(session.db_mut().apply_enrichment(enrichment))
            }));

        match outcome {
            ApplyOutcome::Applied(applied) => Ok(applied),
            ApplyOutcome::Superseded => Err(DiscoveryExecutionOutcome::Superseded),
            ApplyOutcome::Rejected { .. } => Err(DiscoveryExecutionOutcome::StaleSnapshot),
        }
    }
}

#[cfg(test)]
mod startup_generation {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;

    use djls_project::ProjectRootDiscovery;
    use djls_project::SourceFileInventory;
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
    ) -> SourceFileInventory {
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
        assert!(matches!(inventory, SourceFileInventory::Ready(_)));
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
    async fn startup_source_files_runs_source_file_set_through_discovery_run() {
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
            SourceFileInventory::Unavailable { .. }
        ));
        assert!(matches!(
            ProjectDb::project(session.db()).root_discovery(session.db()),
            ProjectRootDiscovery::Ready(_)
        ));
    }

    #[tokio::test]
    async fn startup_request_while_discovery_runs_does_not_wait_for_source_files() {
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
    async fn startup_source_files_superseded_reset_stops_before_discovery() {
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
                .root_discovery(session.db())
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
            ProjectDb::project(session.db()).root_discovery(session.db()),
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
    async fn python_source_models_startup_progress_reports_lifecycle_over_discovery_events() {
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
                StartupProgressEvent::StageStarted(DiscoveryStage::SourceFiles),
                StartupProgressEvent::StageFinished {
                    stage: DiscoveryStage::SourceFiles,
                    status: DiscoveryStageStatus::Unavailable,
                },
                StartupProgressEvent::StageStarted(DiscoveryStage::ProjectRootDiscovery),
                StartupProgressEvent::StageFinished {
                    stage: DiscoveryStage::ProjectRootDiscovery,
                    status: DiscoveryStageStatus::Succeeded,
                },
                StartupProgressEvent::StageStarted(DiscoveryStage::PythonSourceModels),
                StartupProgressEvent::StageFinished {
                    stage: DiscoveryStage::PythonSourceModels,
                    status: DiscoveryStageStatus::Unavailable,
                },
                StartupProgressEvent::StageStarted(DiscoveryStage::DjangoEnvironments),
                StartupProgressEvent::StageFinished {
                    stage: DiscoveryStage::DjangoEnvironments,
                    status: DiscoveryStageStatus::Unavailable,
                },
                StartupProgressEvent::StageStarted(DiscoveryStage::InstalledAppFiles),
                StartupProgressEvent::StageFinished {
                    stage: DiscoveryStage::InstalledAppFiles,
                    status: DiscoveryStageStatus::Unavailable,
                },
                StartupProgressEvent::StageStarted(DiscoveryStage::TemplateDirectoryFiles),
                StartupProgressEvent::StageFinished {
                    stage: DiscoveryStage::TemplateDirectoryFiles,
                    status: DiscoveryStageStatus::Unavailable,
                },
                StartupProgressEvent::StageStarted(DiscoveryStage::Enrichment),
                StartupProgressEvent::StageFinished {
                    stage: DiscoveryStage::Enrichment,
                    status: DiscoveryStageStatus::Unavailable,
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
            ApplyOutcome::Applied(SourceFileInventory::Unavailable { .. })
        ));
    }
}
