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

use djls_project::Db as ProjectDb;
use djls_project::apply_refresh;
use djls_project::compute_refresh;
use djls_project::project_template_files;
use djls_semantic::Db as SemanticDb;
use tokio::sync::Mutex;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;

use crate::document::TextDocument;
use crate::ext::UriExt;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

fn refresh_is_stale(refresh_epoch: &AtomicU64, epoch: u64) -> bool {
    refresh_epoch.load(Ordering::Acquire) != epoch
}

pub(crate) async fn run_project_refresh(
    session: Arc<Mutex<Session>>,
    client: Client,
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    epoch: u64,
    log_initialization: bool,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let result = run_project_refresh_inner(
        session,
        client,
        refresh_epoch,
        diagnostic_publish_lock,
        epoch,
    )
    .await;

    if log_initialization {
        tracing::info!("Server initialization completed in {:?}", start.elapsed());
    } else if result.is_ok() {
        tracing::info!("Environment refresh completed in {:?}", start.elapsed());
    }

    result
}

async fn run_project_refresh_inner(
    session: Arc<Mutex<Session>>,
    client: Client,
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    epoch: u64,
) -> anyhow::Result<()> {
    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before locking session"
        );
        return Ok(());
    }

    let Some(compute_db) = ({
        let session_lock = session.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(
                epoch,
                "Skipping stale project refresh after locking session"
            );
            return Ok(());
        }

        let db = session_lock.db();
        db.project().map(|_| db.clone())
    }) else {
        tracing::info!("Task: No project configured, skipping initialization.");
        return Ok(());
    };

    let refresh = tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| compute_refresh(&compute_db)))
    })
    .await
    .expect("project refresh compute task must not panic");

    let Some(refresh) = (match refresh {
        Ok(refresh) => refresh,
        Err(cancelled) => {
            tracing::debug!(
                ?cancelled,
                "Project refresh compute cancelled; newer inputs will re-run refresh"
            );
            return Ok(());
        }
    }) else {
        return Ok(());
    };

    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh before apply");
        return Ok(());
    }

    let (snapshot, documents) = {
        let mut session_lock = session.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh after apply lock");
            return Ok(());
        }

        let db = session_lock.db_mut();
        if db.project().is_none() {
            return Ok(());
        }

        let t = std::time::Instant::now();
        apply_refresh(db, refresh);
        tracing::info!("External data refresh completed in {:?}", t.elapsed());

        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh after apply");
            return Ok(());
        }

        (session_lock.snapshot(), session_lock.open_documents())
    };

    warm_project_queries(snapshot.clone(), Arc::clone(&refresh_epoch), epoch).await;
    republish_snapshot_diagnostics(
        client,
        snapshot,
        documents,
        refresh_epoch,
        diagnostic_publish_lock,
        epoch,
    )
    .await;

    Ok(())
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
) {
    if snapshot.client_info().supports_pull_diagnostics() {
        tracing::debug!("Client supports pull diagnostics, skipping refresh diagnostics push");
        return;
    }

    for document in documents {
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics republish");
            return;
        }

        let file = document.file();
        let Some(diagnostics) = collect_snapshot_diagnostics(snapshot.clone(), file).await else {
            continue;
        };

        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics publish");
            return;
        }

        let Some(lsp_uri) = ls_types::Uri::from_path(document.path()) else {
            continue;
        };

        let diagnostic_count = diagnostics.len();
        let lsp_uri_text = lsp_uri.to_string();
        let _publish_guard = diagnostic_publish_lock.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics publish");
            return;
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
