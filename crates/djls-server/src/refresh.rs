//! Background project refresh.
//!
//! Runs expensive refresh work off the session lock: load settings on a
//! blocking task, compute project facts on a database clone, apply the results
//! under the lock, then warm derived queries and republish diagnostics from a
//! snapshot. The session's refresh epoch is checked between stages so
//! superseded work is dropped on the floor.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_project::Db as ProjectDb;
use djls_project::Project;
use djls_project::RefreshData;
use djls_project::RefreshStage;
use djls_project::SearchPaths;
use djls_project::apply_refresh;
use djls_project::compute_refresh_model_module_paths;
use djls_project::compute_refresh_search_paths;
use djls_project::compute_refresh_settings_source_paths;
use djls_project::compute_refresh_template_library_module_paths;
use djls_project::compute_refresh_template_tag_candidate_paths;
use djls_project::project_template_files;
use djls_semantic::Db as SemanticDb;
use tokio::sync::Mutex;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::UriExt;
use crate::progress::ProgressItem;
use crate::progress::ProgressReporter;
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

enum CaptureOutcome {
    Captured {
        db: DjangoDatabase,
        project: Project,
    },
    Skipped,
    Superseded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectRefreshReason {
    Startup,
    ConfigurationChanged,
}

impl ProjectRefreshReason {
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

const RESOLVE_ENVIRONMENT_TITLE: &str = "Resolving Django environment";
const DISCOVER_PROJECT_FACTS_TITLE: &str = "Discovering Django project facts";
const WARM_CACHES_TITLE: &str = "Warming Django caches";
const PUBLISH_DIAGNOSTICS_TITLE: &str = "Publishing diagnostics";

pub(crate) async fn run_project_refresh(
    session: Arc<Mutex<Session>>,
    client: Client,
    request: ProjectRefreshRequest,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let progress = ProgressReporter::new(client.clone(), request.client_info.clone());
    let result = run_project_refresh_inner(
        session,
        client,
        &request.project_refresh,
        request.diagnostic_publish_lock,
        &progress,
        request.epoch,
    )
    .await;

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
    progress: &ProgressReporter,
    epoch: u64,
) -> anyhow::Result<RefreshOutcome> {
    if project_refresh.is_stale(epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before locking session"
        );
        return Ok(RefreshOutcome::Superseded);
    }

    // Start visible progress before touching the session. Clients often send
    // didOpen/completion immediately after initialized; progress setup should
    // not sit behind a session snapshot in that race.
    let mut environment_progress = Some(progress.begin(RESOLVE_ENVIRONMENT_TITLE).await);
    if let Some(progress) = environment_progress.as_ref() {
        progress.report("Resolving environment").await;
    }

    if let Err(outcome) =
        load_and_apply_project_settings(&session, project_refresh, epoch, &mut environment_progress)
            .await
    {
        return Ok(outcome);
    }

    let mut facts_progress = None;
    let refresh = match compute_project_refresh(
        &session,
        project_refresh,
        epoch,
        progress,
        &mut environment_progress,
        &mut facts_progress,
    )
    .await
    {
        ComputeOutcome::Computed(refresh) => refresh,
        ComputeOutcome::Skipped => return Ok(RefreshOutcome::Skipped),
        ComputeOutcome::Superseded => return Ok(RefreshOutcome::Superseded),
    };

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh before apply");
        finish_progress(&mut facts_progress, "superseded").await;
        return Ok(RefreshOutcome::Superseded);
    }

    if facts_progress.is_none() {
        facts_progress = Some(progress.begin(DISCOVER_PROJECT_FACTS_TITLE).await);
    }
    if let Some(progress) = facts_progress.as_ref() {
        progress.report("Applying project facts").await;
    }

    let (snapshot, documents) =
        match apply_project_facts(&session, project_refresh, epoch, refresh).await {
            Ok(snapshot) => {
                finish_progress(&mut facts_progress, "complete").await;
                snapshot
            }
            Err(outcome) => {
                finish_progress(&mut facts_progress, outcome.progress_message()).await;
                return Ok(outcome);
            }
        };

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh before warm-up");
        return Ok(RefreshOutcome::Superseded);
    }

    let warm_progress = progress.begin(WARM_CACHES_TITLE).await;
    warm_project_queries(
        snapshot.clone(),
        project_refresh.clone(),
        epoch,
        &warm_progress,
    )
    .await;

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after warm-up");
        warm_progress.finish("superseded").await;
        return Ok(RefreshOutcome::Superseded);
    }
    warm_progress.finish("complete").await;

    let diagnostics_progress = progress.begin(PUBLISH_DIAGNOSTICS_TITLE).await;
    diagnostics_progress.report("Publishing diagnostics").await;
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
        diagnostics_progress.finish("superseded").await;
        return Ok(RefreshOutcome::Superseded);
    }
    diagnostics_progress.finish("complete").await;

    Ok(RefreshOutcome::Complete)
}

impl RefreshOutcome {
    fn progress_message(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Skipped => "skipped",
            Self::Superseded => "superseded",
        }
    }
}

async fn load_and_apply_project_settings(
    session: &Arc<Mutex<Session>>,
    project_refresh: &ProjectRefreshState,
    epoch: u64,
    progress: &mut Option<ProgressItem>,
) -> Result<(), RefreshOutcome> {
    let settings = match load_project_settings(session, project_refresh, epoch).await {
        LoadOutcome::Loaded(settings) => *settings,
        LoadOutcome::Skipped => {
            finish_progress(progress, "skipped").await;
            return Err(RefreshOutcome::Skipped);
        }
        LoadOutcome::Superseded => {
            finish_progress(progress, "superseded").await;
            return Err(RefreshOutcome::Superseded);
        }
    };

    if project_refresh.is_stale(epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before settings apply"
        );
        finish_progress(progress, "superseded").await;
        return Err(RefreshOutcome::Superseded);
    }

    if let Some(progress) = progress.as_ref() {
        progress.report("Applying project settings").await;
    }

    if let Err(outcome) = apply_project_settings(session, project_refresh, epoch, settings).await {
        finish_progress(progress, outcome.progress_message()).await;
        return Err(outcome);
    }

    if project_refresh.is_stale(epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh after settings apply");
        finish_progress(progress, "superseded").await;
        return Err(RefreshOutcome::Superseded);
    }

    Ok(())
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

type SearchPathsJobResult = Result<SearchPaths, salsa::Cancelled>;
type FilePathsJobResult = Result<Vec<Utf8PathBuf>, salsa::Cancelled>;
type SearchPathsJobHandle = tokio::task::JoinHandle<SearchPathsJobResult>;
type FilePathsJobHandle = tokio::task::JoinHandle<FilePathsJobResult>;

struct RefreshJobHandles {
    search_paths: SearchPathsJobHandle,
    settings_sources: FilePathsJobHandle,
    model_modules: FilePathsJobHandle,
    template_library_modules: FilePathsJobHandle,
    template_tag_candidates: FilePathsJobHandle,
}

enum RefreshJobsOutcome {
    Computed(RefreshData),
    Cancelled(salsa::Cancelled),
}

async fn compute_project_refresh(
    session: &Arc<Mutex<Session>>,
    project_refresh: &ProjectRefreshState,
    epoch: u64,
    progress: &ProgressReporter,
    environment_progress: &mut Option<ProgressItem>,
    facts_progress: &mut Option<ProgressItem>,
) -> ComputeOutcome {
    // Cancellation here usually means a document edit, not a config change:
    // nothing bumps the epoch or resubmits, so dropping the compute would lose
    // the refresh for good. Retry with a fresh database clone instead, like
    // the snapshot reads do.
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let (compute_db, project) = match capture_refresh_db(session, project_refresh, epoch).await
        {
            CaptureOutcome::Captured { db, project } => (db, project),
            CaptureOutcome::Skipped => {
                finish_progress(environment_progress, "skipped").await;
                finish_progress(facts_progress, "skipped").await;
                return ComputeOutcome::Skipped;
            }
            CaptureOutcome::Superseded => {
                finish_progress(environment_progress, "superseded").await;
                finish_progress(facts_progress, "superseded").await;
                return ComputeOutcome::Superseded;
            }
        };

        if project_refresh.is_stale(epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh before compute jobs");
            finish_progress(environment_progress, "superseded").await;
            finish_progress(facts_progress, "superseded").await;
            return ComputeOutcome::Superseded;
        }

        let handles = spawn_refresh_jobs(
            compute_db,
            project,
            progress,
            environment_progress,
            facts_progress,
        )
        .await;
        let result = collect_refresh_jobs(handles).await;

        if project_refresh.is_stale(epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh after compute jobs");
            finish_progress(environment_progress, "superseded").await;
            finish_progress(facts_progress, "superseded").await;
            return ComputeOutcome::Superseded;
        }

        match result {
            RefreshJobsOutcome::Computed(refresh) => {
                finish_progress(environment_progress, "complete").await;
                return ComputeOutcome::Computed(refresh);
            }
            RefreshJobsOutcome::Cancelled(cancelled) if attempt < SNAPSHOT_CANCEL_RETRIES => {
                finish_progress(environment_progress, "retrying").await;
                finish_progress(facts_progress, "retrying").await;
                tracing::debug!(
                    ?cancelled,
                    attempt = attempt + 1,
                    "Project refresh compute cancelled; retrying with fresh database clone"
                );
            }
            RefreshJobsOutcome::Cancelled(cancelled) => {
                finish_progress(environment_progress, "cancelled").await;
                finish_progress(facts_progress, "cancelled").await;
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

async fn capture_refresh_db(
    session: &Arc<Mutex<Session>>,
    project_refresh: &ProjectRefreshState,
    epoch: u64,
) -> CaptureOutcome {
    let session_lock = session.lock().await;
    if project_refresh.is_stale(epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh after locking session"
        );
        return CaptureOutcome::Superseded;
    }

    let db = session_lock.db();
    let Some(project) = db.project() else {
        tracing::info!("Task: No project configured, skipping initialization.");
        return CaptureOutcome::Skipped;
    };

    CaptureOutcome::Captured {
        db: db.clone(),
        project,
    }
}

async fn spawn_refresh_jobs(
    compute_db: DjangoDatabase,
    project: Project,
    reporter: &ProgressReporter,
    environment_progress: &mut Option<ProgressItem>,
    facts_progress: &mut Option<ProgressItem>,
) -> RefreshJobHandles {
    let search_paths = spawn_refresh_search_paths_job(compute_db.clone(), project);
    let settings_sources = spawn_refresh_settings_source_paths_job(compute_db.clone(), project);
    let model_modules = spawn_refresh_model_module_paths_job(compute_db.clone(), project);
    let template_library_modules =
        spawn_refresh_template_library_module_paths_job(compute_db.clone(), project);
    let template_tag_candidates =
        spawn_refresh_template_tag_candidate_paths_job(compute_db, project);

    report_refresh_stage(
        RefreshStage::ResolveEnvironment,
        reporter,
        environment_progress,
        facts_progress,
    )
    .await;
    report_refresh_stage(
        RefreshStage::ScanSettings,
        reporter,
        environment_progress,
        facts_progress,
    )
    .await;
    report_refresh_stage(
        RefreshStage::DiscoverModelModules,
        reporter,
        environment_progress,
        facts_progress,
    )
    .await;
    report_refresh_stage(
        RefreshStage::DiscoverTemplateLibraries,
        reporter,
        environment_progress,
        facts_progress,
    )
    .await;
    report_refresh_stage(
        RefreshStage::DiscoverTemplateTagCandidates,
        reporter,
        environment_progress,
        facts_progress,
    )
    .await;

    RefreshJobHandles {
        search_paths,
        settings_sources,
        model_modules,
        template_library_modules,
        template_tag_candidates,
    }
}

fn spawn_refresh_search_paths_job(db: DjangoDatabase, project: Project) -> SearchPathsJobHandle {
    tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            compute_refresh_search_paths(&db, project)
        }))
    })
}

fn spawn_refresh_settings_source_paths_job(
    db: DjangoDatabase,
    project: Project,
) -> FilePathsJobHandle {
    tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            compute_refresh_settings_source_paths(&db, project)
        }))
    })
}

fn spawn_refresh_model_module_paths_job(
    db: DjangoDatabase,
    project: Project,
) -> FilePathsJobHandle {
    tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            compute_refresh_model_module_paths(&db, project)
        }))
    })
}

fn spawn_refresh_template_library_module_paths_job(
    db: DjangoDatabase,
    project: Project,
) -> FilePathsJobHandle {
    tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            compute_refresh_template_library_module_paths(&db, project)
        }))
    })
}

fn spawn_refresh_template_tag_candidate_paths_job(
    db: DjangoDatabase,
    project: Project,
) -> FilePathsJobHandle {
    tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            compute_refresh_template_tag_candidate_paths(&db, project)
        }))
    })
}

async fn collect_refresh_jobs(handles: RefreshJobHandles) -> RefreshJobsOutcome {
    let mut cancellation = None;
    let mut file_paths = Vec::new();

    let search_paths = match await_search_paths_job(handles.search_paths).await {
        Ok(search_paths) => Some(search_paths),
        Err(cancelled) => {
            remember_cancellation(&mut cancellation, cancelled);
            None
        }
    };

    collect_file_paths_job(
        RefreshStage::ScanSettings,
        handles.settings_sources,
        &mut file_paths,
        &mut cancellation,
    )
    .await;
    collect_file_paths_job(
        RefreshStage::DiscoverModelModules,
        handles.model_modules,
        &mut file_paths,
        &mut cancellation,
    )
    .await;
    collect_file_paths_job(
        RefreshStage::DiscoverTemplateLibraries,
        handles.template_library_modules,
        &mut file_paths,
        &mut cancellation,
    )
    .await;
    collect_file_paths_job(
        RefreshStage::DiscoverTemplateTagCandidates,
        handles.template_tag_candidates,
        &mut file_paths,
        &mut cancellation,
    )
    .await;

    if let Some(cancelled) = cancellation {
        return RefreshJobsOutcome::Cancelled(cancelled);
    }

    let Some(search_paths) = search_paths else {
        unreachable!("cancelled search-path job returned without cancellation")
    };

    RefreshJobsOutcome::Computed(RefreshData::from_parts(search_paths, file_paths))
}

async fn await_search_paths_job(handle: SearchPathsJobHandle) -> SearchPathsJobResult {
    handle
        .await
        .expect("project refresh search-path task must not panic")
}

async fn collect_file_paths_job(
    stage: RefreshStage,
    handle: FilePathsJobHandle,
    file_paths: &mut Vec<Utf8PathBuf>,
    cancellation: &mut Option<salsa::Cancelled>,
) {
    match await_file_paths_job(stage, handle).await {
        Ok(paths) => file_paths.extend(paths),
        Err(cancelled) => remember_cancellation(cancellation, cancelled),
    }
}

async fn await_file_paths_job(
    stage: RefreshStage,
    handle: FilePathsJobHandle,
) -> FilePathsJobResult {
    handle.await.unwrap_or_else(|error| {
        panic!("project refresh {stage:?} task must not panic: {error}");
    })
}

fn remember_cancellation(cancellation: &mut Option<salsa::Cancelled>, cancelled: salsa::Cancelled) {
    if cancellation.is_none() {
        *cancellation = Some(cancelled);
    }
}

async fn report_refresh_stage(
    stage: RefreshStage,
    reporter: &ProgressReporter,
    environment_progress: &mut Option<ProgressItem>,
    facts_progress: &mut Option<ProgressItem>,
) {
    match stage {
        RefreshStage::ResolveEnvironment | RefreshStage::ScanSettings => {
            if environment_progress.is_none() {
                *environment_progress = Some(reporter.begin(RESOLVE_ENVIRONMENT_TITLE).await);
            }
            if let Some(progress) = environment_progress.as_ref() {
                progress.report(stage.message()).await;
            }
        }
        RefreshStage::DiscoverModelModules
        | RefreshStage::DiscoverTemplateLibraries
        | RefreshStage::DiscoverTemplateTagCandidates => {
            if facts_progress.is_none() {
                *facts_progress = Some(reporter.begin(DISCOVER_PROJECT_FACTS_TITLE).await);
            }
            if let Some(progress) = facts_progress.as_ref() {
                progress.report(stage.message()).await;
            }
        }
    }
}

async fn finish_progress(progress: &mut Option<ProgressItem>, message: &str) {
    if let Some(progress) = progress.take() {
        progress.finish(message).await;
    }
}

type WarmJobResult = Result<(), salsa::Cancelled>;
type WarmJobHandle = tokio::task::JoinHandle<WarmJobResult>;

#[derive(Clone, Copy, Debug)]
enum WarmStage {
    BuildTagSpecs,
    BuildFilterAritySpecs,
    BuildModelGraph,
    ResolveTemplateDirs,
    IndexTemplateLibraries,
    IndexTemplates,
}

impl WarmStage {
    const ALL: [Self; 6] = [
        Self::BuildTagSpecs,
        Self::BuildFilterAritySpecs,
        Self::BuildModelGraph,
        Self::ResolveTemplateDirs,
        Self::IndexTemplateLibraries,
        Self::IndexTemplates,
    ];

    fn message(self) -> &'static str {
        match self {
            Self::BuildTagSpecs => "Building tag specs",
            Self::BuildFilterAritySpecs => "Building filter arity specs",
            Self::BuildModelGraph => "Building model graph",
            Self::ResolveTemplateDirs => "Resolving template directories",
            Self::IndexTemplateLibraries => "Indexing template libraries",
            Self::IndexTemplates => "Indexing templates",
        }
    }

    fn spawn(
        self,
        snapshot: SessionSnapshot,
        project_refresh: ProjectRefreshState,
        epoch: u64,
    ) -> WarmJobHandle {
        tokio::task::spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                self.run(&snapshot, &project_refresh, epoch);
            }))
        })
    }

    fn run(self, snapshot: &SessionSnapshot, project_refresh: &ProjectRefreshState, epoch: u64) {
        if project_refresh.is_stale(epoch) {
            return;
        }

        let db = snapshot.db();
        let Some(project) = db.project() else {
            return;
        };

        match self {
            Self::BuildTagSpecs => {
                let _ = db.tag_specs();
            }
            Self::BuildFilterAritySpecs => {
                let _ = db.filter_arity_specs();
            }
            Self::BuildModelGraph => {
                let _ = db.model_graph();
            }
            Self::ResolveTemplateDirs => {
                let _ = db.template_dirs();
            }
            Self::IndexTemplateLibraries => {
                let _ = db.template_libraries();
            }
            Self::IndexTemplates => {
                let _ = project_template_files(db, project);
            }
        }
    }

    async fn join(self, handle: WarmJobHandle) -> WarmJobResult {
        handle.await.unwrap_or_else(|error| {
            panic!("project warm-up {self:?} task must not panic: {error}");
        })
    }
}

async fn warm_project_queries(
    snapshot: SessionSnapshot,
    project_refresh: ProjectRefreshState,
    epoch: u64,
    progress: &ProgressItem,
) {
    if project_refresh.is_stale(epoch) {
        return;
    }

    let mut handles = Vec::new();
    for stage in WarmStage::ALL {
        handles.push((
            stage,
            stage.spawn(snapshot.clone(), project_refresh.clone(), epoch),
        ));
        progress.report(stage.message()).await;
    }

    for (stage, handle) in handles {
        if let Err(cancelled) = stage.join(handle).await {
            tracing::debug!(
                ?cancelled,
                ?stage,
                "Project refresh warm-up cancelled; newer inputs will re-warm queries"
            );
        }
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
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn project_settings_load_error_skips_refresh_inputs() {
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
        let session = Session::new(&params);
        let project_refresh = session.project_refresh().clone();
        let epoch = project_refresh.begin_refresh();
        let session = Arc::new(Mutex::new(session));

        let outcome = load_project_settings(&session, &project_refresh, epoch).await;

        assert!(matches!(outcome, LoadOutcome::Skipped));
    }
}
