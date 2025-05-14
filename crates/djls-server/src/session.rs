use djls_conf::Settings;
use djls_project::DjangoProject;
use tower_lsp_server::lsp_types::ClientCapabilities;

use crate::db::ServerDatabaseHandle;
use crate::documents::Store;

#[derive(Default)]
pub struct Session {
    client_capabilities: Option<ClientCapabilities>,
    project: Option<DjangoProject>,
    documents: Store,
    settings: Settings,
    db_handle: ServerDatabaseHandle,
}

impl Session {
    pub fn new(client_capabilities: ClientCapabilities) -> Self {
        Self {
            client_capabilities: Some(client_capabilities),
            project: None,
            documents: Store::new(),
            settings: Settings::default(),
            db_handle: ServerDatabaseHandle::new(),
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

    pub fn db_handle(&self) -> &ServerDatabaseHandle {
        &self.db_handle
    }
}
