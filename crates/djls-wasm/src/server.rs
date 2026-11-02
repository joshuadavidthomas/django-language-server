//! A synchronous LSP server for browser hosts: one JSON-RPC message in, every outgoing
//! message it causes out. It follows `djls-server`'s request handling, minus threads.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::json;
use wasm_bindgen::prelude::*;

use crate::session::BrowserSession;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const SERVER_NOT_INITIALIZED: i64 = -32002;

/// An LSP server over an in-memory project. The host loads workspace files with the
/// `djls/syncFiles` and `djls/removeFiles` notifications; open documents override them.
#[wasm_bindgen]
#[derive(Default)]
pub struct LanguageServer {
    session: Option<BrowserSession>,
    shut_down: bool,
}

#[wasm_bindgen]
impl LanguageServer {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> LanguageServer {
        Self::default()
    }

    /// Handle one JSON-RPC message and return a JSON array of the messages to send back:
    /// the response when `message` is a request, then any notifications.
    #[must_use]
    pub fn handle(&mut self, message: &str) -> String {
        let mut outbox = Vec::new();
        match serde_json::from_str::<Incoming>(message) {
            Ok(Incoming {
                id: Some(id),
                method: Some(method),
                params,
            }) => {
                let response = match self.request(&method, params, &mut outbox) {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                };
                outbox.insert(0, response);
            }
            Ok(Incoming {
                id: None,
                method: Some(method),
                params,
            }) => self.notification(&method, params, &mut outbox),
            // Responses to server requests; this server sends none that need answers.
            Ok(Incoming { method: None, .. }) => {}
            Err(error) => outbox.push(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": ResponseError::new(PARSE_ERROR, error.to_string()),
            })),
        }
        Value::Array(outbox).to_string()
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        outbox: &mut Vec<Value>,
    ) -> Result<Value, ResponseError> {
        if self.shut_down {
            return Err(ResponseError::new(
                INVALID_REQUEST,
                "server is shut down".to_string(),
            ));
        }
        if method == "initialize" {
            if self.session.is_some() {
                return Err(ResponseError::new(
                    INVALID_REQUEST,
                    "server is already initialized".to_string(),
                ));
            }
            let (session, result) = BrowserSession::initialize(&parse(params)?);
            self.session = Some(session);
            return to_value(&result);
        }
        let Some(session) = &mut self.session else {
            return Err(ResponseError::new(
                SERVER_NOT_INITIALIZED,
                "server is not initialized".to_string(),
            ));
        };
        match method {
            "shutdown" => {
                self.shut_down = true;
                Ok(Value::Null)
            }
            "textDocument/completion" => to_value(&session.completion(&parse(params)?, outbox)),
            "textDocument/hover" => to_value(&session.hover(&parse(params)?, outbox)),
            "textDocument/definition" => to_value(&session.definition(&parse(params)?, outbox)),
            "textDocument/foldingRange" => {
                to_value(&session.folding_range(&parse(params)?, outbox))
            }
            "textDocument/formatting" => to_value(&session.formatting(&parse(params)?)),
            _ => Err(ResponseError::new(
                METHOD_NOT_FOUND,
                format!("unsupported request: {method}"),
            )),
        }
    }

    fn notification(&mut self, method: &str, params: Value, outbox: &mut Vec<Value>) {
        let Some(session) = self.session.as_mut().filter(|_| !self.shut_down) else {
            return;
        };
        // Like the server, malformed notifications are dropped; there is no reply to carry an
        // error. `Some(edited)` means diagnostics may have changed, `edited` naming a document
        // whose new version must be published.
        let changed = match method {
            "textDocument/didOpen" => {
                parse(params).map(|params| session.did_open(&params).map(Some))
            }
            "textDocument/didChange" => {
                parse(params).map(|params| session.did_change(params).map(Some))
            }
            "textDocument/didClose" => {
                parse(params).map(|params| session.did_close(&params).map(|()| None))
            }
            "workspace/didChangeConfiguration" => parse(params).map(|params| {
                session.did_change_configuration(params);
                Some(None)
            }),
            "djls/syncFiles" => parse(params).map(|params| {
                session.sync_files(params);
                Some(None)
            }),
            "djls/removeFiles" => parse(params).map(|params| {
                session.remove_files(&params);
                Some(None)
            }),
            // `initialized` needs nothing: Project work runs before the first result that needs it.
            _ => return,
        };
        if let Ok(Some(edited)) = changed {
            session.publish_diagnostics(edited.as_deref(), outbox);
        }
    }
}

#[derive(Deserialize)]
struct Incoming {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
}

#[derive(serde::Serialize)]
struct ResponseError {
    code: i64,
    message: String,
}

impl ResponseError {
    fn new(code: i64, message: String) -> Self {
        Self { code, message }
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, ResponseError> {
    serde_json::from_value(params)
        .map_err(|error| ResponseError::new(INVALID_PARAMS, error.to_string()))
}

fn to_value(value: &impl serde::Serialize) -> Result<Value, ResponseError> {
    serde_json::to_value(value)
        .map_err(|error| ResponseError::new(INTERNAL_ERROR, error.to_string()))
}

pub(crate) fn notification(method: &str, params: &impl serde::Serialize) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}
