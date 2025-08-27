use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use djls_conf::Settings;
use djls_project::DjangoProject;
use djls_workspace::{FileSystem, StdFileSystem, db::Database};
use percent_encoding::percent_decode_str;
use salsa::StorageHandle;
use tower_lsp_server::lsp_types;
use url::Url;

pub struct Session {
    /// The Django project configuration
    project: Option<DjangoProject>,

    /// LSP server settings
    settings: Settings,

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
    /// // Clone the StorageHandle for use in an async context
    /// let db_handle = session.db_handle.clone();
    ///
    /// // Use it in an async context
    /// async_fn(move || {
    ///     // Get a database from the handle
    ///     let storage = db_handle.into_storage();
    ///     let db = Database::from_storage(storage);
    ///
    ///     // Use the database
    ///     db.some_query(args)
    /// });
    /// ```
    db_handle: StorageHandle<Database>,

    /// File system abstraction for reading files
    file_system: Arc<dyn FileSystem>,

    /// Index of open documents with overlays (in-memory changes)
    /// Maps document URL to its current content
    overlays: HashMap<Url, String>,

    /// Tracks the session revision for change detection
    revision: u64,

    #[allow(dead_code)]
    client_capabilities: lsp_types::ClientCapabilities,
}

impl Session {
    /// Determines the project root path from initialization parameters.
    ///
    /// Tries the current directory first, then falls back to the first workspace folder.
    fn get_project_path(params: &lsp_types::InitializeParams) -> Option<PathBuf> {
        // Try current directory first
        std::env::current_dir().ok().or_else(|| {
            // Fall back to the first workspace folder URI
            params
                .workspace_folders
                .as_ref()
                .and_then(|folders| folders.first())
                .and_then(|folder| Self::uri_to_pathbuf(&folder.uri))
        })
    }

    /// Converts a `file:` URI into an absolute `PathBuf`.
    fn uri_to_pathbuf(uri: &lsp_types::Uri) -> Option<PathBuf> {
        // Check if the scheme is "file"
        if uri.scheme().is_none_or(|s| s.as_str() != "file") {
            return None;
        }

        // Get the path part as a string
        let encoded_path_str = uri.path().as_str();

        // Decode the percent-encoded path string
        let decoded_path_cow = percent_decode_str(encoded_path_str).decode_utf8_lossy();
        let path_str = decoded_path_cow.as_ref();

        #[cfg(windows)]
        let path_str = {
            // Remove leading '/' for paths like /C:/...
            path_str.strip_prefix('/').unwrap_or(path_str)
        };

        Some(PathBuf::from(path_str))
    }

    pub fn new(params: &lsp_types::InitializeParams) -> Self {
        let project_path = Self::get_project_path(params);

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
            settings,
            db_handle: StorageHandle::new(None),
            file_system: Arc::new(StdFileSystem),
            overlays: HashMap::new(),
            revision: 0,
        }
    }

    pub fn project(&self) -> Option<&DjangoProject> {
        self.project.as_ref()
    }

    pub fn project_mut(&mut self) -> &mut Option<DjangoProject> {
        &mut self.project
    }



    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    /// Get a database instance from the session.
    ///
    /// This creates a usable database from the handle, which can be used
    /// to query and update data. The database itself is not Send/Sync,
    /// but the StorageHandle is, allowing us to work with tower-lsp.
    pub fn db(&self) -> Database {
        let storage = self.db_handle.clone().into_storage();
        Database::from_storage(storage)
    }
}

impl Default for Session {
    fn default() -> Self {
        Self {
            project: None,
            settings: Settings::default(),
            db_handle: StorageHandle::new(None),
            file_system: Arc::new(StdFileSystem),
            overlays: HashMap::new(),
            revision: 0,
            client_capabilities: lsp_types::ClientCapabilities::default(),
        }
    }
}
