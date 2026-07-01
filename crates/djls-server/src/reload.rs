//! Project reload orchestration.
//!
//! Runs expensive reload work off the session lock: load settings on a
//! blocking task, compute project facts on a database clone, apply the results
//! under the lock, then warm derived queries and republish diagnostics from a
//! snapshot.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_ide::WarmCachePart;
use djls_ide::WarmCachePhase;
use djls_ide::warm_cache_phases;
use djls_project::Db as ProjectDb;
use djls_project::DiscoveryPhase;
use djls_project::DjangoDiscoveryData;
use djls_project::DjangoDiscoveryPart;
use djls_project::Project;
use djls_project::apply_django_discovery;
use djls_project::django_discovery_phases;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::UriExt;
use crate::progress::ProgressItem;
use crate::progress::ProgressReporter;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

/// Drives project reloads off the LSP handler path.
///
/// A capacity-1 channel serializes reloads and leaves room for at most one
/// pending follow-up request while a reload is running. Dropping the sender
/// lets the worker exit after it drains any current or queued request.
pub(crate) struct ProjectReload {
    tx: mpsc::Sender<()>,
}

impl ProjectReload {
    pub(crate) fn new(session: Arc<Mutex<Session>>, client: Client) -> Self {
        Self::spawn(move || {
            let session = Arc::clone(&session);
            let client = client.clone();
            async move {
                let client_info = { session.lock().await.client_info().clone() };
                reload_project(session, client, client_info).await;
            }
        })
    }

    fn spawn<F, Fut>(runner: F) -> Self
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel(1);
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                runner().await;
            }
        });

        Self { tx }
    }

    pub(crate) fn request(&self) {
        match self.tx.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
            Err(mpsc::error::TrySendError::Closed(())) => {
                tracing::error!("project reload worker is gone");
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProgressEnd {
    Complete,
    Skipped,
    Retrying,
    Cancelled,
    Partial,
}

impl ProgressEnd {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Skipped => "skipped",
            Self::Retrying => "retrying",
            Self::Cancelled => "cancelled",
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
    phase: DiscoveryPhase,
    label: CountLabel,
    count: usize,
}

const RESOLVE_ENVIRONMENT_TITLE: &str = "Resolving Django environment";
const DISCOVER_PROJECT_FACTS_TITLE: &str = "Discovering Django project facts";
const WARM_CACHES_TITLE: &str = "Warming Django caches";

async fn reload_project(session: Arc<Mutex<Session>>, client: Client, client_info: ClientInfo) {
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
        return;
    }

    let mut facts_progress = None;
    let Some(discovery) = compute_project_discovery(
        &session,
        &progress,
        &mut environment_progress,
        &mut facts_progress,
    )
    .await
    else {
        return;
    };

    if facts_progress.is_none() {
        facts_progress = Some(progress.begin(DISCOVER_PROJECT_FACTS_TITLE).await);
    }
    if let Some(progress) = facts_progress.as_ref() {
        progress.report("Applying project facts").await;
    }

    let Some((snapshot, documents)) = apply_project_facts(&session, discovery).await else {
        finish_progress(&mut facts_progress, ProgressEnd::Skipped).await;
        return;
    };
    finish_progress(&mut facts_progress, ProgressEnd::Complete).await;

    warm_snapshot_queries(&progress, snapshot.clone()).await;
    republish_snapshot_diagnostics(client, snapshot, documents).await;

    tracing::info!("Project reload completed in {:?}", start.elapsed());
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
    let Some(settings) = load_project_settings(session).await else {
        finish_progress(progress, ProgressEnd::Skipped).await;
        return false;
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

async fn apply_project_facts(
    session: &Arc<Mutex<Session>>,
    discovery: DjangoDiscoveryData,
) -> Option<(SessionSnapshot, Vec<TextDocument>)> {
    let mut session_lock = session.lock().await;
    let db = session_lock.db_mut();
    db.project()?;

    let t = std::time::Instant::now();
    apply_django_discovery(db, discovery);
    tracing::info!("Django Discovery apply completed in {:?}", t.elapsed());

    Some((session_lock.snapshot(), session_lock.open_documents()))
}

async fn load_project_settings(session: &Arc<Mutex<Session>>) -> Option<Settings> {
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
        return None;
    };

    let settings =
        tokio::task::spawn_blocking(move || Settings::new(&project_root, Some(config_overrides)))
            .await
            .expect("project settings load task must not panic");

    match settings {
        Ok(settings) => Some(settings),
        Err(err) => {
            tracing::error!("Error loading settings: {}", err);
            None
        }
    }
}

type DiscoveryJobResult = Result<DjangoDiscoveryPart, salsa::Cancelled>;
type DiscoveryJobSet = JoinSet<DiscoveryJobResult>;

async fn compute_project_discovery(
    session: &Arc<Mutex<Session>>,
    progress: &ProgressReporter,
    environment_progress: &mut Option<ProgressItem>,
    facts_progress: &mut Option<ProgressItem>,
) -> Option<DjangoDiscoveryData> {
    // Cancellation here usually means a document edit, not a config change:
    // nothing bumps the epoch or resubmits, so dropping the compute would lose
    // the Django Discovery run. Retry with a fresh database clone instead,
    // like the snapshot reads do.
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let Some((compute_db, project)) = capture_discovery_db(session).await else {
            finish_progress(environment_progress, ProgressEnd::Skipped).await;
            finish_progress(facts_progress, ProgressEnd::Skipped).await;
            return None;
        };

        let handles = spawn_discovery_jobs(
            compute_db,
            project,
            progress,
            environment_progress,
            facts_progress,
        )
        .await;
        let result = collect_discovery_jobs(handles, environment_progress, facts_progress).await;

        match result {
            Ok(discovery) => {
                finish_progress(environment_progress, ProgressEnd::Complete).await;
                return Some(discovery);
            }
            Err(cancelled) if attempt < SNAPSHOT_CANCEL_RETRIES => {
                finish_progress(environment_progress, ProgressEnd::Retrying).await;
                finish_progress(facts_progress, ProgressEnd::Retrying).await;
                tracing::debug!(
                    ?cancelled,
                    attempt = attempt + 1,
                    "Django Discovery compute cancelled; retrying with fresh database clone"
                );
            }
            Err(cancelled) => {
                finish_progress(environment_progress, ProgressEnd::Cancelled).await;
                finish_progress(facts_progress, ProgressEnd::Cancelled).await;
                tracing::warn!(
                    ?cancelled,
                    retries = SNAPSHOT_CANCEL_RETRIES,
                    "Django Discovery compute cancelled repeatedly; project facts may be stale until the next Django Discovery run"
                );
                return None;
            }
        }
    }

    unreachable!("Django Discovery retry loop must return")
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

async fn spawn_discovery_jobs(
    compute_db: DjangoDatabase,
    project: Project,
    reporter: &ProgressReporter,
    environment_progress: &mut Option<ProgressItem>,
    facts_progress: &mut Option<ProgressItem>,
) -> DiscoveryJobSet {
    let mut jobs = JoinSet::new();
    for phase in django_discovery_phases() {
        let db = compute_db.clone();
        jobs.spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| phase.run(&db, project)))
        });
    }

    for phase in django_discovery_phases() {
        report_discovery_phase(phase, reporter, environment_progress, facts_progress).await;
    }

    jobs
}

async fn collect_discovery_jobs(
    mut jobs: DiscoveryJobSet,
    environment_progress: &mut Option<ProgressItem>,
    facts_progress: &mut Option<ProgressItem>,
) -> Result<DjangoDiscoveryData, salsa::Cancelled> {
    let mut cancellation = None;
    let mut counts = Vec::new();
    let mut parts = Vec::new();

    while let Some(joined) = jobs.join_next().await {
        match joined.expect("Django Discovery phase must not panic") {
            Ok(part) => {
                let phase = part.phase();
                let progress = phase.progress();
                counts.push(DiscoveryJobCount {
                    phase,
                    label: progress.count_label.into(),
                    count: part.count(),
                });
                parts.push(part);
            }
            Err(cancelled) => remember_cancellation(&mut cancellation, cancelled),
        }
    }

    if let Some(cancelled) = cancellation {
        return Err(cancelled);
    }

    let discovery = DjangoDiscoveryData::assemble(parts);
    report_discovery_job_counts(
        counts,
        discovery.discovered_file_count(),
        environment_progress.as_ref(),
        facts_progress.as_ref(),
    )
    .await;

    Ok(discovery)
}

async fn report_discovery_job_counts(
    counts: Vec<DiscoveryJobCount>,
    discovered_file_count: usize,
    environment_progress: Option<&ProgressItem>,
    facts_progress: Option<&ProgressItem>,
) {
    let environment_total = DiscoveryPhase::environment_phase_count();
    let facts_total = DiscoveryPhase::project_facts_phase_count() + 1;
    let mut environment_done = 0;
    let mut facts_done = 0;

    for summary in counts {
        match summary.phase {
            DiscoveryPhase::Environment(_) => {
                environment_done += 1;
                report_count(
                    environment_progress,
                    environment_done,
                    environment_total,
                    summary.count,
                    summary.label,
                )
                .await;
            }
            DiscoveryPhase::ProjectFacts(_) => {
                facts_done += 1;
                report_count(
                    facts_progress,
                    facts_done,
                    facts_total,
                    summary.count,
                    summary.label,
                )
                .await;
            }
        }
    }

    report_count(
        facts_progress,
        facts_total,
        facts_total,
        discovered_file_count,
        DjangoDiscoveryData::discovered_file_count_label().into(),
    )
    .await;
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

async fn report_discovery_phase(
    phase: DiscoveryPhase,
    reporter: &ProgressReporter,
    environment_progress: &mut Option<ProgressItem>,
    facts_progress: &mut Option<ProgressItem>,
) {
    let phase_progress = phase.progress();
    let (progress, title) = discovery_phase_progress(phase, environment_progress, facts_progress);

    if progress.is_none() {
        *progress = Some(reporter.begin(title).await);
    }
    if let Some(progress) = progress.as_ref() {
        progress.report(phase_progress.message).await;
    }
}

fn discovery_phase_progress<'a>(
    phase: DiscoveryPhase,
    environment_progress: &'a mut Option<ProgressItem>,
    facts_progress: &'a mut Option<ProgressItem>,
) -> (&'a mut Option<ProgressItem>, &'static str) {
    match phase {
        DiscoveryPhase::Environment(_) => (environment_progress, RESOLVE_ENVIRONMENT_TITLE),
        DiscoveryPhase::ProjectFacts(_) => (facts_progress, DISCOVER_PROJECT_FACTS_TITLE),
    }
}

async fn finish_progress(progress: &mut Option<ProgressItem>, end: ProgressEnd) {
    if let Some(progress) = progress.take() {
        progress.finish(end.as_str()).await;
    }
}

type WarmJobResult = Result<WarmCachePart, salsa::Cancelled>;
type WarmJobHandle = tokio::task::JoinHandle<WarmJobResult>;

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

async fn join_warm_cache_job(phase: WarmCachePhase, handle: WarmJobHandle) -> WarmJobResult {
    handle.await.unwrap_or_else(|error| {
        panic!("IDE cache warm-up {phase:?} task must not panic: {error}");
    })
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

    let mut summaries = Vec::new();
    let mut outcome = WarmOutcome::Complete;
    for (done, phase, handle) in handles {
        match join_warm_cache_job(phase, handle).await {
            Ok(part) => {
                if let Some(count) = part.count() {
                    summaries.push((done, part.phase(), count));
                }
            }
            Err(cancelled) => {
                outcome = WarmOutcome::Partial;
                tracing::debug!(
                    ?cancelled,
                    ?phase,
                    "IDE cache warm-up cancelled; newer inputs will re-warm queries"
                );
            }
        }
    }

    if outcome == WarmOutcome::Complete {
        for (done, phase, count) in summaries {
            report_warm_summary(progress, done, phase, count).await;
        }
    }

    outcome
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

async fn republish_snapshot_diagnostics(
    client: Client,
    snapshot: SessionSnapshot,
    documents: Vec<TextDocument>,
) {
    if snapshot.client_info().supports_pull_diagnostics() {
        tracing::debug!("Client supports pull diagnostics, skipping diagnostics push");
        return;
    }

    for document in documents {
        let file = document.file();
        let Some(diagnostics) = collect_snapshot_diagnostics(snapshot.clone(), file).await else {
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

async fn collect_snapshot_diagnostics(
    snapshot: SessionSnapshot,
    file: djls_source::File,
) -> Option<Vec<ls_types::Diagnostic>> {
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let snapshot = snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                djls_ide::collect_diagnostics(snapshot.db(), file)
            }))
        })
        .await
        .expect("diagnostics snapshot task must not panic");

        match result {
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
    use tokio::time::sleep;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn request_runs_one_reload() {
        let run_count = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(Notify::new());
        let reload = ProjectReload::spawn({
            let run_count = Arc::clone(&run_count);
            let completed = Arc::clone(&completed);
            move || {
                let run_count = Arc::clone(&run_count);
                let completed = Arc::clone(&completed);
                async move {
                    run_count.fetch_add(1, Ordering::SeqCst);
                    completed.notify_one();
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
    async fn requests_during_run_coalesce_to_one_followup() {
        let run_count = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(Notify::new());
        let followup_completed = Arc::new(Notify::new());
        let (release_tx, release_rx) = oneshot::channel();
        let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
        let reload = ProjectReload::spawn({
            let run_count = Arc::clone(&run_count);
            let first_started = Arc::clone(&first_started);
            let followup_completed = Arc::clone(&followup_completed);
            let release_rx = Arc::clone(&release_rx);
            move || {
                let run_count = Arc::clone(&run_count);
                let first_started = Arc::clone(&first_started);
                let followup_completed = Arc::clone(&followup_completed);
                let release_rx = Arc::clone(&release_rx);
                async move {
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
        sleep(Duration::from_millis(50)).await;

        assert_eq!(run_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn reloads_never_overlap() {
        let active_count = Arc::new(AtomicUsize::new(0));
        let overlap_detected = Arc::new(AtomicBool::new(false));
        let run_count = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(Notify::new());
        let second_completed = Arc::new(Notify::new());
        let (release_tx, release_rx) = oneshot::channel();
        let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
        let reload = ProjectReload::spawn({
            let active_count = Arc::clone(&active_count);
            let overlap_detected = Arc::clone(&overlap_detected);
            let run_count = Arc::clone(&run_count);
            let first_started = Arc::clone(&first_started);
            let second_completed = Arc::clone(&second_completed);
            let release_rx = Arc::clone(&release_rx);
            move || {
                let active_count = Arc::clone(&active_count);
                let overlap_detected = Arc::clone(&overlap_detected);
                let run_count = Arc::clone(&run_count);
                let first_started = Arc::clone(&first_started);
                let second_completed = Arc::clone(&second_completed);
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
                    }

                    active_count.fetch_sub(1, Ordering::SeqCst);
                    if run == 2 {
                        second_completed.notify_one();
                    }
                }
            }
        });

        reload.request();
        timeout(Duration::from_secs(1), first_started.notified())
            .await
            .unwrap();
        reload.request();
        sleep(Duration::from_millis(50)).await;

        assert_eq!(run_count.load(Ordering::SeqCst), 1);
        assert!(!overlap_detected.load(Ordering::SeqCst));

        release_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), second_completed.notified())
            .await
            .unwrap();

        assert_eq!(run_count.load(Ordering::SeqCst), 2);
        assert!(!overlap_detected.load(Ordering::SeqCst));
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

        assert!(outcome.is_none());
    }
}
