use salsa::Database;

/// A thread-safe handle to a Salsa database that can be shared between threads.
///
/// This implements the insight from [this Salsa Zulip discussion](https://salsa.zulipchat.com/#narrow/channel/145099-Using-Salsa/topic/.E2.9C.94.20Advice.20on.20using.20salsa.20from.20Sync.20.2B.20Send.20context/with/495497515)
/// where we're using the StorageHandle to create a thread-safe handle that can be
/// shared between threads. When we need to use it, we clone the handle to get a new reference.
///
/// Usage:
/// ```rust,ignore
/// // Create a new handle
/// let db_handle = ServerDatabaseHandle::new();
///
/// // Clone it to pass to different threads
/// let db_handle_clone = db_handle.clone();
///
/// // Use it in an async context
/// async_fn(move || {
///     // Get a database from the handle
///     let db = db_handle_clone.db();
///
///     // Use the database
///     db.some_query(args)
/// });
/// ```
#[derive(Clone, Default)]
pub struct ServerDatabaseHandle {
    handle: salsa::StorageHandle<ServerDatabase>,
}

impl ServerDatabaseHandle {
    /// Create a new thread-safe database handle
    ///
    /// This creates a handle that can be safely shared between threads and cloned
    /// to create new references to the same underlying database storage.
    pub fn new() -> Self {
        Self {
            handle: salsa::StorageHandle::new(None),
        }
    }

    /// Get a database instance from this handle
    ///
    /// This creates a usable database from the handle, which can be used
    /// to query and update data in the database. The database can be cloned
    /// cheaply as it's just a reference to the underlying storage.
    pub fn db(&self) -> ServerDatabase {
        let storage = self.handle.clone().into_storage();
        ServerDatabase { storage }
    }
}

impl std::fmt::Debug for ServerDatabaseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerDatabaseHandle").finish()
    }
}

#[salsa::db]
#[derive(Clone, Default)]
pub struct ServerDatabase {
    storage: salsa::Storage<Self>,
}

impl std::fmt::Debug for ServerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerDatabase").finish()
    }
}

impl Database for ServerDatabase {}
