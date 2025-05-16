use djls_conf::Settings;
use djls_project::DjangoProject;
use salsa::StorageHandle;
use tower_lsp_server::lsp_types::ClientCapabilities;

use crate::db::ServerDatabase;
use crate::documents::Store;

#[derive(Default)]
pub struct Session {
    client_capabilities: Option<ClientCapabilities>,
    project: Option<DjangoProject>,
    documents: Store,
    settings: Settings,

    /// A thread-safe Salsa database handle that can be shared between threads.
    ///
    /// This handle allows us to create database instances as needed.
    /// Even though we're using a single-threaded runtime, we still need
    /// this to be thread-safe because of LSP trait requirements.
    ///
    /// Usage:
    /// ```rust,ignore
    /// // Get a database instance directly
    /// let db = session.db();
    ///
    /// // Use the database
    /// db.some_query(args)
    /// ```
    // Note: We tried using a direct ServerDatabase but it doesn't implement Sync
    // due to internal RefCell usage, which is required for LSP
    db_handle: StorageHandle<ServerDatabase>,
}

impl Session {
    pub fn new(client_capabilities: ClientCapabilities) -> Self {
        Self {
            client_capabilities: Some(client_capabilities),
            project: None,
            documents: Store::new(),
            settings: Settings::default(),
            db_handle: StorageHandle::new(None),
        }
    }

    pub fn client_capabilities(&self) -> Option<&ClientCapabilities> {
        self.client_capabilities.as_ref()
    }

    pub fn client_capabilities_mut(&mut self) -> &mut Option<ClientCapabilities> {
        &mut self.client_capabilities
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

    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// Get the raw database handle from the session
    ///
    /// Note: In most cases, you'll want to use `db()` instead to get a usable
    /// database instance directly.
    pub fn db_handle(&self) -> &StorageHandle<ServerDatabase> {
        &self.db_handle
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
