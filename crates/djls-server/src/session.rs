use djls_conf::Settings;
use djls_project::DjangoProject;
use salsa::StorageHandle;
use tower_lsp_server::lsp_types::ClientCapabilities;
use tower_lsp_server::lsp_types::InitializeParams;

use crate::db::ServerDatabase;
use crate::documents::Store;

#[derive(Default)]
pub struct Session {
    project: Option<DjangoProject>,
    documents: Store,
    settings: Settings,

    #[allow(dead_code)]
    client_capabilities: ClientCapabilities,

    /// A thread-safe Salsa database handle that can be shared between threads.
    ///
    /// This implements the insight from [this Salsa Zulip discussion](https://salsa.zulipchat.com/#narrow/channel/145099-Using-Salsa/topic/.E2.9C.94.20Advice.20on.20using.20salsa.20from.20Sync.20.2B.20Send.20context/with/495497515)
    /// where we're using the `StorageHandle` to create a thread-safe handle that can be
    /// shared between threads. When we need to use it, we clone the handle to get a new reference.
    ///
    /// This handle allows us to create database instances as needed.
    /// Even though we're using a single-threaded runtime, we still need
    /// this to be thread-safe because of LSP trait requirements.
    ///
    /// Usage:
    /// ```rust,ignore
    /// // Use the StorageHandle in Session
    /// let db_handle = StorageHandle::new(None);
    ///
    /// // Clone it to pass to different threads
    /// let db_handle_clone = db_handle.clone();
    ///
    /// // Use it in an async context
    /// async_fn(move || {
    ///     // Get a database from the handle
    ///     let storage = db_handle_clone.into_storage();
    ///     let db = ServerDatabase::new(storage);
    ///
    ///     // Use the database
    ///     db.some_query(args)
    /// });
    /// ```
    db_handle: StorageHandle<ServerDatabase>,
}

impl Session {
    pub fn new(params: &InitializeParams) -> Self {
        let project_path = crate::workspace::get_project_path(params);

        let (project, settings) = if let Some(path) = &project_path {
            let settings =
                djls_conf::Settings::new(path).unwrap_or_else(|_| djls_conf::Settings::default());

            let project = Some(djls_project::DjangoProject::new(path.clone()));

            (project, settings)
        } else {
            (None, Settings::default())
        };

        Self {
            client_capabilities: params.capabilities.clone(),
            project,
            documents: Store::default(),
            settings,
            db_handle: StorageHandle::new(None),
        }
    }

    pub fn project(&self) -> Option<&DjangoProject> {
        self.project.as_ref()
    }

    pub fn project_mut(&mut self) -> &mut Option<DjangoProject> {
        &mut self.project
    }

    pub fn documents(&self) -> &Store {
        &self.documents
    }

    pub fn documents_mut(&mut self) -> &mut Store {
        &mut self.documents
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    /// Get a database instance directly from the session
    ///
    /// This creates a usable database from the handle, which can be used
    /// to query and update data in the database.
    pub fn db(&self) -> ServerDatabase {
        let storage = self.db_handle.clone().into_storage();
        ServerDatabase::new(storage)
    }
}
