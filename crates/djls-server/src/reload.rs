//! Project reload orchestration.
//!
//! Runs expensive reload work off the session lock: load settings on a
//! blocking task, compute project facts on a database clone, apply the results
//! under the lock, then warm derived queries and republish diagnostics from a
//! snapshot.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use camino::Utf8PathBuf;
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
use djls_source::path_to_file;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinError;
use tokio::task::JoinSet;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::UriExt;
use crate::progress::ProgressItem;
use crate::progress::ProgressReporter;
use crate::session::IntrinsicReadinessState;
use crate::session::ProjectWork;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReloadRunOutcome {
    Complete,
    Cancelled,
}

/// Drives full Project reloads and intrinsic-only re-primes off the request path.
///
/// The channel is only a wake-up edge. Pending work lives under a synchronous
/// mutex, so a full reload can atomically dominate a queued re-prime and no
/// wake-up can be lost while the single worker is running.
pub(crate) struct ProjectReload {
    tx: mpsc::Sender<()>,
    pending: Arc<StdMutex<Option<ProjectWork>>>,
    session: Option<Arc<Mutex<Session>>>,
}

impl ProjectReload {
    pub(crate) fn new(session: Arc<Mutex<Session>>, client: Client) -> Self {
        let worker_session = Arc::clone(&session);
        let reload = Self::spawn(move |job| {
            let session = Arc::clone(&worker_session);
            let client = client.clone();
            async move {
                let client_info = { session.lock().await.client_info().clone() };
                match job {
                    ProjectWork::FullReload => {
                        reload_project(Arc::clone(&session), client, client_info).await
                    }
                    ProjectWork::Reprime => reprime_project(Arc::clone(&session), client).await,
                }
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
        let pending = Arc::new(StdMutex::new(None));
        let worker_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                loop {
                    let Some(job) = worker_pending.lock().unwrap().take() else {
                        break;
                    };
                    if rx.is_closed() {
                        return;
                    }
                    if runner(job).await == ReloadRunOutcome::Cancelled {
                        merge_project_work(&worker_pending, job);
                    }
                }
            }
        });

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

fn merge_project_work(pending: &StdMutex<Option<ProjectWork>>, requested: ProjectWork) {
    let mut pending = pending.lock().unwrap();
    *pending = Some(match (*pending, requested) {
        (Some(ProjectWork::FullReload), _) | (_, ProjectWork::FullReload) => {
            ProjectWork::FullReload
        }
        (Some(ProjectWork::Reprime) | None, ProjectWork::Reprime) => ProjectWork::Reprime,
    });
}

fn enqueue_project_work(
    pending: &StdMutex<Option<ProjectWork>>,
    tx: &mpsc::Sender<()>,
    requested: ProjectWork,
) {
    merge_project_work(pending, requested);
    match tx.try_send(()) {
        Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
        Err(mpsc::error::TrySendError::Closed(())) => {
            tracing::error!("project reload worker is gone");
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
) -> ReloadRunOutcome {
    let generation = { session.lock().await.desired_generation() };
    let start = std::time::Instant::now();
    let progress = ProgressReporter::new(client.clone(), client_info);

    // Start visible progress before touching the session. Clients often send
    // didOpen/completion immediately after initialized; progress setup should
    // not sit behind a session snapshot in that race.
    let mut environment_progress = Some(progress.begin(RESOLVE_ENVIRONMENT_TITLE).await);
    if let Some(progress) = environment_progress.as_ref() {
        progress.report("Resolving environment").await;
    }

    if !load_and_apply_project_settings(&session, &mut environment_progress).await {
        fail_generation(&session, generation).await;
        return ReloadRunOutcome::Complete;
    }

    let environment =
        match compute_environment(&session, &progress, &mut environment_progress).await {
            StageOutcome::Complete(environment) => environment,
            StageOutcome::Cancelled => return ReloadRunOutcome::Cancelled,
            StageOutcome::Failed => {
                fail_generation(&session, generation).await;
                return ReloadRunOutcome::Complete;
            }
        };
    if !apply_environment(&session, environment).await {
        finish_progress(&mut environment_progress, ProgressEnd::Skipped).await;
        fail_generation(&session, generation).await;
        return ReloadRunOutcome::Complete;
    }
    finish_progress(&mut environment_progress, ProgressEnd::Complete).await;

    let mut facts_progress = None;
    let facts = match compute_project_facts_data(&session, &progress, &mut facts_progress).await {
        StageOutcome::Complete(facts) => facts,
        StageOutcome::Cancelled => return ReloadRunOutcome::Cancelled,
        StageOutcome::Failed => {
            fail_generation(&session, generation).await;
            return ReloadRunOutcome::Complete;
        }
    };
    if !apply_facts(&session, &facts).await {
        finish_progress(&mut facts_progress, ProgressEnd::Skipped).await;
        fail_generation(&session, generation).await;
        return ReloadRunOutcome::Complete;
    }
    finish_progress(&mut facts_progress, ProgressEnd::Complete).await;

    let Some((intrinsic_snapshot, _)) = snapshot_session(&session).await else {
        fail_generation(&session, generation).await;
        return ReloadRunOutcome::Complete;
    };
    let primed = match prime_snapshot(intrinsic_snapshot).await {
        StageOutcome::Complete(primed) => primed,
        StageOutcome::Cancelled => return ReloadRunOutcome::Cancelled,
        StageOutcome::Failed => {
            fail_generation(&session, generation).await;
            return ReloadRunOutcome::Complete;
        }
    };
    if !session
        .lock()
        .await
        .publish_intrinsic_readiness(generation, &primed)
    {
        return ReloadRunOutcome::Complete;
    }

    // Readiness is observable as soon as the required intrinsic products are
    // current. The remaining IDE cache warm-up is optional and must not hold
    // project-aware requests behind unrelated work.
    let Some((snapshot, documents)) = snapshot_session(&session).await else {
        return ReloadRunOutcome::Complete;
    };
    refresh_or_republish_diagnostics(client, snapshot.clone(), documents).await;
    warm_snapshot_queries(&progress, snapshot).await;

    tracing::info!("Project reload completed in {:?}", start.elapsed());
    ReloadRunOutcome::Complete
}

async fn reprime_project(session: Arc<Mutex<Session>>, client: Client) -> ReloadRunOutcome {
    let (generation, snapshot) = {
        let session = session.lock().await;
        (session.desired_generation(), session.snapshot())
    };
    match prime_snapshot(snapshot).await {
        StageOutcome::Complete(primed) => {
            if !session
                .lock()
                .await
                .publish_intrinsic_readiness(generation, &primed)
            {
                return ReloadRunOutcome::Complete;
            }
            let Some((snapshot, documents)) = snapshot_session(&session).await else {
                return ReloadRunOutcome::Complete;
            };
            refresh_or_republish_diagnostics(client, snapshot, documents).await;
            ReloadRunOutcome::Complete
        }
        StageOutcome::Cancelled => ReloadRunOutcome::Cancelled,
        StageOutcome::Failed => {
            fail_generation(&session, generation).await;
            ReloadRunOutcome::Complete
        }
    }
}

async fn fail_generation(session: &Arc<Mutex<Session>>, generation: u64) {
    session.lock().await.fail_intrinsic_readiness(generation);
}

async fn warm_snapshot_queries(progress: &ProgressReporter, snapshot: SessionSnapshot) {
    let warm_progress = progress.begin(WARM_CACHES_TITLE).await;
    let warm_outcome = warm_cache_queries(snapshot, &warm_progress).await;
    warm_progress
        .finish(warm_outcome.progress_end().as_str())
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

async fn snapshot_session(
    session: &Arc<Mutex<Session>>,
) -> Option<(SessionSnapshot, Vec<TextDocument>)> {
    let session_lock = session.lock().await;
    session_lock.db().project()?;
    Some((session_lock.snapshot(), session_lock.open_documents()))
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
        tracing::info!("Task: No project configured, skipping settings load.");
        return StageOutcome::Failed;
    };

    let joined =
        tokio::task::spawn_blocking(move || Settings::new(&project_root, Some(config_overrides)))
            .await;
    let settings = match classify_child_task_join(joined) {
        ChildTaskJoin::Complete(settings) => settings,
        ChildTaskJoin::Failed(error) => {
            tracing::error!(?error, "Project settings load task failed");
            return StageOutcome::Failed;
        }
    };

    match settings {
        Ok(settings) => StageOutcome::Complete(settings),
        Err(error) => {
            tracing::error!(%error, "Error loading project settings");
            StageOutcome::Failed
        }
    }
}

type EnvironmentJobResult = Result<EnvironmentPart, salsa::Cancelled>;
type ProjectFactsJobResult = Result<ProjectFactsPart, salsa::Cancelled>;

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
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let Some((compute_db, project)) = capture_discovery_db(session).await else {
            finish_progress(progress, ProgressEnd::Skipped).await;
            return StageOutcome::Failed;
        };

        let mut jobs: JoinSet<EnvironmentJobResult> = JoinSet::new();
        for phase in environment_phases() {
            let db = compute_db.clone();
            jobs.spawn_blocking(move || {
                salsa::Cancelled::catch(AssertUnwindSafe(|| phase.run(&db, project)))
            });
            report_environment_phase(phase, reporter, progress).await;
        }

        let result = collect_environment_jobs(jobs, progress.as_ref()).await;
        match result {
            StageOutcome::Complete(environment) => return StageOutcome::Complete(environment),
            StageOutcome::Cancelled if attempt < SNAPSHOT_CANCEL_RETRIES => {
                finish_progress(progress, ProgressEnd::Retrying).await;
                tracing::debug!(
                    attempt = attempt + 1,
                    "Environment compute cancelled; retrying with fresh database clone"
                );
            }
            StageOutcome::Cancelled => {
                finish_progress(progress, ProgressEnd::Cancelled).await;
                tracing::warn!(
                    retries = SNAPSHOT_CANCEL_RETRIES,
                    "Environment compute cancelled repeatedly; project reload cancelled"
                );
                return StageOutcome::Cancelled;
            }
            StageOutcome::Failed => {
                finish_progress(progress, ProgressEnd::Failed).await;
                return StageOutcome::Failed;
            }
        }
    }
    unreachable!("Environment retry loop must return")
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
                tracing::error!(?error, "Django Environment phase task failed");
            }
        }
    }
    if failed {
        StageOutcome::Failed
    } else if cancellation.is_some() {
        StageOutcome::Cancelled
    } else {
        StageOutcome::Complete(DjangoEnvironmentData::assemble(parts))
    }
}

async fn compute_project_facts_data(
    session: &Arc<Mutex<Session>>,
    reporter: &ProgressReporter,
    progress: &mut Option<ProgressItem>,
) -> StageOutcome<ProjectFactsData> {
    // Capture happens after Environment application, so every attempt observes
    // registered, rescanned roots and overlay-authoritative file contents.
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let Some((compute_db, project)) = capture_discovery_db(session).await else {
            finish_progress(progress, ProgressEnd::Skipped).await;
            return StageOutcome::Failed;
        };

        let mut jobs: JoinSet<ProjectFactsJobResult> = JoinSet::new();
        for phase in project_facts_phases() {
            let db = compute_db.clone();
            jobs.spawn_blocking(move || {
                salsa::Cancelled::catch(AssertUnwindSafe(|| phase.run(&db, project)))
            });
            report_project_facts_phase(phase, reporter, progress).await;
        }

        let result = collect_project_facts_jobs(jobs, progress.as_ref()).await;
        match result {
            StageOutcome::Complete(facts) => return StageOutcome::Complete(facts),
            StageOutcome::Cancelled if attempt < SNAPSHOT_CANCEL_RETRIES => {
                finish_progress(progress, ProgressEnd::Retrying).await;
                tracing::debug!(
                    attempt = attempt + 1,
                    "Project Facts compute cancelled; retrying with fresh database clone"
                );
            }
            StageOutcome::Cancelled => {
                finish_progress(progress, ProgressEnd::Cancelled).await;
                tracing::warn!(
                    retries = SNAPSHOT_CANCEL_RETRIES,
                    "Project Facts compute cancelled repeatedly; project reload cancelled"
                );
                return StageOutcome::Cancelled;
            }
            StageOutcome::Failed => {
                finish_progress(progress, ProgressEnd::Failed).await;
                return StageOutcome::Failed;
            }
        }
    }
    unreachable!("Project Facts retry loop must return")
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
                tracing::error!(?error, "Project Facts phase task failed");
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
        tracing::info!("Task: No project configured, skipping initialization.");
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

fn remember_cancellation(cancellation: &mut Option<salsa::Cancelled>, cancelled: salsa::Cancelled) {
    if cancellation.is_none() {
        *cancellation = Some(cancelled);
    }
}

async fn finish_progress(progress: &mut Option<ProgressItem>, end: ProgressEnd) {
    if let Some(progress) = progress.take() {
        progress.finish(end.as_str()).await;
    }
}

type WarmJobResult = Result<WarmCachePart, salsa::Cancelled>;
type WarmJobHandle = tokio::task::JoinHandle<WarmJobResult>;

async fn prime_snapshot(snapshot: SessionSnapshot) -> StageOutcome<PrimedTemplateLibraries> {
    let joined = tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            prime_template_library_products(snapshot.db())
        }))
    })
    .await;

    classify_prime_task_join(joined)
}

fn classify_prime_task_join(
    joined: Result<Result<Option<PrimedTemplateLibraries>, salsa::Cancelled>, JoinError>,
) -> StageOutcome<PrimedTemplateLibraries> {
    match classify_child_task_join(joined) {
        ChildTaskJoin::Complete(Ok(Some(primed))) => StageOutcome::Complete(primed),
        ChildTaskJoin::Complete(Ok(None)) => StageOutcome::Failed,
        ChildTaskJoin::Complete(Err(cancelled)) => {
            tracing::debug!(?cancelled, "Template Library priming cancelled");
            StageOutcome::Cancelled
        }
        ChildTaskJoin::Failed(error) => {
            tracing::error!(?error, "Template Library priming task failed");
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
    tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| phase.run(snapshot.db())))
    })
}

async fn join_warm_cache_job(
    phase: WarmCachePhase,
    handle: WarmJobHandle,
) -> StageOutcome<WarmCachePart> {
    match classify_child_task_join(handle.await) {
        ChildTaskJoin::Complete(Ok(part)) => StageOutcome::Complete(part),
        ChildTaskJoin::Complete(Err(cancelled)) => {
            tracing::debug!(
                ?cancelled,
                ?phase,
                "IDE cache warm-up cancelled; newer inputs will re-warm queries"
            );
            StageOutcome::Cancelled
        }
        ChildTaskJoin::Failed(error) => {
            tracing::error!(?error, ?phase, "IDE cache warm-up task failed");
            StageOutcome::Failed
        }
    }
}

struct WarmBatchOutcome {
    status: WarmOutcome,
    summaries: Vec<(usize, WarmCachePhase, usize)>,
}

async fn collect_warm_cache_jobs(
    handles: Vec<(usize, WarmCachePhase, WarmJobHandle)>,
) -> WarmBatchOutcome {
    let mut summaries = Vec::new();
    let mut status = WarmOutcome::Complete;
    for (done, phase, handle) in handles {
        match join_warm_cache_job(phase, handle).await {
            StageOutcome::Complete(part) => {
                if let Some(count) = part.count() {
                    summaries.push((done, part.phase(), count));
                }
            }
            StageOutcome::Cancelled | StageOutcome::Failed => {
                status = WarmOutcome::Partial;
            }
        }
    }

    WarmBatchOutcome { status, summaries }
}

async fn warm_cache_queries(snapshot: SessionSnapshot, progress: &ProgressItem) -> WarmOutcome {
    let mut handles = Vec::new();
    for (index, phase) in warm_cache_phases().iter().copied().enumerate() {
        handles.push((
            index + 1,
            phase,
            spawn_warm_cache_job(phase, snapshot.clone()),
        ));
        progress.report(phase.progress().message).await;
    }

    let batch = collect_warm_cache_jobs(handles).await;
    if batch.status == WarmOutcome::Complete {
        for (done, phase, count) in batch.summaries {
            report_warm_summary(progress, done, phase, count).await;
        }
    }

    batch.status
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticsDelivery {
    WorkspaceRefresh,
    PublishOpenDocuments,
}

fn diagnostics_delivery(client_info: &ClientInfo) -> DiagnosticsDelivery {
    if client_info.supports_pull_diagnostics()
        && client_info.supports_workspace_diagnostic_refresh()
    {
        DiagnosticsDelivery::WorkspaceRefresh
    } else {
        DiagnosticsDelivery::PublishOpenDocuments
    }
}

async fn refresh_or_republish_diagnostics(
    client: Client,
    snapshot: SessionSnapshot,
    documents: Vec<TextDocument>,
) {
    if diagnostics_delivery(snapshot.client_info()) == DiagnosticsDelivery::WorkspaceRefresh {
        match client.workspace_diagnostic_refresh().await {
            Ok(()) => tracing::debug!("Requested workspace diagnostics refresh"),
            Err(error) => tracing::debug!(?error, "Client rejected workspace diagnostics refresh"),
        }
        return;
    }

    for document in documents {
        let path = document.path().to_path_buf();
        let Some(diagnostics) = collect_snapshot_diagnostics(snapshot.clone(), path).await else {
            continue;
        };

        let Some(lsp_uri) = ls_types::Uri::from_path(document.path()) else {
            continue;
        };

        let diagnostic_count = diagnostics.len();
        let lsp_uri_text = lsp_uri.to_string();
        client
            .publish_diagnostics(lsp_uri, diagnostics, Some(document.version()))
            .await;

        tracing::debug!(
            "Published {} diagnostics for {}",
            diagnostic_count,
            lsp_uri_text
        );
    }
}

type DiagnosticsJobResult = Result<Option<Vec<ls_types::Diagnostic>>, salsa::Cancelled>;

fn classify_diagnostics_task_join(
    joined: Result<DiagnosticsJobResult, JoinError>,
) -> DiagnosticsJobResult {
    match classify_child_task_join(joined) {
        ChildTaskJoin::Complete(result) => result,
        ChildTaskJoin::Failed(error) => {
            tracing::error!(
                ?error,
                "Diagnostics snapshot task failed; skipping republish"
            );
            Ok(None)
        }
    }
}

async fn collect_snapshot_diagnostics(
    snapshot: SessionSnapshot,
    path: Utf8PathBuf,
) -> Option<Vec<ls_types::Diagnostic>> {
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let snapshot = snapshot.clone();
        let path = path.clone();
        let joined = tokio::task::spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                let file = path_to_file(snapshot.db(), &path).ok()?;
                djls_ide::collect_diagnostics(snapshot.db(), file)
            }))
        })
        .await;

        match classify_diagnostics_task_join(joined) {
            Ok(diagnostics) => return diagnostics,
            Err(cancelled) if attempt < SNAPSHOT_CANCEL_RETRIES => {
                tracing::debug!(
                    ?cancelled,
                    attempt = attempt + 1,
                    "Snapshot diagnostics cancelled; retrying with same snapshot"
                );
            }
            Err(cancelled) => {
                tracing::debug!(
                    ?cancelled,
                    retries = SNAPSHOT_CANCEL_RETRIES,
                    "Snapshot diagnostics cancelled; skipping diagnostics republish"
                );
                return None;
            }
        }
    }

    unreachable!("diagnostics retry loop must return")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use camino::Utf8PathBuf;
    use tempfile::tempdir;
    use tokio::sync::Notify;
    use tokio::sync::oneshot;
    use tokio::time::timeout;

    use super::*;

    struct DropProbe {
        dropped: Option<oneshot::Sender<()>>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            if let Some(dropped) = self.dropped.take() {
                dropped.send(()).ok();
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
                            .unwrap()
                            .take()
                            .expect("active reload owns release receiver");
                        release.await.ok();
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

        assert!(matches!(
            dropped_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
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
                            .unwrap()
                            .take()
                            .expect("cancelled reload owns release receiver");
                        release.await.ok();
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

        assert!(matches!(
            dropped_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("cancelled reload worker should terminate after owner drop")
            .expect("reload worker should drop its runner capture");
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ready_diagnostics_use_the_delivery_supported_by_the_client() {
        let push_session = Session::default();
        assert_eq!(
            diagnostics_delivery(push_session.client_info()),
            DiagnosticsDelivery::PublishOpenDocuments
        );

        let pull_without_refresh = Session::new(&ls_types::InitializeParams {
            capabilities: ls_types::ClientCapabilities {
                text_document: Some(ls_types::TextDocumentClientCapabilities {
                    diagnostic: Some(ls_types::DiagnosticClientCapabilities::default()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(
            diagnostics_delivery(pull_without_refresh.client_info()),
            DiagnosticsDelivery::PublishOpenDocuments
        );

        let pull_with_refresh = Session::new(&ls_types::InitializeParams {
            capabilities: ls_types::ClientCapabilities {
                workspace: Some(ls_types::WorkspaceClientCapabilities {
                    diagnostics: Some(ls_types::DiagnosticWorkspaceClientCapabilities {
                        refresh_support: Some(true),
                    }),
                    ..Default::default()
                }),
                text_document: Some(ls_types::TextDocumentClientCapabilities {
                    diagnostic: Some(ls_types::DiagnosticClientCapabilities::default()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(
            diagnostics_delivery(pull_with_refresh.client_info()),
            DiagnosticsDelivery::WorkspaceRefresh
        );
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
            .unwrap();
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
                        let joined = tokio::spawn(async {
                            panic!("synthetic critical child panic");
                            #[allow(unreachable_code)]
                            Ok::<Option<PrimedTemplateLibraries>, salsa::Cancelled>(None)
                        })
                        .await;
                        assert!(matches!(
                            classify_prime_task_join(joined),
                            StageOutcome::Failed
                        ));
                        fail_generation(&session, 0).await;
                    }
                    completed_tx.send(run).unwrap();
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
                .unwrap(),
            Some(1)
        );
        readiness.changed().await.unwrap();
        assert_eq!(
            *readiness.borrow_and_update(),
            IntrinsicReadinessState::Failed(0)
        );

        reload.request();
        assert_eq!(
            timeout(Duration::from_secs(1), completed_rx.recv())
                .await
                .unwrap(),
            Some(2)
        );
        assert_eq!(run_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn project_settings_task_panic_is_classified_as_failure() {
        let joined: Result<(), JoinError> = tokio::spawn(async {
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
        let joined = tokio::spawn(async {
            panic!("synthetic intrinsic priming panic");
            #[allow(unreachable_code)]
            Ok::<Option<PrimedTemplateLibraries>, salsa::Cancelled>(None)
        })
        .await;

        assert!(matches!(
            classify_prime_task_join(joined),
            StageOutcome::Failed
        ));
    }

    #[tokio::test]
    async fn intrinsic_priming_salsa_cancellation_is_not_classified_as_failure() {
        let joined = tokio::spawn(async {
            Err::<Option<PrimedTemplateLibraries>, _>(salsa::Cancelled::Local)
        })
        .await;

        assert!(matches!(
            classify_prime_task_join(joined),
            StageOutcome::Cancelled
        ));
    }

    #[tokio::test]
    async fn warm_cache_panic_in_mixed_batch_is_partial_and_retains_successful_sibling() {
        let failed: WarmJobHandle = tokio::task::spawn_blocking(|| {
            panic!("synthetic warm-cache panic");
        });
        let successful_phase = WarmCachePhase::ResolveTemplateDirs;
        let successful = spawn_warm_cache_job(successful_phase, Session::default().snapshot());

        let batch = collect_warm_cache_jobs(vec![
            (1, WarmCachePhase::BuildModelGraph, failed),
            (2, successful_phase, successful),
        ])
        .await;

        assert_eq!(batch.status, WarmOutcome::Partial);
        assert!(
            batch
                .summaries
                .iter()
                .any(|(_, phase, _)| *phase == successful_phase)
        );
    }

    #[tokio::test]
    async fn diagnostics_snapshot_task_panic_produces_no_publish_payload() {
        let joined = tokio::task::spawn_blocking(|| {
            panic!("synthetic diagnostics panic");
            #[allow(unreachable_code)]
            Ok::<Option<Vec<ls_types::Diagnostic>>, salsa::Cancelled>(None)
        })
        .await;

        let publish_payload = classify_diagnostics_task_join(joined)
            .expect("child panic is an infrastructure failure, not Salsa cancellation");
        assert!(publish_payload.is_none());
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
                            .unwrap()
                            .take()
                            .expect("first reload owns release receiver");
                        release_rx.await.ok();
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
            .unwrap();

        for _ in 0..5 {
            reload.request();
        }

        release_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), followup_completed.notified())
            .await
            .unwrap();
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
                        let mut jobs = jobs.lock().unwrap();
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
            .unwrap();
        drop(reload);
        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("reload worker should terminate after the cancellation retry")
            .expect("reload worker should drop its runner capture");
        assert_eq!(
            *jobs.lock().unwrap(),
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
                            .unwrap()
                            .take()
                            .expect("first reload owns release receiver");
                        release_rx.await.ok();
                    } else if run == 2 {
                        second_started_tx
                            .lock()
                            .unwrap()
                            .take()
                            .expect("second reload owns start sender")
                            .send(())
                            .ok();
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
            .unwrap();
        reload.request();
        tokio::task::yield_now().await;

        assert!(matches!(
            second_started_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
        assert!(!overlap_detected.load(Ordering::SeqCst));

        release_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), second_started_rx)
            .await
            .expect("second reload should start after the first is released")
            .expect("second reload start sender should remain live");
        timeout(Duration::from_secs(1), second_completed.notified())
            .await
            .unwrap();

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
                        let mut jobs = jobs.lock().unwrap();
                        jobs.push(job);
                        jobs.len()
                    };
                    if run == 1 {
                        first_started.notify_one();
                        let release = release_rx
                            .lock()
                            .unwrap()
                            .take()
                            .expect("first job owns release");
                        release.await.ok();
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
            .unwrap();
        reload.request_current(ProjectWork::Reprime);
        reload.request_full_reload().await;
        release_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), second_completed.notified())
            .await
            .unwrap();

        assert_eq!(
            *jobs.lock().unwrap(),
            [ProjectWork::Reprime, ProjectWork::FullReload]
        );
    }

    #[tokio::test]
    async fn project_facts_use_fresh_post_environment_clone() {
        let tempdir = tempdir().unwrap();
        let base = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        let root = base.join("project");
        let vendor = base.join("vendor");
        std::fs::create_dir_all(root.as_std_path()).unwrap();
        std::fs::create_dir_all(vendor.join("blog").as_std_path()).unwrap();
        std::fs::write(vendor.join("blog/models.py").as_std_path(), "").unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            format!(
                "venv_path = \"{}\"\npythonpath = [\"{vendor}\"]\n",
                root.join(".venv")
            ),
        )
        .unwrap();

        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path()).unwrap(),
                name: "test_project".to_string(),
            }]),
            ..Default::default()
        };
        let session = Arc::new(Mutex::new(Session::new(&params)));
        let StageOutcome::Complete(settings) = load_project_settings(&session).await else {
            panic!("project settings should load");
        };
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
        );
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
        assert!(facts.file_paths().contains(&vendor.join("blog/models.py")));
    }

    #[tokio::test]
    async fn project_settings_load_error_skips_discovery_inputs() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "debug = not_a_boolean",
        )
        .unwrap();

        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path()).unwrap(),
                name: "test_project".to_string(),
            }]),
            ..Default::default()
        };
        let session = Arc::new(Mutex::new(Session::new(&params)));

        let outcome = load_project_settings(&session).await;

        assert!(matches!(outcome, StageOutcome::Failed));
    }
}
