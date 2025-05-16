use salsa::Database;

#[salsa::db]
#[derive(Clone, Default)]
pub struct ServerDatabase {
    // We need to use Salsa's thread-safe storage approach even in our single-threaded runtime
    // because the DjangoLanguageServer must implement Send+Sync for LSP compatibility
    storage: salsa::Storage<Self>,
}

impl ServerDatabase {
    /// Create a new database from storage
    pub fn new(storage: salsa::Storage<Self>) -> Self {
        Self { storage }
    }
}

impl std::fmt::Debug for ServerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerDatabase").finish_non_exhaustive()
    }
}

impl Database for ServerDatabase {}
