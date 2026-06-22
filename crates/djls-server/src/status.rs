use serde::Deserialize;
use serde::Serialize;
use tower_lsp_server::ls_types::notification::Notification;

pub(crate) struct ServerStatusNotification;

impl Notification for ServerStatusNotification {
    type Params = ServerStatusParams;
    const METHOD: &'static str = "djls/serverStatus";
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ServerStatusParams {
    pub(crate) health: ServerStatusHealth,
    pub(crate) quiescent: bool,
    pub(crate) message: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerStatusHealth {
    Ok,
    Warning,
    Error,
}
