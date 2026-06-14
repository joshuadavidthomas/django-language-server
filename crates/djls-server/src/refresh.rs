//! Background project refresh.
//!
//! Runs off the session lock as much as possible: compute the refresh on a
//! database clone, apply it briefly under the lock, then warm derived queries
//! and republish diagnostics from a snapshot. The session's refresh epoch is
//! checked between stages so superseded work is dropped on the floor.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_project::ProjectInputData;
use djls_project::RefreshData;
use djls_project::apply_refresh;
use djls_project::compute_refresh;
use djls_project::project_template_files;
use djls_semantic::Db as SemanticDb;
use tokio::sync::Mutex;
use tokio::sync::watch;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::UriExt;
use crate::progress::LoadProgress;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

enum RefreshOutcome {
    Complete,
    Skipped,
    Superseded,
}

struct LoadedProjectSettings {
    settings: Settings,
    project_inputs: ProjectInputData,
}

enum LoadOutcome {
    Loaded(Box<LoadedProjectSettings>),
    Skipped,
    Superseded,
}

enum ComputeOutcome {
    Computed(RefreshData),
    Skipped,
    Superseded,
}

pub(crate) struct ProjectRefreshRequest {
    refresh_epoch: Arc<AtomicU64>,
    refresh_completion: watch::Sender<u64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    client_info: ClientInfo,
    epoch: u64,
    log_initialization: bool,
}

impl ProjectRefreshRequest {
    pub(crate) fn new(
        refresh_epoch: Arc<AtomicU64>,
        refresh_completion: watch::Sender<u64>,
        diagnostic_publish_lock: Arc<Mutex<()>>,
        client_info: ClientInfo,
        epoch: u64,
        log_initialization: bool,
    ) -> Self {
        Self {
            refresh_epoch,
            refresh_completion,
            diagnostic_publish_lock,
            client_info,
            epoch,
            log_initialization,
        }
    }
}

fn refresh_is_stale(refresh_epoch: &AtomicU64, epoch: u64) -> bool {
    refresh_epoch.load(Ordering::Acquire) != epoch
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
        "Loading Django project",
    )
    .await;
    let result = run_project_refresh_inner(
        session,
        client,
        request.refresh_epoch,
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

    if request.log_initialization {
        tracing::info!("Server initialization completed in {:?}", start.elapsed());
    } else if result.is_ok() {
        tracing::info!("Environment refresh completed in {:?}", start.elapsed());
    }

    if !matches!(result, Ok(RefreshOutcome::Superseded)) {
        let _ = request.refresh_completion.send(request.epoch);
    }

    result.map(|_| ())
}

async fn run_project_refresh_inner(
    session: Arc<Mutex<Session>>,
    client: Client,
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    progress: &LoadProgress,
    epoch: u64,
) -> anyhow::Result<RefreshOutcome> {
    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before locking session"
        );
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Resolving environment").await;

    let loaded_settings = match load_project_settings(&session, &refresh_epoch, epoch).await {
        LoadOutcome::Loaded(loaded_settings) => *loaded_settings,
        LoadOutcome::Skipped => return Ok(RefreshOutcome::Skipped),
        LoadOutcome::Superseded => return Ok(RefreshOutcome::Superseded),
    };

    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before settings apply"
        );
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Applying project settings").await;

    {
        let mut session_lock = session.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(
                epoch,
                "Skipping stale project refresh after settings apply lock"
            );
            return Ok(RefreshOutcome::Superseded);
        }

        let db = session_lock.db_mut();
        if db.project().is_none() {
            return Ok(RefreshOutcome::Skipped);
        }

        db.apply_loaded_settings(loaded_settings.settings, loaded_settings.project_inputs);
    }

    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after settings apply");
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Computing project facts").await;

    let refresh = match compute_project_refresh(&session, &refresh_epoch, epoch).await {
        ComputeOutcome::Computed(refresh) => refresh,
        ComputeOutcome::Skipped => return Ok(RefreshOutcome::Skipped),
        ComputeOutcome::Superseded => return Ok(RefreshOutcome::Superseded),
    };

    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh before apply");
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Applying project facts").await;

    let (snapshot, documents) = {
        let mut session_lock = session.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh after apply lock");
            return Ok(RefreshOutcome::Superseded);
        }

        let db = session_lock.db_mut();
        if db.project().is_none() {
            return Ok(RefreshOutcome::Skipped);
        }

        let t = std::time::Instant::now();
        apply_refresh(db, refresh);
        tracing::info!("External data refresh completed in {:?}", t.elapsed());

        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh after apply");
            return Ok(RefreshOutcome::Superseded);
        }

        (session_lock.snapshot(), session_lock.open_documents())
    };

    progress.report("Warming caches").await;
    warm_project_queries(snapshot.clone(), Arc::clone(&refresh_epoch), epoch).await;

    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after warm-up");
        return Ok(RefreshOutcome::Superseded);
    }

    progress.report("Publishing diagnostics").await;
    if !republish_snapshot_diagnostics(
        client,
        snapshot,
        documents,
        refresh_epoch,
        diagnostic_publish_lock,
        epoch,
    )
    .await
    {
        return Ok(RefreshOutcome::Superseded);
    }

    Ok(RefreshOutcome::Complete)
}

async fn load_project_settings(
    session: &Arc<Mutex<Session>>,
    refresh_epoch: &Arc<AtomicU64>,
    epoch: u64,
) -> LoadOutcome {
    let Some((project_root, config_overrides, load_db)) = ({
        let session_lock = session.lock().await;
        if refresh_is_stale(refresh_epoch, epoch) {
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
                db.clone(),
            )
        })
    }) else {
        tracing::info!("Task: No project configured, skipping settings load.");
        return LoadOutcome::Skipped;
    };

    let loaded_settings = tokio::task::spawn_blocking(move || {
        let settings = match Settings::new(&project_root, Some(config_overrides.clone())) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::error!("Error loading settings: {}", err);
                config_overrides
            }
        };
        let project_inputs = ProjectInputData::load(&load_db, &project_root, &settings);

        LoadedProjectSettings {
            settings,
            project_inputs,
        }
    })
    .await
    .expect("project settings load task must not panic");

    if refresh_is_stale(refresh_epoch, epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after settings load");
        return LoadOutcome::Superseded;
    }

    LoadOutcome::Loaded(Box::new(loaded_settings))
}

async fn compute_project_refresh(
    session: &Arc<Mutex<Session>>,
    refresh_epoch: &Arc<AtomicU64>,
    epoch: u64,
) -> ComputeOutcome {
    // Cancellation here usually means a document edit, not a config change:
    // nothing bumps the epoch or resubmits, so dropping the compute would lose
    // the refresh for good. Retry with a fresh database clone instead, like
    // the snapshot reads do.
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let Some(compute_db) = ({
            let session_lock = session.lock().await;
            if refresh_is_stale(refresh_epoch, epoch) {
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
    refresh_epoch: Arc<AtomicU64>,
    epoch: u64,
) {
    let result = tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            let db = snapshot.db();
            let Some(project) = db.project() else {
                return;
            };

            if refresh_is_stale(&refresh_epoch, epoch) {
                return;
            }
            let _ = db.tag_specs();

            if refresh_is_stale(&refresh_epoch, epoch) {
                return;
            }
            let _ = db.template_dirs();

            if refresh_is_stale(&refresh_epoch, epoch) {
                return;
            }
            let _ = db.template_libraries();

            if refresh_is_stale(&refresh_epoch, epoch) {
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
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    epoch: u64,
) -> bool {
    if snapshot.client_info().supports_pull_diagnostics() {
        tracing::debug!("Client supports pull diagnostics, skipping refresh diagnostics push");
        return true;
    }

    for document in documents {
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics republish");
            return false;
        }

        let file = document.file();
        let Some(diagnostics) = collect_snapshot_diagnostics(snapshot.clone(), file).await else {
            continue;
        };

        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics publish");
            return false;
        }

        let Some(lsp_uri) = ls_types::Uri::from_path(document.path()) else {
            continue;
        };

        let diagnostic_count = diagnostics.len();
        let lsp_uri_text = lsp_uri.to_string();
        let _publish_guard = diagnostic_publish_lock.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
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
