use djls_conf::Settings;
use djls_project::DjangoProject;
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
    /// This creates a usable database with Salsa event logging
    pub fn db(&self) -> ServerDatabase {
        let storage = salsa::Storage::new(if tracing::enabled!(tracing::Level::DEBUG) {
            Some(Box::new({
                move |event: salsa::Event| {
                    if matches!(event.kind, salsa::EventKind::WillCheckCancellation) {
                        return;
                    }
                    tracing::debug!("Salsa event: {event:?}");
                }
            }))
        } else {
            None
        });
        ServerDatabase::new(storage)
    }
}
