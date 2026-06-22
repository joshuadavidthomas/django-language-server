//! Background project refresh.
//!
//! Runs expensive refresh work off the session lock: load settings on a
//! blocking task, compute project facts on a database clone, apply the results
//! under the lock, then warm derived queries and republish diagnostics from a
//! snapshot. The session's refresh epoch is checked between stages so
//! superseded work is dropped on the floor.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;

#[cfg(test)]
use camino::Utf8Path;
#[cfg(test)]
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_project::RefreshData;
use djls_project::apply_refresh;
use djls_project::compute_refresh;
use djls_project::project_template_files;
use djls_semantic::Db as SemanticDb;
use tokio::sync::Mutex;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::UriExt;
use crate::progress::LoadProgress;
use crate::session::ProjectRefreshState;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

enum RefreshOutcome {
    Complete,
    Skipped,
    Superseded,
}

enum LoadOutcome {
    Loaded(Box<Settings>),
    Skipped,
    Superseded,
}

enum ComputeOutcome {
    Computed(RefreshData),
    Skipped,
    Superseded,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefreshTestPoint {
    SettingsCaptured,
    ComputeCloned,
}

#[cfg(test)]
struct RefreshTestHook {
    project_root: Utf8PathBuf,
    point: RefreshTestPoint,
    reached: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[cfg(test)]
impl RefreshTestHook {
    fn new(project_root: Utf8PathBuf, point: RefreshTestPoint) -> Arc<Self> {
        Arc::new(Self {
            project_root,
            point,
            reached: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        })
    }

    async fn wait_until_reached(&self) {
        self.reached.notified().await;
    }

    fn resume(&self) {
        self.resume.notify_one();
    }

    async fn pause_if_matches(&self, project_root: &Utf8Path, point: RefreshTestPoint) {
        if self.point != point || self.project_root != project_root {
            return;
        }

        self.reached.notify_one();
        self.resume.notified().await;
    }
}

#[cfg(test)]
struct RefreshTestHookGuard;

#[cfg(test)]
impl Drop for RefreshTestHookGuard {
    fn drop(&mut self) {
        let mut hook = REFRESH_TEST_HOOK.lock().unwrap();
        *hook = None;
    }
}

#[cfg(test)]
static REFRESH_TEST_HOOK: StdMutex<Option<Arc<RefreshTestHook>>> = StdMutex::new(None);

#[cfg(test)]
fn install_refresh_test_hook(hook: Arc<RefreshTestHook>) -> RefreshTestHookGuard {
    let mut installed = REFRESH_TEST_HOOK.lock().unwrap();
    assert!(installed.is_none(), "refresh test hook already installed");
    *installed = Some(hook);
    RefreshTestHookGuard
}

#[cfg(test)]
async fn pause_for_refresh_test(project_root: &Utf8Path, point: RefreshTestPoint) {
    let hook = REFRESH_TEST_HOOK.lock().unwrap().clone();
    if let Some(hook) = hook {
        hook.pause_if_matches(project_root, point).await;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectRefreshReason {
    Startup,
    ConfigurationChanged,
}

impl ProjectRefreshReason {
    fn progress_title(self) -> &'static str {
        match self {
            Self::Startup => "Loading Django project",
            Self::ConfigurationChanged => "Refreshing Django project",
        }
    }

    fn completion_log(self) -> &'static str {
        match self {
            Self::Startup => "Server initialization completed",
            Self::ConfigurationChanged => "Project refresh completed",
        }
    }
}

pub(crate) struct ProjectRefreshRequest {
    project_refresh: ProjectRefreshState,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    client_info: ClientInfo,
    epoch: u64,
    reason: ProjectRefreshReason,
}

impl ProjectRefreshRequest {
    pub(crate) fn new(
        project_refresh: ProjectRefreshState,
        diagnostic_publish_lock: Arc<Mutex<()>>,
        client_info: ClientInfo,
        epoch: u64,
        reason: ProjectRefreshReason,
    ) -> Self {
        Self {
            project_refresh,
            diagnostic_publish_lock,
            client_info,
            epoch,
            reason,
        }
    }
}

pub(crate) async fn run_project_refresh(
    session: Arc<Mutex<Session>>,
    client: Client,
    request: ProjectRefreshRequest,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let progress = LoadProgress::begin(
        client.clone(),
        &request.client_info,
        request.reason.progress_title(),
    )
    .await;
    let result = run_project_refresh_inner(
        session,
        client,
        &request.project_refresh,
        request.diagnostic_publish_lock,
        &progress,
        request.epoch,
    )
    .await;

    match &result {
        Ok(RefreshOutcome::Complete) => {
            progress.finish("complete").await;
        }
        Ok(RefreshOutcome::Skipped) => {
            progress.finish("skipped").await;
        }
        Ok(RefreshOutcome::Superseded) => {
            progress.finish("superseded").await;
        }
        Err(_) => {
            progress.finish("failed").await;
        }
    }

    if result.is_ok() {
        tracing::info!(
            "{} in {:?}",
            request.reason.completion_log(),
            start.elapsed()
        );
    }

    result.map(|_| ())
}

async fn run_project_refresh_inner(
    session: Arc<Mutex<Session>>,
    client: Client,
    project_refresh: &ProjectRefreshState,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    progress: &LoadProgress,
    epoch: u64,
) -> anyhow::Result<RefreshOutcome> {
    if project_refresh.is_stale(epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before locking session"
        );
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Resolving environment").await;

    let settings = match load_project_settings(&session, project_refresh, epoch).await {
        LoadOutcome::Loaded(settings) => *settings,
        LoadOutcome::Skipped => return Ok(RefreshOutcome::Skipped),
        LoadOutcome::Superseded => return Ok(RefreshOutcome::Superseded),
    };

    if project_refresh.is_stale(epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before settings apply"
        );
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Applying project settings").await;

    if let Err(outcome) = apply_project_settings(&session, project_refresh, epoch, settings).await {
        return Ok(outcome);
    }

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after settings apply");
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Computing project facts").await;

    let refresh = match compute_project_refresh(&session, project_refresh, epoch).await {
        ComputeOutcome::Computed(refresh) => refresh,
        ComputeOutcome::Skipped => return Ok(RefreshOutcome::Skipped),
        ComputeOutcome::Superseded => return Ok(RefreshOutcome::Superseded),
    };

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh before apply");
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Applying project facts").await;

    let (snapshot, documents) =
        match apply_project_facts(&session, project_refresh, epoch, refresh).await {
            Ok(snapshot) => snapshot,
            Err(outcome) => return Ok(outcome),
        };

    progress.report("Warming caches").await;
    warm_project_queries(snapshot.clone(), project_refresh.clone(), epoch).await;

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after warm-up");
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Publishing diagnostics").await;
    if !republish_snapshot_diagnostics(
        client,
        snapshot,
        documents,
        project_refresh.clone(),
        diagnostic_publish_lock,
        epoch,
    )
    .await
    {
        return Ok(RefreshOutcome::Superseded);
    }

    Ok(RefreshOutcome::Complete)
}

async fn apply_project_settings(
    session: &Arc<Mutex<Session>>,
    project_refresh: &ProjectRefreshState,
    epoch: u64,
    settings: Settings,
) -> Result<(), RefreshOutcome> {
    let mut session_lock = session.lock().await;
    if project_refresh.is_stale(epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh after settings apply lock"
        );
        return Err(RefreshOutcome::Superseded);
    }

    let db = session_lock.db_mut();
    if db.project().is_none() {
        return Err(RefreshOutcome::Skipped);
    }

    db.apply_project_settings(settings);
    Ok(())
}

async fn apply_project_facts(
    session: &Arc<Mutex<Session>>,
    project_refresh: &ProjectRefreshState,
    epoch: u64,
    refresh: RefreshData,
) -> Result<(SessionSnapshot, Vec<TextDocument>), RefreshOutcome> {
    let mut session_lock = session.lock().await;
    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after apply lock");
        return Err(RefreshOutcome::Superseded);
    }

    let db = session_lock.db_mut();
    if db.project().is_none() {
        return Err(RefreshOutcome::Skipped);
    }

    let t = std::time::Instant::now();
    apply_refresh(db, refresh);
    tracing::info!("External data refresh completed in {:?}", t.elapsed());

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after apply");
        return Err(RefreshOutcome::Superseded);
    }

    Ok((session_lock.snapshot(), session_lock.open_documents()))
}

async fn load_project_settings(
    session: &Arc<Mutex<Session>>,
    project_refresh: &ProjectRefreshState,
    epoch: u64,
) -> LoadOutcome {
    let Some((project_root, config_overrides)) = ({
        let session_lock = session.lock().await;
        if project_refresh.is_stale(epoch) {
            tracing::debug!(
                epoch,
                "Skipping stale project settings load after locking session"
            );
            return LoadOutcome::Superseded;
        }

        let db = session_lock.db();
        db.project().map(|project| {
            (
                project.root(db).clone(),
                session_lock.client_info().config_overrides().clone(),
            )
        })
    }) else {
        tracing::info!("Task: No project configured, skipping settings load.");
        return LoadOutcome::Skipped;
    };

    #[cfg(test)]
    pause_for_refresh_test(&project_root, RefreshTestPoint::SettingsCaptured).await;

    let settings =
        tokio::task::spawn_blocking(move || Settings::new(&project_root, Some(config_overrides)))
            .await
            .expect("project settings load task must not panic");

    let settings = match settings {
        Ok(settings) => settings,
        Err(err) => {
            tracing::error!("Error loading settings: {}", err);
            return LoadOutcome::Skipped;
        }
    };

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after settings load");
        return LoadOutcome::Superseded;
    }

    LoadOutcome::Loaded(Box::new(settings))
}

async fn compute_project_refresh(
    session: &Arc<Mutex<Session>>,
    project_refresh: &ProjectRefreshState,
    epoch: u64,
) -> ComputeOutcome {
    // Cancellation here usually means a document edit, not a config change:
    // nothing bumps the epoch or resubmits, so dropping the compute would lose
    // the refresh for good. Retry with a fresh database clone instead, like
    // the snapshot reads do.
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let Some(compute_db) = ({
            let session_lock = session.lock().await;
            if project_refresh.is_stale(epoch) {
                tracing::debug!(
                    epoch,
                    "Skipping stale project refresh after locking session"
                );
                return ComputeOutcome::Superseded;
            }

            let db = session_lock.db();
            db.project().map(|_| db.clone())
        }) else {
            tracing::info!("Task: No project configured, skipping initialization.");
            return ComputeOutcome::Skipped;
        };

        #[cfg(test)]
        if let Some(project) = compute_db.project() {
            pause_for_refresh_test(project.root(&compute_db), RefreshTestPoint::ComputeCloned)
                .await;
        }

        let result = tokio::task::spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| compute_refresh(&compute_db)))
        })
        .await
        .expect("project refresh compute task must not panic");

        match result {
            Ok(Some(refresh)) => return ComputeOutcome::Computed(refresh),
            Ok(None) => return ComputeOutcome::Skipped,
            Err(cancelled) if attempt < SNAPSHOT_CANCEL_RETRIES => {
                tracing::debug!(
                    ?cancelled,
                    attempt = attempt + 1,
                    "Project refresh compute cancelled; retrying with fresh database clone"
                );
            }
            Err(cancelled) => {
                tracing::warn!(
                    ?cancelled,
                    retries = SNAPSHOT_CANCEL_RETRIES,
                    "Project refresh compute cancelled repeatedly; project facts may be stale until the next refresh"
                );
                return ComputeOutcome::Skipped;
            }
        }
    }

    unreachable!("project refresh retry loop must return")
}

async fn warm_project_queries(
    snapshot: SessionSnapshot,
    project_refresh: ProjectRefreshState,
    epoch: u64,
) {
    let result = tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            let db = snapshot.db();
            let Some(project) = db.project() else {
                return;
            };

            if project_refresh.is_stale(epoch) {
                return;
            }
            let _ = db.tag_specs();

            if project_refresh.is_stale(epoch) {
                return;
            }
            let _ = db.template_dirs();

            if project_refresh.is_stale(epoch) {
                return;
            }
            let _ = db.template_libraries();

            if project_refresh.is_stale(epoch) {
                return;
            }
            let _ = project_template_files(db, project);
        }))
    })
    .await
    .expect("project warm-up task must not panic");

    if let Err(cancelled) = result {
        tracing::debug!(
            ?cancelled,
            "Project refresh warm-up cancelled; newer inputs will re-warm queries"
        );
    }
}

async fn republish_snapshot_diagnostics(
    client: Client,
    snapshot: SessionSnapshot,
    documents: Vec<TextDocument>,
    project_refresh: ProjectRefreshState,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    epoch: u64,
) -> bool {
    if snapshot.client_info().supports_pull_diagnostics() {
        tracing::debug!("Client supports pull diagnostics, skipping refresh diagnostics push");
        return true;
    }

    for document in documents {
        if project_refresh.is_stale(epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics republish");
            return false;
        }

        let file = document.file();
        let Some(diagnostics) = collect_snapshot_diagnostics(snapshot.clone(), file).await else {
            continue;
        };

        if project_refresh.is_stale(epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics publish");
            return false;
        }

        let Some(lsp_uri) = ls_types::Uri::from_path(document.path()) else {
            continue;
        };

        let diagnostic_count = diagnostics.len();
        let lsp_uri_text = lsp_uri.to_string();
        let _publish_guard = diagnostic_publish_lock.lock().await;
        if project_refresh.is_stale(epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics publish");
            return false;
        }
        client
            .publish_diagnostics(lsp_uri, diagnostics, Some(document.version()))
            .await;

        tracing::debug!(
            "Published {} diagnostics for {}",
            diagnostic_count,
            lsp_uri_text
        );
    }

    true
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
                    "Refresh diagnostics cancelled; retrying with same snapshot"
                );
            }
            Err(cancelled) => {
                tracing::debug!(
                    ?cancelled,
                    retries = SNAPSHOT_CANCEL_RETRIES,
                    "Refresh diagnostics cancelled; skipping diagnostics republish"
                );
                return None;
            }
        }
    }

    unreachable!("diagnostics retry loop must return")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    use super::*;

    static REFRESH_TEST_HOOK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn initialize_params(root: &Utf8Path) -> ls_types::InitializeParams {
        ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path()).unwrap(),
                name: "test_project".to_string(),
            }]),
            ..Default::default()
        }
    }

    fn session_for_root(root: &Utf8Path) -> Session {
        Session::new(&initialize_params(root))
    }

    async fn snapshot_while_refresh_is_paused(session: &Arc<Mutex<Session>>) -> SessionSnapshot {
        tokio::time::timeout(Duration::from_secs(1), async {
            session.lock().await.snapshot()
        })
        .await
        .expect("session snapshot should be available while project refresh is paused")
    }

    async fn wait_for_refresh_pause(hook: &RefreshTestHook) {
        tokio::time::timeout(Duration::from_secs(1), hook.wait_until_reached())
            .await
            .expect("refresh test hook should reach its pause point");
    }

    #[tokio::test]
    async fn startup_settings_load_does_not_block_session_snapshots() {
        let _serial = REFRESH_TEST_HOOK_LOCK.lock().await;
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        let session = session_for_root(root.as_path());
        let project_refresh = session.project_refresh().clone();
        let epoch = project_refresh.begin_refresh();
        let session = Arc::new(Mutex::new(session));
        let hook = RefreshTestHook::new(root.clone(), RefreshTestPoint::SettingsCaptured);
        let _hook_guard = install_refresh_test_hook(Arc::clone(&hook));

        let settings_load = tokio::spawn({
            let session = Arc::clone(&session);
            let project_refresh = project_refresh.clone();
            async move { load_project_settings(&session, &project_refresh, epoch).await }
        });

        wait_for_refresh_pause(&hook).await;

        let snapshot = snapshot_while_refresh_is_paused(&session).await;
        assert!(snapshot.db().project().is_some());

        hook.resume();
        let outcome = settings_load.await.unwrap();
        assert!(matches!(outcome, LoadOutcome::Loaded(_)));
    }

    #[tokio::test]
    async fn startup_refresh_compute_does_not_block_session_snapshots() {
        let _serial = REFRESH_TEST_HOOK_LOCK.lock().await;
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        let session = session_for_root(root.as_path());
        let project_refresh = session.project_refresh().clone();
        let epoch = project_refresh.begin_refresh();
        let session = Arc::new(Mutex::new(session));
        let hook = RefreshTestHook::new(root.clone(), RefreshTestPoint::ComputeCloned);
        let _hook_guard = install_refresh_test_hook(Arc::clone(&hook));

        let refresh_compute = tokio::spawn({
            let session = Arc::clone(&session);
            let project_refresh = project_refresh.clone();
            async move { compute_project_refresh(&session, &project_refresh, epoch).await }
        });

        wait_for_refresh_pause(&hook).await;

        let snapshot = snapshot_while_refresh_is_paused(&session).await;
        assert!(snapshot.db().project().is_some());

        hook.resume();
        let outcome = refresh_compute.await.unwrap();
        assert!(matches!(outcome, ComputeOutcome::Computed(_)));
    }

    #[tokio::test]
    async fn startup_superseded_settings_load_is_dropped_before_apply() {
        let _serial = REFRESH_TEST_HOOK_LOCK.lock().await;
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"config.settings\"\n",
        )
        .unwrap();

        let session = session_for_root(root.as_path());
        let project_refresh = session.project_refresh().clone();
        let epoch = project_refresh.begin_refresh();
        let session = Arc::new(Mutex::new(session));
        let hook = RefreshTestHook::new(root.clone(), RefreshTestPoint::SettingsCaptured);
        let _hook_guard = install_refresh_test_hook(Arc::clone(&hook));

        let settings_load = tokio::spawn({
            let session = Arc::clone(&session);
            let project_refresh = project_refresh.clone();
            async move { load_project_settings(&session, &project_refresh, epoch).await }
        });

        wait_for_refresh_pause(&hook).await;
        project_refresh.begin_refresh();
        hook.resume();

        let outcome = settings_load.await.unwrap();
        assert!(matches!(outcome, LoadOutcome::Superseded));

        let stale_settings = Settings::new(root.as_path(), None).unwrap();
        let outcome =
            apply_project_settings(&session, &project_refresh, epoch, stale_settings).await;
        assert!(matches!(outcome, Err(RefreshOutcome::Superseded)));

        let session_lock = session.lock().await;
        let db = session_lock.db();
        let project = db.project().expect("project should exist");
        assert_eq!(project.django_settings_module(db).as_deref(), None);
    }

    #[tokio::test]
    async fn startup_superseded_project_facts_are_dropped_before_apply() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        let session = session_for_root(root.as_path());
        let project_refresh = session.project_refresh().clone();
        let epoch = project_refresh.begin_refresh();
        let session = Arc::new(Mutex::new(session));

        let refresh = match compute_project_refresh(&session, &project_refresh, epoch).await {
            ComputeOutcome::Computed(refresh) => refresh,
            ComputeOutcome::Skipped => panic!("project refresh should have project facts"),
            ComputeOutcome::Superseded => panic!("project refresh should not be superseded yet"),
        };

        project_refresh.begin_refresh();

        let outcome = apply_project_facts(&session, &project_refresh, epoch, refresh).await;
        assert!(matches!(outcome, Err(RefreshOutcome::Superseded)));
    }

    #[tokio::test]
    async fn project_settings_load_error_skips_refresh_inputs() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "debug = not_a_boolean",
        )
        .unwrap();

        let session = session_for_root(root.as_path());
        let project_refresh = session.project_refresh().clone();
        let epoch = project_refresh.begin_refresh();
        let session = Arc::new(Mutex::new(session));

        let outcome = load_project_settings(&session, &project_refresh, epoch).await;

        assert!(matches!(outcome, LoadOutcome::Skipped));
    }
}
