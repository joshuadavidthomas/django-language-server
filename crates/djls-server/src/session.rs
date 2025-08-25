use std::path::PathBuf;

use djls_conf::Settings;
use djls_project::DjangoProject;
use djls_workspace::DocumentStore;
use percent_encoding::percent_decode_str;
use tower_lsp_server::lsp_types::ClientCapabilities;
use tower_lsp_server::lsp_types::InitializeParams;
use tower_lsp_server::lsp_types::Uri;

#[derive(Default)]
pub struct Session {
    project: Option<DjangoProject>,
    documents: DocumentStore,
    settings: Settings,

    #[allow(dead_code)]
    client_capabilities: ClientCapabilities,
}

impl Session {
    /// Determines the project root path from initialization parameters.
    ///
    /// Tries the current directory first, then falls back to the first workspace folder.
    fn get_project_path(params: &InitializeParams) -> Option<PathBuf> {
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
    fn uri_to_pathbuf(uri: &Uri) -> Option<PathBuf> {
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

    pub fn new(params: &InitializeParams) -> Self {
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
            documents: DocumentStore::new(),
            settings,
        }
    }

    pub fn project(&self) -> Option<&DjangoProject> {
        self.project.as_ref()
    }

    pub fn project_mut(&mut self) -> &mut Option<DjangoProject> {
        &mut self.project
    }

    pub fn documents(&self) -> &DocumentStore {
        &self.documents
    }

    pub fn documents_mut(&mut self) -> &mut DocumentStore {
        &mut self.documents
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }
}
