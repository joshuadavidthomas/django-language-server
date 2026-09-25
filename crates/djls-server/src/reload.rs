//! Project reload orchestration.
//!
//! Runs expensive reload work off the session lock: load settings on a
//! blocking task, compute project facts on a database clone, apply the results
//! under the lock, then warm derived queries and republish diagnostics from a
//! snapshot.

mod failure_notice;

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::Instant;

use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_ide::PrimedTemplateLibraries;
use djls_ide::WarmCachePart;
use djls_ide::WarmCachePhase;
use djls_ide::prime_template_library_products;
use djls_ide::warm_cache_phases;
use djls_project::Db as ProjectDb;
use djls_project::DjangoEnvironmentData;
use djls_project::EnvironmentPart;
use djls_project::EnvironmentPhase;
use djls_project::Project;
use djls_project::ProjectFactsData;
use djls_project::ProjectFactsPart;
use djls_project::ProjectFactsPhase;
use djls_project::apply_django_environment;
use djls_project::apply_project_facts;
use djls_project::environment_phases;
use djls_project::project_facts_phases;
use salsa::Cancelled;
use tokio::spawn as spawn_task;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinError;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio::task::spawn_blocking;
use tower_lsp_server::Client;
use tracing::Instrument;
use tracing::debug;
use tracing::error;
use tracing::instrument::WithSubscriber;

use crate::client::ClientInfo;
use crate::diagnostics::DiagnosticPublisher;
use crate::logging::blocking_in_span;
use crate::progress::ProgressItem;
use crate::progress::ProgressReporter;
use crate::reload::failure_notice::ReloadFailureNotice;
use crate::session::CancellationRetryAction;
use crate::session::CancellationRetryState;
use crate::session::IntrinsicReadinessState;
use crate::session::ProjectWork;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReloadRunOutcome {
    Complete,
    Cancelled,
    Failed,
    Stale,
    Skipped,
}

impl ReloadRunOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "success",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Stale => "stale",
            Self::Skipped => "skipped",
        }
    }
}

/// Drives full Project reloads and intrinsic-only re-primes off the request path.
///
/// The channel is only a wake-up edge. Pending work is one atomic state where
/// a full reload dominates a queued re-prime and no wake-up can be lost while
/// the single worker is running.
pub(crate) struct ProjectReload {
    tx: mpsc::Sender<()>,
    pending: Arc<AtomicU8>,
    session: Option<Arc<Mutex<Session>>>,
}

impl ProjectReload {
    pub(crate) fn new(
        session: Arc<Mutex<Session>>,
        client: Client,
        diagnostics: DiagnosticPublisher,
    ) -> Self {
        let worker_session = Arc::clone(&session);
        let notice = ReloadFailureNotice::new(&session, client.clone());
        let reload = Self::spawn(move |job| {
            let session = Arc::clone(&worker_session);
            let client = client.clone();
            let diagnostics = diagnostics.clone();
            let notice = notice.clone();
            async move {
                let client_info = { session.lock().await.client_info().clone() };
                let outcome = match job {
                    ProjectWork::FullReload => {
                        reload_project(Arc::clone(&session), client, client_info, diagnostics).await
                    }
                    ProjectWork::Reprime => {
                        reprime_project(Arc::clone(&session), diagnostics).await
                    }
                };
                match outcome {
                    ReloadRunOutcome::Failed => {
                        if let IntrinsicReadinessState::Failed(generation) =
                            session.lock().await.readiness_state()
                        {
                            notice.failed(generation);
                        }
                    }
                    ReloadRunOutcome::Complete => notice.recovered(),
                    ReloadRunOutcome::Cancelled
                    | ReloadRunOutcome::Stale
                    | ReloadRunOutcome::Skipped => {}
                }
                outcome
            }
        });
        Self {
            session: Some(session),
            ..reload
        }
    }

    fn spawn<F, Fut>(runner: F) -> Self
    where
        F: Fn(ProjectWork) -> Fut + Send + 'static,
        Fut: Future<Output = ReloadRunOutcome> + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel(1);
        let pending = Arc::new(AtomicU8::new(PENDING_NONE));
        let worker_pending = Arc::clone(&pending);
        spawn_task(async move {
            while rx.recv().await.is_some() {
                while let Some(job) = take_project_work(&worker_pending) {
                    if rx.is_closed() {
                        return;
                    }
                    let operation = match job {
                        ProjectWork::FullReload => tracing::info_span!(parent: None, "project.reload", ?job, generation = tracing::field::Empty),
                        ProjectWork::Reprime => tracing::debug_span!(parent: None, "project.reprime", ?job, generation = tracing::field::Empty),
                    };
                    let run = operation.in_scope(|| runner(job));
                    let outcome = async move {
                        let outcome = run.await;
                        debug!(outcome = outcome.as_str(), "Project operation finished");
                        outcome
                    }.instrument(operation).await;
                    if outcome == ReloadRunOutcome::Cancelled {
                        merge_project_work(&worker_pending, job);
                    }
                }
            }
        }.with_current_subscriber());

        Self {
            tx,
            pending,
            session: None,
        }
    }

    pub(crate) async fn request_full_reload(&self) {
        if let Some(session) = &self.session {
            let mut session = session.lock().await;
            session.mark_project_changed();
            if matches!(
                session.readiness_state(),
                IntrinsicReadinessState::ReadyWithoutProject
            ) {
                return;
            }
        }
        self.enqueue(ProjectWork::FullReload);
    }

    /// Enqueue work after the session mutation has already advanced the
    /// readiness generation.
    pub(crate) fn request_current(&self, work: ProjectWork) {
        self.enqueue(work);
    }

    #[cfg(test)]
    fn request(&self) {
        self.request_current(ProjectWork::Reprime);
    }

    fn enqueue(&self, work: ProjectWork) {
        enqueue_project_work(&self.pending, &self.tx, work);
    }
}

const PENDING_NONE: u8 = 0;
const PENDING_REPRIME: u8 = 1;
const PENDING_FULL_RELOAD: u8 = 2;

fn merge_project_work(pending: &AtomicU8, requested: ProjectWork) {
    let requested = match requested {
        ProjectWork::Reprime => PENDING_REPRIME,
        ProjectWork::FullReload => PENDING_FULL_RELOAD,
    };
    pending.fetch_max(requested, AtomicOrdering::Release);
}

fn take_project_work(pending: &AtomicU8) -> Option<ProjectWork> {
    match pending.swap(PENDING_NONE, AtomicOrdering::AcqRel) {
        PENDING_REPRIME => Some(ProjectWork::Reprime),
        PENDING_FULL_RELOAD => Some(ProjectWork::FullReload),
        PENDING_NONE | 3..=u8::MAX => None,
    }
}

fn enqueue_project_work(pending: &AtomicU8, tx: &mpsc::Sender<()>, requested: ProjectWork) {
    merge_project_work(pending, requested);
    match tx.try_send(()) {
        Ok(()) | Err(TrySendError::Full(())) => {}
        Err(TrySendError::Closed(())) => {
            error!("project reload worker is gone");
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProgressEnd {
    Complete,
    Skipped,
    Retrying,
    Cancelled,
    Failed,
    Partial,
}

impl ProgressEnd {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Skipped => "skipped",
            Self::Retrying => "retrying",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Partial => "partial",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CountLabel {
    singular: &'static str,
    plural: &'static str,
}

impl From<djls_project::CountLabel> for CountLabel {
    fn from(label: djls_project::CountLabel) -> Self {
        Self {
            singular: label.singular,
            plural: label.plural,
        }
    }
}

impl From<djls_ide::CountLabel> for CountLabel {
    fn from(label: djls_ide::CountLabel) -> Self {
        Self {
            singular: label.singular,
            plural: label.plural,
        }
    }
}

struct DiscoveryJobCount {
    label: CountLabel,
    count: usize,
}

const RESOLVE_ENVIRONMENT_TITLE: &str = "Resolving Django environment";
const DISCOVER_PROJECT_FACTS_TITLE: &str = "Discovering Django project facts";
const WARM_CACHES_TITLE: &str = "Warming Django caches";

async fn reload_project(
    session: Arc<Mutex<Session>>,
    client: Client,
    client_info: ClientInfo,
    diagnostics: DiagnosticPublisher,
) -> ReloadRunOutcome {
    let generation = {
        let session = session.lock().await;
        tracing::Span::current().record("generation", session.desired_generation());
        if session.db().project().is_none() {
            return ReloadRunOutcome::Skipped;
        }
        session.desired_generation()
    };
    let start = Instant::now();
    let progress = ProgressReporter::new(client.clone(), client_info);

    // Start visible progress before touching the session. Clients often send
    // didOpen/completion immediately after initialized; progress setup should
    // not sit behind a session snapshot in that race.
    let mut environment_progress = Some(progress.begin(RESOLVE_ENVIRONMENT_TITLE).await);
    if let Some(progress) = environment_progress.as_ref() {
        progress.report("Resolving environment").await;
    }

    if !load_and_apply_project_settings(&session, &mut environment_progress).await {
        return fail_generation(&session, generation).await;
    }

    let environment =
        match compute_environment(&session, &progress, &mut environment_progress).await {
            StageOutcome::Complete(environment) => environment,
            StageOutcome::Cancelled => return ReloadRunOutcome::Cancelled,
            StageOutcome::Failed => {
                return fail_generation(&session, generation).await;
            }
        };
    if !apply_environment(&session, environment).await {
        finish_progress(&mut environment_progress, ProgressEnd::Skipped).await;
        return ReloadRunOutcome::Skipped;
    }
    finish_progress(&mut environment_progress, ProgressEnd::Complete).await;

    let mut facts_progress = None;
    let facts = match compute_project_facts_data(&session, &progress, &mut facts_progress).await {
        StageOutcome::Complete(facts) => facts,
        StageOutcome::Cancelled => return ReloadRunOutcome::Cancelled,
        StageOutcome::Failed => {
            return fail_generation(&session, generation).await;
        }
    };
    if !apply_facts(&session, &facts).await {
        finish_progress(&mut facts_progress, ProgressEnd::Skipped).await;
        return ReloadRunOutcome::Skipped;
    }
    finish_progress(&mut facts_progress, ProgressEnd::Complete).await;

    let Some(intrinsic_snapshot) = snapshot_session(&session).await else {
        return ReloadRunOutcome::Skipped;
    };
    let primed = match prime_snapshot(intrinsic_snapshot).await {
        StageOutcome::Complete(primed) => primed,
        StageOutcome::Cancelled => return ReloadRunOutcome::Cancelled,
        StageOutcome::Failed => {
            return fail_generation(&session, generation).await;
        }
    };
    if !session
        .lock()
        .await
        .publish_intrinsic_readiness(generation, &primed)
    {
        return ReloadRunOutcome::Stale;
    }

    tracing::info!(
        outcome = "success",
        elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
        library_count = primed.library_count(),
        discovered_file_count = facts.discovered_file_count(),
        "Project reload completed"
    );
    // Readiness is observable as soon as the required intrinsic products are
    // current. The remaining IDE cache warm-up is optional and must not hold
    // project-aware requests behind unrelated work.
    diagnostics.project_ready(&session, generation).await;
    spawn_task(
        warm_snapshot_queries(Arc::clone(&session), progress, generation)
            .instrument(tracing::debug_span!(parent: None, "ide_cache.warmup", generation))
            .with_current_subscriber(),
    );

    ReloadRunOutcome::Complete
}

async fn reprime_project(
    session: Arc<Mutex<Session>>,
    diagnostics: DiagnosticPublisher,
) -> ReloadRunOutcome {
    let snapshot = {
        let session = session.lock().await;
        tracing::Span::current().record("generation", session.desired_generation());
        session.reprime_snapshot()
    };
    let Some((generation, snapshot)) = snapshot else {
        return ReloadRunOutcome::Skipped;
    };
    let start = Instant::now();
    match prime_snapshot(snapshot).await {
        StageOutcome::Complete(primed) => {
            if !session
                .lock()
                .await
                .publish_intrinsic_readiness(generation, &primed)
            {
                return ReloadRunOutcome::Stale;
            }
            debug!(
                outcome = "success",
                elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
                library_count = primed.library_count(),
                "Project re-prime completed"
            );
            diagnostics.project_ready(&session, generation).await;
            ReloadRunOutcome::Complete
        }
        StageOutcome::Cancelled => ReloadRunOutcome::Cancelled,
        StageOutcome::Failed => fail_generation(&session, generation).await,
    }
}

async fn fail_generation(session: &Arc<Mutex<Session>>, generation: u64) -> ReloadRunOutcome {
    if session.lock().await.fail_intrinsic_readiness(generation) {
        ReloadRunOutcome::Failed
    } else {
        ReloadRunOutcome::Stale
    }
}

async fn warm_snapshot_queries(
    session: Arc<Mutex<Session>>,
    progress: ProgressReporter,
    generation: u64,
) {
    let start = Instant::now();
    let warm_progress = progress.begin(WARM_CACHES_TITLE).await;
    for phase in warm_cache_phases() {
        warm_progress.report(phase.progress().message).await;
    }
    // Progress transport is optional and may stall. Only hold storage while computing,
    // and do not let delayed progress warm an obsolete generation.
    let snapshot = {
        let session = session.lock().await;
        (session.readiness_state() == IntrinsicReadinessState::Ready(generation))
            .then(|| session.snapshot())
    };
    let Some(snapshot) = snapshot else {
        debug!(
            outcome = "stale",
            elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
            "IDE cache warm-up skipped"
        );
        warm_progress.finish(ProgressEnd::Cancelled.as_str()).await;
        return;
    };
    let batch = warm_cache_queries(snapshot).await;
    debug!(
        outcome = batch.status.progress_end().as_str(),
        elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
        "IDE cache warm-up finished"
    );
    if batch.status == WarmOutcome::Complete {
        for (index, part) in batch.parts.iter().enumerate() {
            if let Some(count) = part.count() {
                report_warm_summary(&warm_progress, index + 1, part.phase(), count).await;
            }
        }
    }
    warm_progress
        .finish(batch.status.progress_end().as_str())
        .await;
}

async fn load_and_apply_project_settings(
    session: &Arc<Mutex<Session>>,
    progress: &mut Option<ProgressItem>,
) -> bool {
    let settings = match load_project_settings(session).await {
        StageOutcome::Complete(settings) => settings,
        StageOutcome::Cancelled | StageOutcome::Failed => {
            finish_progress(progress, ProgressEnd::Failed).await;
            return false;
        }
    };

    if let Some(progress) = progress.as_ref() {
        progress.report("Applying project settings").await;
    }

    if !apply_project_settings(session, settings).await {
        finish_progress(progress, ProgressEnd::Skipped).await;
        return false;
    }

    true
}

async fn apply_project_settings(session: &Arc<Mutex<Session>>, settings: Settings) -> bool {
    let mut session_lock = session.lock().await;
    let db = session_lock.db_mut();
    if db.project().is_none() {
        return false;
    }

    db.apply_project_settings(settings);
    true
}

async fn apply_environment(
    session: &Arc<Mutex<Session>>,
    environment: DjangoEnvironmentData,
) -> bool {
    let mut session_lock = session.lock().await;
    let db = session_lock.db_mut();
    if db.project().is_none() {
        return false;
    }

    apply_django_environment(db, environment);
    true
}

async fn apply_facts(session: &Arc<Mutex<Session>>, facts: &ProjectFactsData) -> bool {
    let mut session_lock = session.lock().await;
    let db = session_lock.db_mut();
    if db.project().is_none() {
        return false;
    }

    apply_project_facts(db, facts);
    true
}

async fn snapshot_session(session: &Arc<Mutex<Session>>) -> Option<SessionSnapshot> {
    let session_lock = session.lock().await;
    session_lock.db().project()?;
    Some(session_lock.snapshot())
}

async fn load_project_settings(session: &Arc<Mutex<Session>>) -> StageOutcome<Settings> {
    let Some((project_root, config_overrides)) = ({
        let session_lock = session.lock().await;
        let db = session_lock.db();
        db.project().map(|project| {
            (
                project.root(db).clone(),
                session_lock.client_info().config_overrides().clone(),
            )
        })
    }) else {
        debug!(
            outcome = "skipped",
            "No project configured for settings load"
        );
        return StageOutcome::Failed;
    };

    let joined = spawn_blocking(blocking_in_span(
        tracing::debug_span!("project.settings_load"),
        move || {
            let result = Settings::new(&project_root, Some(config_overrides));
            debug!(
                outcome = if result.is_ok() { "success" } else { "failed" },
                "Settings load finished"
            );
            result
        },
    ))
    .await;
    let settings = match classify_child_task_join(joined) {
        ChildTaskJoin::Complete(settings) => settings,
        ChildTaskJoin::Failed(error) => {
            error!(
                panicked = error.is_panic(),
                outcome = "failed",
                "Project settings load task failed"
            );
            debug!(?error, "Task failure detail");
            return StageOutcome::Failed;
        }
    };

    match settings {
        Ok(settings) => StageOutcome::Complete(settings),
        Err(error) => {
            // `ConfigError`'s message names only the failing stage; its source
            // carries the key, value, and file detail.
            error!(%error, outcome = "failed", "Error loading project settings");
            debug!(?error, "Project settings error detail");
            StageOutcome::Failed
        }
    }
}

type EnvironmentJobResult = Result<EnvironmentPart, Cancelled>;
type ProjectFactsJobResult = Result<ProjectFactsPart, Cancelled>;

#[derive(Debug)]
enum StageOutcome<T> {
    Complete(T),
    Cancelled,
    Failed,
}

#[derive(Debug)]
enum ChildTaskJoin<T> {
    Complete(T),
    Failed(JoinError),
}

fn classify_child_task_join<T>(joined: Result<T, JoinError>) -> ChildTaskJoin<T> {
    match joined {
        Ok(value) => ChildTaskJoin::Complete(value),
        Err(error) => ChildTaskJoin::Failed(error),
    }
}

async fn compute_environment(
    session: &Arc<Mutex<Session>>,
    reporter: &ProgressReporter,
    progress: &mut Option<ProgressItem>,
) -> StageOutcome<DjangoEnvironmentData> {
    let mut retry_state = CancellationRetryState::new();
    loop {
        // Announce before capturing storage: progress transport may stall while
        // mutations need every database clone to be released.
        for phase in environment_phases() {
            report_environment_phase(phase, reporter, progress).await;
        }
        let Some((compute_db, project)) = capture_discovery_db(session).await else {
            finish_progress(progress, ProgressEnd::Skipped).await;
            return StageOutcome::Failed;
        };

        let mut jobs: JoinSet<EnvironmentJobResult> = JoinSet::new();
        for phase in environment_phases() {
            let db = compute_db.clone();
            jobs.spawn_blocking(blocking_in_span(
                tracing::debug_span!("project.django_environment", ?phase),
                move || {
                    let result = Cancelled::catch(AssertUnwindSafe(|| phase.run(&db, project)));
                    debug!(
                        outcome = if result.is_ok() {
                            "success"
                        } else {
                            "cancelled"
                        },
                        count = result.as_ref().ok().map(EnvironmentPart::count),
                        "Environment phase finished"
                    );
                    result
                },
            ));
        }
        // Only workers retain storage; the coordinator may await progress during collection.
        drop(compute_db);

        let result = collect_environment_jobs(jobs, progress.as_ref()).await;
        match result {
            StageOutcome::Complete(environment) => return StageOutcome::Complete(environment),
            StageOutcome::Cancelled => match retry_state.after_cancellation() {
                CancellationRetryAction::Retry { attempt } => {
                    finish_progress(progress, ProgressEnd::Retrying).await;
                    debug!(
                        attempt,
                        "Environment compute cancelled; retrying with fresh database clone"
                    );
                }
                CancellationRetryAction::Exhausted => {
                    finish_progress(progress, ProgressEnd::Cancelled).await;
                    debug!(
                        retries = SNAPSHOT_CANCEL_RETRIES,
                        "Environment compute cancelled repeatedly; project reload cancelled"
                    );
                    return StageOutcome::Cancelled;
                }
            },
            StageOutcome::Failed => {
                finish_progress(progress, ProgressEnd::Failed).await;
                return StageOutcome::Failed;
            }
        }
    }
}

async fn collect_environment_jobs(
    mut jobs: JoinSet<EnvironmentJobResult>,
    progress: Option<&ProgressItem>,
) -> StageOutcome<DjangoEnvironmentData> {
    let mut cancellation = None;
    let mut failed = false;
    let mut parts = Vec::new();
    let mut done = 0;
    let total = environment_phases().count();

    while let Some(joined) = jobs.join_next().await {
        match classify_child_task_join(joined) {
            ChildTaskJoin::Complete(Ok(part)) => {
                done += 1;
                let phase_progress = part.phase().progress();
                report_count(
                    progress,
                    done,
                    total,
                    part.count(),
                    phase_progress.count_label.into(),
                )
                .await;
                parts.push(part);
            }
            ChildTaskJoin::Complete(Err(cancelled)) => {
                remember_cancellation(&mut cancellation, cancelled);
            }
            ChildTaskJoin::Failed(error) => {
                failed = true;
                error!(
                    panicked = error.is_panic(),
                    outcome = "failed",
                    "Django Environment phase task failed"
                );
                debug!(?error, "Task failure detail");
            }
        }
    }
    if failed {
        StageOutcome::Failed
    } else if cancellation.is_some() {
        StageOutcome::Cancelled
    } else {
        match DjangoEnvironmentData::assemble(parts) {
            Ok(environment) => StageOutcome::Complete(environment),
            Err(error) => {
                error!(%error, outcome = "failed", "Django environment assembly failed");
                StageOutcome::Failed
            }
        }
    }
}

async fn compute_project_facts_data(
    session: &Arc<Mutex<Session>>,
    reporter: &ProgressReporter,
    progress: &mut Option<ProgressItem>,
) -> StageOutcome<ProjectFactsData> {
    // Capture happens after Environment application, so every attempt observes
    // registered, rescanned roots and overlay-authoritative file contents.
    let mut retry_state = CancellationRetryState::new();
    loop {
        // Keep progress waits outside the lifetime of the captured database.
        for phase in project_facts_phases() {
            report_project_facts_phase(phase, reporter, progress).await;
        }
        let Some((compute_db, project)) = capture_discovery_db(session).await else {
            finish_progress(progress, ProgressEnd::Skipped).await;
            return StageOutcome::Failed;
        };

        let mut jobs: JoinSet<ProjectFactsJobResult> = JoinSet::new();
        for phase in project_facts_phases() {
            let db = compute_db.clone();
            jobs.spawn_blocking(blocking_in_span(
                tracing::debug_span!("project.facts", ?phase),
                move || {
                    let result = Cancelled::catch(AssertUnwindSafe(|| phase.run(&db, project)));
                    debug!(
                        outcome = if result.is_ok() {
                            "success"
                        } else {
                            "cancelled"
                        },
                        count = result.as_ref().ok().map(ProjectFactsPart::count),
                        "Project Facts phase finished"
                    );
                    result
                },
            ));
        }
        // Collection/reporting must not keep the coordinator's storage clone alive.
        drop(compute_db);

        let result = collect_project_facts_jobs(jobs, progress.as_ref()).await;
        match result {
            StageOutcome::Complete(facts) => return StageOutcome::Complete(facts),
            StageOutcome::Cancelled => match retry_state.after_cancellation() {
                CancellationRetryAction::Retry { attempt } => {
                    finish_progress(progress, ProgressEnd::Retrying).await;
                    debug!(
                        attempt,
                        "Project Facts compute cancelled; retrying with fresh database clone"
                    );
                }
                CancellationRetryAction::Exhausted => {
                    finish_progress(progress, ProgressEnd::Cancelled).await;
                    debug!(
                        retries = SNAPSHOT_CANCEL_RETRIES,
                        "Project Facts compute cancelled repeatedly; project reload cancelled"
                    );
                    return StageOutcome::Cancelled;
                }
            },
            StageOutcome::Failed => {
                finish_progress(progress, ProgressEnd::Failed).await;
                return StageOutcome::Failed;
            }
        }
    }
}

async fn collect_project_facts_jobs(
    mut jobs: JoinSet<ProjectFactsJobResult>,
    progress: Option<&ProgressItem>,
) -> StageOutcome<ProjectFactsData> {
    let mut cancellation = None;
    let mut failed = false;
    let mut counts = Vec::new();
    let mut parts = Vec::new();

    while let Some(joined) = jobs.join_next().await {
        match classify_child_task_join(joined) {
            ChildTaskJoin::Complete(Ok(part)) => {
                let phase = part.phase();
                counts.push(DiscoveryJobCount {
                    label: phase.progress().count_label.into(),
                    count: part.count(),
                });
                parts.push(part);
            }
            ChildTaskJoin::Complete(Err(cancelled)) => {
                remember_cancellation(&mut cancellation, cancelled);
            }
            ChildTaskJoin::Failed(error) => {
                failed = true;
                error!(
                    panicked = error.is_panic(),
                    outcome = "failed",
                    "Project Facts phase task failed"
                );
                debug!(?error, "Task failure detail");
            }
        }
    }
    if failed {
        return StageOutcome::Failed;
    }
    if cancellation.is_some() {
        return StageOutcome::Cancelled;
    }

    let facts = ProjectFactsData::assemble(parts);
    let total = project_facts_phases().count() + 1;
    for (index, summary) in counts.into_iter().enumerate() {
        report_count(progress, index + 1, total, summary.count, summary.label).await;
    }
    report_count(
        progress,
        total,
        total,
        facts.discovered_file_count(),
        ProjectFactsData::discovered_file_count_label().into(),
    )
    .await;
    StageOutcome::Complete(facts)
}

async fn capture_discovery_db(session: &Arc<Mutex<Session>>) -> Option<(DjangoDatabase, Project)> {
    let session_lock = session.lock().await;
    let db = session_lock.db();
    let Some(project) = db.project() else {
        debug!(outcome = "skipped", "No project configured for discovery");
        return None;
    };
    Some((db.clone(), project))
}

async fn report_environment_phase(
    phase: EnvironmentPhase,
    reporter: &ProgressReporter,
    progress: &mut Option<ProgressItem>,
) {
    report_discovery_phase(
        phase.progress().message,
        reporter,
        progress,
        RESOLVE_ENVIRONMENT_TITLE,
    )
    .await;
}

async fn report_project_facts_phase(
    phase: ProjectFactsPhase,
    reporter: &ProgressReporter,
    progress: &mut Option<ProgressItem>,
) {
    report_discovery_phase(
        phase.progress().message,
        reporter,
        progress,
        DISCOVER_PROJECT_FACTS_TITLE,
    )
    .await;
}

async fn report_discovery_phase(
    message: &str,
    reporter: &ProgressReporter,
    progress: &mut Option<ProgressItem>,
    title: &'static str,
) {
    if progress.is_none() {
        *progress = Some(reporter.begin(title).await);
    }
    if let Some(progress) = progress.as_ref() {
        progress.report(message).await;
    }
}

async fn report_count(
    progress: Option<&ProgressItem>,
    done: usize,
    total: usize,
    count: usize,
    label: CountLabel,
) {
    let message = count_message(count, label);
    if let Some(progress) = progress {
        progress.report_fraction(done, total, &message).await;
    }
}

fn count_message(count: usize, label: CountLabel) -> String {
    let unit = if count == 1 {
        label.singular
    } else {
        label.plural
    };
    format!("{count} {unit}")
}

fn remember_cancellation(cancellation: &mut Option<Cancelled>, cancelled: Cancelled) {
    if cancellation.is_none() {
        *cancellation = Some(cancelled);
    }
}

async fn finish_progress(progress: &mut Option<ProgressItem>, end: ProgressEnd) {
    if let Some(progress) = progress.take() {
        progress.finish(end.as_str()).await;
    }
}

type WarmJobResult = Result<WarmCachePart, Cancelled>;
type WarmJobHandle = JoinHandle<WarmJobResult>;

async fn prime_snapshot(snapshot: SessionSnapshot) -> StageOutcome<PrimedTemplateLibraries> {
    let joined = spawn_blocking(blocking_in_span(
        tracing::debug_span!("project.intrinsic_priming"),
        move || {
            let result = Cancelled::catch(AssertUnwindSafe(|| {
                prime_template_library_products(snapshot.db())
            }));
            debug!(
                outcome = match &result {
                    Ok(Some(_)) => "success",
                    Ok(None) => "skipped",
                    Err(_) => "cancelled",
                },
                count = result
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .map(PrimedTemplateLibraries::library_count),
                "Intrinsic priming finished"
            );
            result
        },
    ))
    .await;

    classify_prime_task_join(joined)
}

fn classify_prime_task_join(
    joined: Result<Result<Option<PrimedTemplateLibraries>, Cancelled>, JoinError>,
) -> StageOutcome<PrimedTemplateLibraries> {
    match classify_child_task_join(joined) {
        ChildTaskJoin::Complete(Ok(Some(primed))) => StageOutcome::Complete(primed),
        ChildTaskJoin::Complete(Ok(None)) => StageOutcome::Failed,
        ChildTaskJoin::Complete(Err(cancelled)) => {
            debug!(?cancelled, "Template Library priming cancelled");
            StageOutcome::Cancelled
        }
        ChildTaskJoin::Failed(error) => {
            error!(
                panicked = error.is_panic(),
                outcome = "failed",
                "Template Library priming task failed"
            );
            debug!(?error, "Task failure detail");
            StageOutcome::Failed
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WarmOutcome {
    Complete,
    Partial,
}

impl WarmOutcome {
    const fn progress_end(self) -> ProgressEnd {
        match self {
            Self::Complete => ProgressEnd::Complete,
            Self::Partial => ProgressEnd::Partial,
        }
    }
}

fn spawn_warm_cache_job(phase: WarmCachePhase, snapshot: SessionSnapshot) -> WarmJobHandle {
    spawn_blocking(blocking_in_span(
        tracing::debug_span!("ide_cache.phase", ?phase),
        move || {
            let result = Cancelled::catch(AssertUnwindSafe(|| phase.run(snapshot.db())));
            debug!(
                outcome = if result.is_ok() {
                    "success"
                } else {
                    "cancelled"
                },
                count = result.as_ref().ok().and_then(WarmCachePart::count),
                "IDE cache phase finished"
            );
            result
        },
    ))
}

async fn join_warm_cache_job(
    phase: WarmCachePhase,
    handle: WarmJobHandle,
) -> StageOutcome<WarmCachePart> {
    match classify_child_task_join(handle.await) {
        ChildTaskJoin::Complete(Ok(part)) => StageOutcome::Complete(part),
        ChildTaskJoin::Complete(Err(cancelled)) => {
            debug!(
                ?cancelled,
                ?phase,
                "IDE cache warm-up cancelled; newer inputs will re-warm queries"
            );
            StageOutcome::Cancelled
        }
        ChildTaskJoin::Failed(error) => {
            error!(
                panicked = error.is_panic(),
                ?phase,
                outcome = "failed",
                "IDE cache warm-up task failed"
            );
            debug!(?error, "Task failure detail");
            StageOutcome::Failed
        }
    }
}

struct WarmBatchOutcome {
    status: WarmOutcome,
    parts: Vec<WarmCachePart>,
}

async fn collect_warm_cache_jobs(
    handles: Vec<(WarmCachePhase, WarmJobHandle)>,
) -> WarmBatchOutcome {
    let mut parts = Vec::new();
    let mut status = WarmOutcome::Complete;
    for (phase, handle) in handles {
        match join_warm_cache_job(phase, handle).await {
            StageOutcome::Complete(part) => parts.push(part),
            StageOutcome::Cancelled | StageOutcome::Failed => {
                status = WarmOutcome::Partial;
            }
        }
    }

    WarmBatchOutcome { status, parts }
}

async fn warm_cache_queries(snapshot: SessionSnapshot) -> WarmBatchOutcome {
    let mut handles = Vec::new();
    for phase in warm_cache_phases().iter().copied() {
        handles.push((phase, spawn_warm_cache_job(phase, snapshot.clone())));
    }
    drop(snapshot);

    collect_warm_cache_jobs(handles).await
}

async fn report_warm_summary(
    progress: &ProgressItem,
    done: usize,
    phase: WarmCachePhase,
    count: usize,
) {
    let Some(label) = phase.progress().count_label else {
        return;
    };

    report_count(
        Some(progress),
        done,
        warm_cache_phases().len(),
        count,
        label.into(),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use camino::Utf8PathBuf;
    use futures_util::SinkExt;
    use futures_util::StreamExt;
    use tempfile::tempdir;
    use tokio::spawn as spawn_task;
    use tokio::sync::Notify;
    use tokio::sync::oneshot;
    use tokio::sync::oneshot::error::TryRecvError;
    use tokio::task::spawn_blocking;
    use tokio::task::yield_now;
    use tokio::time::timeout;
    use tower_lsp_server::LanguageServer;
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc;
    use tower_lsp_server::ls_types;
    use tower_service::Service;

    use super::*;

    struct TransportBackend(Client);

    impl LanguageServer for TransportBackend {
        async fn initialize(
            &self,
            _: ls_types::InitializeParams,
        ) -> jsonrpc::Result<ls_types::InitializeResult> {
            Ok(ls_types::InitializeResult::default())
        }

        async fn shutdown(&self) -> jsonrpc::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn failure_outcome_requires_current_generation_and_is_accepted_once() {
        let session = Arc::new(Mutex::new(Session::default()));
        assert_eq!(fail_generation(&session, 1).await, ReloadRunOutcome::Stale);
        assert_eq!(fail_generation(&session, 0).await, ReloadRunOutcome::Failed);
        assert_eq!(fail_generation(&session, 0).await, ReloadRunOutcome::Stale);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_phases_inherit_scoped_dispatcher_and_operation() {
        let log = tempfile::NamedTempFile::new().expect("log file");
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(StdMutex::new(log.reopen().expect("log writer")))
            .finish();
        async {
            let operation = tracing::info_span!("project.reload", generation = 73);
            async {
                let mut jobs = JoinSet::new();
                jobs.spawn_blocking(blocking_in_span(
                    tracing::debug_span!("project.django_environment", phase = "test"),
                    || {
                        debug!(count = 7, "phase evidence");
                    },
                ));
                jobs.join_next()
                    .await
                    .expect("phase job")
                    .expect("phase result");
                spawn_warm_cache_job(
                    WarmCachePhase::ResolveTemplateDirs,
                    Session::default().snapshot(),
                )
                .await
                .expect("warm job")
                .expect("warm result");
                prime_snapshot(Session::default().snapshot()).await;
            }
            .instrument(operation)
            .await;
        }
        .with_subscriber(subscriber)
        .await;
        let output = std::fs::read_to_string(log.path()).expect("read log");
        for line in output
            .lines()
            .filter(|line| line.contains("phase evidence") || line.contains("compute_completed"))
        {
            assert!(line.contains("project.reload{generation=73}"), "{line}");
        }
        assert!(output.contains("phase evidence count=7"), "{output}");
        assert!(output.contains("ide_cache.phase"), "{output}");
        assert!(output.contains("project.intrinsic_priming"), "{output}");
    }

    #[tokio::test]
    async fn dequeued_jobs_have_distinct_operation_spans() {
        async {
            let (tx, mut rx) = mpsc::channel(2);
            let reload = ProjectReload::spawn(move |_| {
                let tx = tx.clone();
                async move {
                    tx.send(tracing::Span::current())
                        .await
                        .expect("observed operation");
                    ReloadRunOutcome::Complete
                }
            });
            reload.request_current(ProjectWork::FullReload);
            let full = rx.recv().await.expect("full reload span");
            reload.request_current(ProjectWork::Reprime);
            let reprime = rx.recv().await.expect("re-prime span");
            assert_eq!(
                full.metadata().expect("full metadata").name(),
                "project.reload"
            );
            assert_eq!(
                reprime.metadata().expect("re-prime metadata").name(),
                "project.reprime"
            );
            assert_ne!(full.id(), reprime.id());
        }
        .with_subscriber(tracing_subscriber::registry())
        .await;
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Keep the progress handshake and readiness assertions together.
    async fn readiness_summary_precedes_stalled_warmup_and_warmup_has_separate_context() {
        let root = tempdir().expect("project root");
        let log = tempfile::NamedTempFile::new().expect("log file");
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter("off,djls_server::reload=debug,salsa::function::execute=info")
            .with_writer(StdMutex::new(log.reopen().expect("log writer")))
            .finish();
        let (mut service, socket) = LspService::new(TransportBackend);
        service
            .call(
                jsonrpc::Request::build("initialize")
                    .params(serde_json::json!({"capabilities": {}}))
                    .id(1_i64)
                    .finish(),
            )
            .await
            .expect("initialize");
        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.path()).expect("root URI"),
                name: "private-workspace-canary".into(),
            }]),
            capabilities: ls_types::ClientCapabilities {
                window: Some(ls_types::WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let session = Arc::new(Mutex::new(Session::new(&params)));
        let client = service.inner().0.clone();
        let diagnostics = DiagnosticPublisher::new(Arc::clone(&session), client.clone());
        let client_info = session.lock().await.client_info().clone();
        let (mut requests, mut responses) = socket.split();
        let observe = async {
            let mut creates = 0;
            while let Some(message) = requests.next().await {
                if message.method() == "window/workDoneProgress/create" {
                    creates += 1;
                    if creates == 3 {
                        assert_eq!(
                            session.lock().await.readiness_state(),
                            IntrinsicReadinessState::Ready(0)
                        );
                        let before =
                            std::fs::read_to_string(log.path()).expect("read readiness log");
                        assert_eq!(before.matches("Project reload completed").count(), 1);
                        assert!(!before.contains("IDE cache warm-up finished"));
                        // Telemetry must not force optional indexing, Models,
                        // or per-Template work just to produce summary counts.
                        assert!(before.contains("template_library_catalog("));
                        for query in ["template_resolution(", "model_graph(", "parse_template("] {
                            assert!(!before.contains(query), "unexpected eager query: {query}");
                        }
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        assert_eq!(
                            std::fs::read_to_string(log.path()).expect("read stalled log"),
                            before
                        );
                    }
                    responses
                        .send(jsonrpc::Response::from_ok(
                            message.id().expect("create id").clone(),
                            serde_json::Value::Null,
                        ))
                        .await
                        .expect("create response");
                } else if creates == 3
                    && message.method() == "$/progress"
                    && message.params().expect("progress params")["value"]["kind"] == "end"
                {
                    break;
                }
            }
            assert_eq!(creates, 3);
        };
        let run = async {
            reload_project(Arc::clone(&session), client, client_info, diagnostics)
                .instrument(tracing::info_span!(
                    "project.reload",
                    generation = tracing::field::Empty
                ))
                .await
        }
        .with_subscriber(subscriber);
        let (outcome, ()) = timeout(Duration::from_secs(30), async {
            tokio::join!(run, observe)
        })
        .await
        .expect("reload and warm-up complete");
        assert_eq!(outcome, ReloadRunOutcome::Complete);
        let output = std::fs::read_to_string(log.path()).expect("read log");
        assert_eq!(
            output
                .lines()
                .filter(|line| line.contains(" INFO ") && line.contains("djls_server::reload:"))
                .count(),
            1,
            "{output}"
        );
        let warm = output
            .lines()
            .find(|line| line.contains("IDE cache warm-up finished"))
            .expect("warm summary");
        assert!(warm.contains("ide_cache.warmup{generation=0}"), "{warm}");
        assert!(!warm.contains("project.reload"), "{warm}");
        assert!(!output.contains("private-workspace-canary"), "{output}");
        assert!(
            !output.contains(root.path().to_str().expect("root path")),
            "{output}"
        );
    }

    struct DropProbe {
        dropped: Option<oneshot::Sender<()>>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            if let Some(dropped) = self.dropped.take() {
                match dropped.send(()) {
                    Ok(()) | Err(()) => {}
                }
            }
        }
    }

    #[tokio::test]
    async fn idle_reload_worker_drops_runner_capture() {
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let runner_probe = Arc::new(DropProbe {
            dropped: Some(dropped_tx),
        });
        let reload = ProjectReload::spawn(move |_| {
            let runner_probe = Arc::clone(&runner_probe);
            async move {
                let _runner_probe = runner_probe;
                ReloadRunOutcome::Complete
            }
        });

        drop(reload);

        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("idle reload worker should terminate when its owner is dropped")
            .expect("reload worker should drop its runner capture");
    }

    #[tokio::test]
    async fn active_reload_worker_drops_after_current_run_without_followup() {
        let run_count = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let (release_tx, release_rx) = oneshot::channel();
        let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
        let (dropped_tx, mut dropped_rx) = oneshot::channel();
        let runner_probe = Arc::new(DropProbe {
            dropped: Some(dropped_tx),
        });
        let reload = ProjectReload::spawn({
            let run_count = Arc::clone(&run_count);
            let started = Arc::clone(&started);
            let release_rx = Arc::clone(&release_rx);
            move |_| {
                let run_count = Arc::clone(&run_count);
                let started = Arc::clone(&started);
                let release_rx = Arc::clone(&release_rx);
                let runner_probe = Arc::clone(&runner_probe);
                async move {
                    let _runner_probe = runner_probe;
                    let run = run_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if run == 1 {
                        started.notify_one();
                        let release = release_rx
                            .lock()
                            .expect("active reload release mutex should not be poisoned")
                            .take()
                            .expect("active reload owns release receiver");
                        drop(release.await);
                    }
                    ReloadRunOutcome::Complete
                }
            }
        });

        reload.request();
        timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("reload should start");
        reload.request();
        drop(reload);

        assert!(matches!(dropped_rx.try_recv(), Err(TryRecvError::Empty)));
        release_tx
            .send(())
            .expect("active reload release receiver should remain live");
        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("reload worker should terminate after its active run completes")
            .expect("reload worker should drop its runner capture");
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_active_reload_worker_drops_without_retry() {
        let run_count = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let (release_tx, release_rx) = oneshot::channel();
        let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
        let (dropped_tx, mut dropped_rx) = oneshot::channel();
        let runner_probe = Arc::new(DropProbe {
            dropped: Some(dropped_tx),
        });
        let reload = ProjectReload::spawn({
            let run_count = Arc::clone(&run_count);
            let started = Arc::clone(&started);
            let release_rx = Arc::clone(&release_rx);
            move |_| {
                let run_count = Arc::clone(&run_count);
                let started = Arc::clone(&started);
                let release_rx = Arc::clone(&release_rx);
                let runner_probe = Arc::clone(&runner_probe);
                async move {
                    let _runner_probe = runner_probe;
                    let run = run_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if run == 1 {
                        started.notify_one();
                        let release = release_rx
                            .lock()
                            .expect("cancelled reload release mutex should not be poisoned")
                            .take()
                            .expect("cancelled reload owns release receiver");
                        drop(release.await);
                        ReloadRunOutcome::Cancelled
                    } else {
                        ReloadRunOutcome::Complete
                    }
                }
            }
        });

        reload.request();
        timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("reload should start");
        drop(reload);

        assert!(matches!(dropped_rx.try_recv(), Err(TryRecvError::Empty)));
        release_tx
            .send(())
            .expect("cancelled reload release receiver should remain live");
        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("cancelled reload worker should terminate after owner drop")
            .expect("reload worker should drop its runner capture");
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn request_runs_one_reload() {
        let run_count = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(Notify::new());
        let reload = ProjectReload::spawn({
            let run_count = Arc::clone(&run_count);
            let completed = Arc::clone(&completed);
            move |_| {
                let run_count = Arc::clone(&run_count);
                let completed = Arc::clone(&completed);
                async move {
                    run_count.fetch_add(1, Ordering::SeqCst);
                    completed.notify_one();
                    ReloadRunOutcome::Complete
                }
            }
        });

        reload.request();

        timeout(Duration::from_secs(1), completed.notified())
            .await
            .expect("reload should complete before the test timeout");
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn panicking_critical_child_fails_generation_without_killing_reload_worker() {
        let session = Arc::new(Mutex::new(Session::default()));
        let mut readiness = session.lock().await.readiness_receiver();
        let run_count = Arc::new(AtomicUsize::new(0));
        let (completed_tx, mut completed_rx) = mpsc::unbounded_channel();
        let reload = ProjectReload::spawn({
            let session = Arc::clone(&session);
            let run_count = Arc::clone(&run_count);
            move |_| {
                let session = Arc::clone(&session);
                let run_count = Arc::clone(&run_count);
                let completed_tx = completed_tx.clone();
                async move {
                    let run = run_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if run == 1 {
                        let joined = spawn_task(async {
                            panic!("synthetic critical child panic");
                            #[allow(unreachable_code)]
                            Ok::<Option<PrimedTemplateLibraries>, Cancelled>(None)
                        })
                        .await;
                        assert!(matches!(
                            classify_prime_task_join(joined),
                            StageOutcome::Failed
                        ));
                        assert_eq!(fail_generation(&session, 0).await, ReloadRunOutcome::Failed);
                    }
                    completed_tx
                        .send(run)
                        .expect("reload completion receiver should remain live");
                    // Critical child failures complete the run after publishing
                    // Failed; only Salsa cancellation is automatically retried.
                    ReloadRunOutcome::Complete
                }
            }
        });

        reload.request();
        assert_eq!(
            timeout(Duration::from_secs(1), completed_rx.recv())
                .await
                .expect("first reload should complete before the test timeout"),
            Some(1)
        );
        readiness
            .changed()
            .await
            .expect("reload worker should publish a readiness update");
        assert_eq!(
            *readiness.borrow_and_update(),
            IntrinsicReadinessState::Failed(0)
        );

        reload.request();
        assert_eq!(
            timeout(Duration::from_secs(1), completed_rx.recv())
                .await
                .expect("second reload should complete before the test timeout"),
            Some(2)
        );
        assert_eq!(run_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn project_settings_task_panic_is_classified_as_failure() {
        let joined: Result<(), JoinError> = spawn_task(async {
            panic!("synthetic settings panic");
        })
        .await;

        assert!(matches!(
            classify_child_task_join(joined),
            ChildTaskJoin::Failed(_)
        ));
    }

    #[tokio::test]
    async fn environment_phase_panic_is_classified_as_failure() {
        let mut jobs = JoinSet::new();
        jobs.spawn(async {
            panic!("synthetic Environment panic");
        });

        assert!(matches!(
            collect_environment_jobs(jobs, None).await,
            StageOutcome::Failed
        ));
    }

    #[tokio::test]
    async fn missing_environment_phase_is_classified_as_failure() {
        assert!(matches!(
            collect_environment_jobs(JoinSet::new(), None).await,
            StageOutcome::Failed
        ));
    }

    #[tokio::test]
    async fn project_facts_phase_panic_is_classified_as_failure() {
        let mut jobs = JoinSet::new();
        jobs.spawn(async {
            panic!("synthetic Project Facts panic");
        });

        assert!(matches!(
            collect_project_facts_jobs(jobs, None).await,
            StageOutcome::Failed
        ));
    }

    #[tokio::test]
    async fn intrinsic_priming_task_panic_is_classified_as_failure() {
        let joined = spawn_task(async {
            panic!("synthetic intrinsic priming panic");
            #[allow(unreachable_code)]
            Ok::<Option<PrimedTemplateLibraries>, Cancelled>(None)
        })
        .await;

        assert!(matches!(
            classify_prime_task_join(joined),
            StageOutcome::Failed
        ));
    }

    #[tokio::test]
    async fn intrinsic_priming_salsa_cancellation_is_not_classified_as_failure() {
        let joined =
            spawn_task(async { Err::<Option<PrimedTemplateLibraries>, _>(Cancelled::Local) }).await;

        assert!(matches!(
            classify_prime_task_join(joined),
            StageOutcome::Cancelled
        ));
    }

    #[tokio::test]
    async fn warm_cache_batch_retains_phase_order() {
        let batch = warm_cache_queries(Session::default().snapshot()).await;

        assert_eq!(batch.status, WarmOutcome::Complete);
        assert_eq!(
            batch
                .parts
                .iter()
                .map(WarmCachePart::phase)
                .collect::<Vec<_>>(),
            [
                WarmCachePhase::ResolveTemplateDirs,
                WarmCachePhase::IndexTemplateLibraries,
                WarmCachePhase::IndexTemplates,
            ]
        );
    }

    #[tokio::test]
    async fn warm_cache_panic_in_mixed_batch_is_partial_and_retains_successful_sibling() {
        let failed: WarmJobHandle = spawn_blocking(|| {
            panic!("synthetic warm-cache panic");
        });
        let successful_phase = WarmCachePhase::ResolveTemplateDirs;
        let successful = spawn_warm_cache_job(successful_phase, Session::default().snapshot());

        let batch = collect_warm_cache_jobs(vec![
            (WarmCachePhase::IndexTemplateLibraries, failed),
            (successful_phase, successful),
        ])
        .await;

        assert_eq!(batch.status, WarmOutcome::Partial);
        assert!(
            batch
                .parts
                .iter()
                .any(|part| part.phase() == successful_phase)
        );
    }

    #[tokio::test]
    async fn requests_during_run_coalesce_to_one_followup() {
        let run_count = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(Notify::new());
        let followup_completed = Arc::new(Notify::new());
        let (release_tx, release_rx) = oneshot::channel();
        let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let runner_probe = Arc::new(DropProbe {
            dropped: Some(dropped_tx),
        });
        let reload = ProjectReload::spawn({
            let run_count = Arc::clone(&run_count);
            let first_started = Arc::clone(&first_started);
            let followup_completed = Arc::clone(&followup_completed);
            let release_rx = Arc::clone(&release_rx);
            move |_| {
                let run_count = Arc::clone(&run_count);
                let first_started = Arc::clone(&first_started);
                let followup_completed = Arc::clone(&followup_completed);
                let release_rx = Arc::clone(&release_rx);
                let runner_probe = Arc::clone(&runner_probe);
                async move {
                    let _runner_probe = runner_probe;
                    let run = run_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if run == 1 {
                        first_started.notify_one();
                        let release_rx = release_rx
                            .lock()
                            .expect("first reload release mutex should not be poisoned")
                            .take()
                            .expect("first reload owns release receiver");
                        drop(release_rx.await);
                    } else {
                        followup_completed.notify_one();
                    }
                    ReloadRunOutcome::Complete
                }
            }
        });

        reload.request();
        timeout(Duration::from_secs(1), first_started.notified())
            .await
            .expect("first reload should start before the test timeout");

        for _ in 0..5 {
            reload.request();
        }

        release_tx
            .send(())
            .expect("first reload release receiver should remain live");
        timeout(Duration::from_secs(1), followup_completed.notified())
            .await
            .expect("follow-up reload should complete before the test timeout");
        drop(reload);
        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("reload worker should terminate after the coalesced follow-up")
            .expect("reload worker should drop its runner capture");

        assert_eq!(run_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancelled_full_reload_retries_the_same_dominant_job() {
        let jobs = Arc::new(StdMutex::new(Vec::new()));
        let replacement_completed = Arc::new(Notify::new());
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let runner_probe = Arc::new(DropProbe {
            dropped: Some(dropped_tx),
        });
        let reload = ProjectReload::spawn({
            let jobs = Arc::clone(&jobs);
            let replacement_completed = Arc::clone(&replacement_completed);
            move |job| {
                let jobs = Arc::clone(&jobs);
                let replacement_completed = Arc::clone(&replacement_completed);
                let runner_probe = Arc::clone(&runner_probe);
                async move {
                    let _runner_probe = runner_probe;
                    let run = {
                        let mut jobs = jobs
                            .lock()
                            .expect("recorded jobs mutex should not be poisoned");
                        jobs.push(job);
                        jobs.len()
                    };
                    if run == 2 {
                        replacement_completed.notify_one();
                        ReloadRunOutcome::Complete
                    } else {
                        ReloadRunOutcome::Cancelled
                    }
                }
            }
        });

        reload.request_current(ProjectWork::FullReload);
        timeout(Duration::from_secs(1), replacement_completed.notified())
            .await
            .expect("replacement reload should complete before the test timeout");
        drop(reload);
        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("reload worker should terminate after the cancellation retry")
            .expect("reload worker should drop its runner capture");
        assert_eq!(
            *jobs
                .lock()
                .expect("recorded jobs mutex should not be poisoned"),
            [ProjectWork::FullReload, ProjectWork::FullReload]
        );
    }

    #[tokio::test]
    async fn reloads_never_overlap() {
        let active_count = Arc::new(AtomicUsize::new(0));
        let overlap_detected = Arc::new(AtomicBool::new(false));
        let run_count = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(Notify::new());
        let second_completed = Arc::new(Notify::new());
        let (second_started_tx, mut second_started_rx) = oneshot::channel();
        let second_started_tx = Arc::new(StdMutex::new(Some(second_started_tx)));
        let (release_tx, release_rx) = oneshot::channel();
        let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
        let reload = ProjectReload::spawn({
            let active_count = Arc::clone(&active_count);
            let overlap_detected = Arc::clone(&overlap_detected);
            let run_count = Arc::clone(&run_count);
            let first_started = Arc::clone(&first_started);
            let second_completed = Arc::clone(&second_completed);
            let second_started_tx = Arc::clone(&second_started_tx);
            let release_rx = Arc::clone(&release_rx);
            move |_| {
                let active_count = Arc::clone(&active_count);
                let overlap_detected = Arc::clone(&overlap_detected);
                let run_count = Arc::clone(&run_count);
                let first_started = Arc::clone(&first_started);
                let second_completed = Arc::clone(&second_completed);
                let second_started_tx = Arc::clone(&second_started_tx);
                let release_rx = Arc::clone(&release_rx);
                async move {
                    if active_count.fetch_add(1, Ordering::SeqCst) != 0 {
                        overlap_detected.store(true, Ordering::SeqCst);
                    }

                    let run = run_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if run == 1 {
                        first_started.notify_one();
                        let release_rx = release_rx
                            .lock()
                            .expect("first reload release mutex should not be poisoned")
                            .take()
                            .expect("first reload owns release receiver");
                        drop(release_rx.await);
                    } else if run == 2 {
                        let start_sender = second_started_tx
                            .lock()
                            .expect("second reload start mutex should not be poisoned")
                            .take()
                            .expect("second reload owns start sender");
                        match start_sender.send(()) {
                            Ok(()) | Err(()) => {}
                        }
                    }

                    active_count.fetch_sub(1, Ordering::SeqCst);
                    if run == 2 {
                        second_completed.notify_one();
                    }
                    ReloadRunOutcome::Complete
                }
            }
        });

        reload.request();
        timeout(Duration::from_secs(1), first_started.notified())
            .await
            .expect("first reload should start before the test timeout");
        reload.request();
        yield_now().await;

        assert!(matches!(
            second_started_rx.try_recv(),
            Err(TryRecvError::Empty)
        ));
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
        assert!(!overlap_detected.load(Ordering::SeqCst));

        release_tx
            .send(())
            .expect("first reload release receiver should remain live");
        timeout(Duration::from_secs(1), second_started_rx)
            .await
            .expect("second reload should start after the first is released")
            .expect("second reload start sender should remain live");
        timeout(Duration::from_secs(1), second_completed.notified())
            .await
            .expect("second reload should complete before the test timeout");

        assert_eq!(run_count.load(Ordering::SeqCst), 2);
        assert!(!overlap_detected.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn queued_full_reload_dominates_cancelled_reprime_without_overlap() {
        let jobs = Arc::new(StdMutex::new(Vec::new()));
        let first_started = Arc::new(Notify::new());
        let second_completed = Arc::new(Notify::new());
        let (release_tx, release_rx) = oneshot::channel();
        let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
        let reload = ProjectReload::spawn({
            let jobs = Arc::clone(&jobs);
            let first_started = Arc::clone(&first_started);
            let second_completed = Arc::clone(&second_completed);
            let release_rx = Arc::clone(&release_rx);
            move |job| {
                let jobs = Arc::clone(&jobs);
                let first_started = Arc::clone(&first_started);
                let second_completed = Arc::clone(&second_completed);
                let release_rx = Arc::clone(&release_rx);
                async move {
                    let run = {
                        let mut jobs = jobs
                            .lock()
                            .expect("recorded jobs mutex should not be poisoned");
                        jobs.push(job);
                        jobs.len()
                    };
                    if run == 1 {
                        first_started.notify_one();
                        let release = release_rx
                            .lock()
                            .expect("first job release mutex should not be poisoned")
                            .take()
                            .expect("first job owns release");
                        drop(release.await);
                        ReloadRunOutcome::Cancelled
                    } else {
                        second_completed.notify_one();
                        ReloadRunOutcome::Complete
                    }
                }
            }
        });

        reload.request_current(ProjectWork::Reprime);
        timeout(Duration::from_secs(1), first_started.notified())
            .await
            .expect("first job should start before the test timeout");
        reload.request_current(ProjectWork::Reprime);
        reload.request_full_reload().await;
        release_tx
            .send(())
            .expect("first job release receiver should remain live");
        timeout(Duration::from_secs(1), second_completed.notified())
            .await
            .expect("second job should complete before the test timeout");

        assert_eq!(
            *jobs
                .lock()
                .expect("recorded jobs mutex should not be poisoned"),
            [ProjectWork::Reprime, ProjectWork::FullReload]
        );
    }

    #[tokio::test]
    async fn project_facts_use_fresh_post_environment_clone() {
        let tempdir = tempdir().expect("temporary project directory should be created");
        let base = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary project path should be valid UTF-8");
        let root = base.join("project");
        let vendor = base.join("vendor");
        std::fs::create_dir_all(root.as_std_path()).expect("project root should be created");
        std::fs::create_dir_all(vendor.join("blog/templatetags").as_std_path())
            .expect("vendor Template Library directory should be created");
        std::fs::write(vendor.join("blog/__init__.py").as_std_path(), "")
            .expect("vendor app package should be written");
        std::fs::write(
            vendor.join("blog/templatetags/__init__.py").as_std_path(),
            "",
        )
        .expect("vendor Template Library package should be written");
        std::fs::write(
            vendor.join("blog/templatetags/blog_tags.py").as_std_path(),
            "",
        )
        .expect("vendor Template Library fixture should be written");
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            format!(
                "venv_path = \"{}\"\npythonpath = [\"{vendor}\"]\n",
                root.join(".venv")
            ),
        )
        .expect("project settings fixture should be written");

        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path())
                    .expect("project root should convert to a file URI"),
                name: "test_project".to_string(),
            }]),
            ..Default::default()
        };
        let session = Arc::new(Mutex::new(Session::new(&params)));
        let settings = match load_project_settings(&session).await {
            StageOutcome::Complete(settings) => Some(settings),
            StageOutcome::Cancelled | StageOutcome::Failed => None,
        }
        .expect("valid project settings should load");
        assert!(apply_project_settings(&session, settings).await);

        let (pre_environment_db, project) = capture_discovery_db(&session)
            .await
            .expect("project should exist");
        assert!(
            !project
                .search_paths(&pre_environment_db)
                .iter()
                .any(|path| path.path() == vendor)
        );
        let environment = DjangoEnvironmentData::assemble(
            environment_phases().map(|phase| phase.run(&pre_environment_db, project)),
        )
        .expect("all Django environment phases should assemble");
        drop(pre_environment_db);
        assert!(apply_environment(&session, environment).await);

        let (facts_db, project) = capture_discovery_db(&session)
            .await
            .expect("project should still exist");
        assert!(
            project
                .search_paths(&facts_db)
                .iter()
                .any(|path| path.path() == vendor)
        );
        let facts = ProjectFactsData::assemble(
            project_facts_phases().map(|phase| phase.run(&facts_db, project)),
        );
        assert!(
            facts
                .file_paths()
                .contains(&vendor.join("blog/templatetags/blog_tags.py"))
        );
    }

    #[tokio::test]
    async fn lsp_overrides_preserve_explicit_false_and_empty_values() {
        let tempdir = tempdir().expect("temporary project directory should be created");
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary project path should be valid UTF-8");
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            r#"
django_settings_module = "project.settings"
pythonpath = ["inherited"]

[format]
enabled = true

[diagnostics.severity]
S100 = "off"
"#,
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
                "pythonpath": [],
                "format": { "enabled": false },
                "diagnostics": {},
            })),
            ..Default::default()
        };
        let session = Arc::new(Mutex::new(Session::new(&params)));

        let settings = match load_project_settings(&session).await {
            StageOutcome::Complete(settings) => settings,
            StageOutcome::Cancelled | StageOutcome::Failed => {
                panic!("valid project settings and LSP overrides should load")
            }
        };

        assert_eq!(settings.django_settings_module(), Some("client.settings"));
        assert!(settings.pythonpath().is_empty());
        assert!(!settings.format().enabled());
        assert_eq!(
            settings.diagnostics().get_severity("S100"),
            djls_conf::DiagnosticSeverity::Error
        );
    }

    #[tokio::test]
    async fn omitted_lsp_overrides_preserve_project_values() {
        let tempdir = tempdir().expect("temporary project directory should be created");
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary project path should be valid UTF-8");
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            r#"
pythonpath = ["inherited"]

[format]
enabled = true

[diagnostics.severity]
S100 = "off"
"#,
        )
        .expect("project settings fixture should be written");

        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path())
                    .expect("project root should convert to a file URI"),
                name: "test_project".to_string(),
            }]),
            initialization_options: Some(serde_json::json!({})),
            ..Default::default()
        };
        let session = Arc::new(Mutex::new(Session::new(&params)));

        let settings = match load_project_settings(&session).await {
            StageOutcome::Complete(settings) => settings,
            StageOutcome::Cancelled | StageOutcome::Failed => {
                panic!("valid project settings and omitted LSP overrides should load")
            }
        };

        assert_eq!(settings.pythonpath(), &[Utf8PathBuf::from("inherited")]);
        assert!(settings.format().enabled());
        assert_eq!(
            settings.diagnostics().get_severity("S100"),
            djls_conf::DiagnosticSeverity::Off
        );
    }

    #[tokio::test]
    async fn project_settings_load_error_skips_discovery_inputs() {
        let tempdir = tempdir().expect("temporary project directory should be created");
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary project path should be valid UTF-8");
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "pythonpath = 'private-config-canary'",
        )
        .expect("invalid project settings fixture should be written");

        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path())
                    .expect("project root should convert to a file URI"),
                name: "test_project".to_string(),
            }]),
            ..Default::default()
        };
        let session = Arc::new(Mutex::new(Session::new(&params)));

        let log = tempfile::NamedTempFile::new().expect("log file");
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(StdMutex::new(log.reopen().expect("log writer")))
            .finish();
        let outcome = load_project_settings(&session)
            .with_subscriber(subscriber)
            .await;

        assert!(matches!(outcome, StageOutcome::Failed));
        let output = std::fs::read_to_string(log.path()).expect("read log");
        assert!(output.contains("Settings load finished"), "{output}");
        let default_visible: Vec<_> = output
            .lines()
            .filter(|line| !line.contains(" DEBUG "))
            .collect();
        assert!(
            default_visible
                .iter()
                .any(|line| line.contains("Error loading project settings")),
            "{output}"
        );
        for line in default_visible {
            assert!(!line.contains("private-config-canary"), "{line}");
            assert!(!line.contains(root.as_str()), "{line}");
        }
        // The detail needed to fix the setting stays available at DEBUG.
        assert!(output.contains("key `pythonpath`"), "{output}");
    }
}
