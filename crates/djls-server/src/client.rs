use std::collections::HashMap;

use djls_conf::Settings;
use djls_source::PositionEncoding;
use serde::Deserialize;
use serde_json::Value;
use tower_lsp_server::lsp_types;

use crate::ext::ClientInfoExt;
use crate::ext::PositionEncodingKindExt;

#[derive(Debug, Clone)]
pub struct ClientInfo {
    client: Client,
    position_encoding: PositionEncoding,
    capabilities: ClientCapabilities,
    options: ClientOptions,
}

impl ClientInfo {
    #[must_use]
    pub fn new(
        capabilities: &lsp_types::ClientCapabilities,
        client_info: Option<&lsp_types::ClientInfo>,
        options: ClientOptions,
    ) -> Self {
        let client = client_info.to_client();

        let client_encodings = capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
            .map_or(&[][..], |kinds| kinds.as_slice());

        let position_encoding = [
            PositionEncoding::Utf8,
            PositionEncoding::Utf32,
            PositionEncoding::Utf16,
        ]
        .into_iter()
        .find(|&preferred| {
            client_encodings
                .iter()
                .any(|kind| kind.to_position_encoding() == Some(preferred))
        })
        .unwrap_or(PositionEncoding::Utf16);

        let capabilities = ClientCapabilities::new(capabilities);

        Self {
            client,
            position_encoding,
            capabilities,
            options,
        }
    }

    #[must_use]
    pub fn client(&self) -> Client {
        self.client
    }

    #[must_use]
    pub fn capabilities(&self) -> ClientCapabilities {
        self.capabilities
    }

    #[must_use]
    pub fn options(&self) -> &ClientOptions {
        &self.options
    }

    #[must_use]
    pub fn config_overrides(&self) -> &Settings {
        &self.options.settings
    }

    #[must_use]
    pub fn position_encoding(&self) -> PositionEncoding {
        self.position_encoding
    }

    #[must_use]
    pub fn supports_pull_diagnostics(&self) -> bool {
        self.capabilities.pull_diagnostics
    }

    #[must_use]
    pub fn supports_snippets(&self) -> bool {
        self.capabilities.snippets
    }
}

/// LSP client identification for client-specific behavioral overrides.
///
/// Most clients work fine with standard LSP behavior, but some require
/// specific workarounds (e.g., language ID mappings, capability quirks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Client {
    /// Standard LSP client behavior (no special overrides needed)
    Default,
    /// Sublime Text LSP - uses "html" language ID for Django templates
    SublimeText,
}

#[derive(Debug, Clone, Copy)]
pub struct ClientCapabilities {
    pull_diagnostics: bool,
    snippets: bool,
}

impl ClientCapabilities {
    #[must_use]
    pub fn new(capabilities: &lsp_types::ClientCapabilities) -> Self {
        let pull_diagnostics = capabilities
            .text_document
            .as_ref()
            .and_then(|text_doc| text_doc.diagnostic.as_ref())
            .is_some();

        let snippets = capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|completion_item| completion_item.snippet_support)
            .unwrap_or(false);

        Self {
            pull_diagnostics,
            snippets,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ClientOptions {
    #[serde(flatten)]
    pub settings: Settings,

    #[serde(flatten)]
    pub unknown: HashMap<String, Value>,
}

impl ClientOptions {
    #[must_use]
    pub fn from_value(value: Option<Value>) -> (Self, Option<serde_json::Error>) {
        let Some(value) = value else {
            return (Self::default(), None);
        };

        match serde_json::from_value::<Self>(value) {
            Ok(client_opts) => (client_opts, None),
            Err(err) => (Self::default(), Some(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use djls_source::FileKind;

    use super::*;
    use crate::ext::TextDocumentItemExt;

    #[test]
    fn test_negotiate_prefers_utf8_when_available() {
        let capabilities = lsp_types::ClientCapabilities {
            general: Some(lsp_types::GeneralClientCapabilities {
                position_encodings: Some(vec![
                    lsp_types::PositionEncodingKind::new("utf-16"),
                    lsp_types::PositionEncodingKind::new("utf-8"),
                    lsp_types::PositionEncodingKind::new("utf-32"),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        assert_eq!(client_info.position_encoding(), PositionEncoding::Utf8);
    }

    #[test]
    fn test_negotiate_prefers_utf32_over_utf16() {
        let capabilities = lsp_types::ClientCapabilities {
            general: Some(lsp_types::GeneralClientCapabilities {
                position_encodings: Some(vec![
                    lsp_types::PositionEncodingKind::new("utf-16"),
                    lsp_types::PositionEncodingKind::new("utf-32"),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        assert_eq!(client_info.position_encoding(), PositionEncoding::Utf32);
    }

    #[test]
    fn test_negotiate_fallback_with_unsupported_encodings() {
        let capabilities = lsp_types::ClientCapabilities {
            general: Some(lsp_types::GeneralClientCapabilities {
                position_encodings: Some(vec![
                    lsp_types::PositionEncodingKind::new("ascii"),
                    lsp_types::PositionEncodingKind::new("utf-7"),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        assert_eq!(client_info.position_encoding(), PositionEncoding::Utf16);
    }

    #[test]
    fn test_negotiate_fallback_with_no_capabilities() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        assert_eq!(client_info.position_encoding(), PositionEncoding::Utf16);
    }

    #[test]
    fn test_negotiate_detects_sublime_client() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let lsp_client_info = lsp_types::ClientInfo {
            name: "Sublime Text LSP".to_string(),
            version: Some("1.0.0".to_string()),
        };
        let client_info = ClientInfo::new(
            &capabilities,
            Some(&lsp_client_info),
            ClientOptions::default(),
        );
        assert_eq!(client_info.client(), Client::SublimeText);
    }

    #[test]
    fn test_negotiate_defaults_to_default_client() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let lsp_client_info = lsp_types::ClientInfo {
            name: "Other Client".to_string(),
            version: None,
        };
        let client_info = ClientInfo::new(
            &capabilities,
            Some(&lsp_client_info),
            ClientOptions::default(),
        );
        assert_eq!(client_info.client(), Client::Default);
    }

    #[test]
    fn test_map_language_id_sublime_html_to_template() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let lsp_client_info = lsp_types::ClientInfo {
            name: "Sublime Text LSP".to_string(),
            version: None,
        };
        let client_info = ClientInfo::new(
            &capabilities,
            Some(&lsp_client_info),
            ClientOptions::default(),
        );
        let doc = lsp_types::TextDocumentItem {
            uri: lsp_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(client_info.client()),
            FileKind::Template
        );
    }

    #[test]
    fn test_map_language_id_default_html_to_other() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        let doc = lsp_types::TextDocumentItem {
            uri: lsp_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(client_info.client()),
            FileKind::Other
        );
    }

    #[test]
    fn test_map_language_id_django_html_always_template() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        let doc = lsp_types::TextDocumentItem {
            uri: lsp_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "django-html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(client_info.client()),
            FileKind::Template
        );
    }

    #[test]
    fn test_client_options_from_value_none() {
        let (opts, err) = super::ClientOptions::from_value(None);
        assert!(err.is_none());
        assert!(!opts.settings.debug());
        assert!(opts.unknown.is_empty());
    }

    #[test]
    fn test_client_options_from_value_empty_object() {
        let value = serde_json::json!({});
        let (opts, err) = super::ClientOptions::from_value(Some(value));
        assert!(err.is_none());
        assert!(!opts.settings.debug());
        assert!(opts.unknown.is_empty());
    }

    #[test]
    fn test_client_options_from_value_valid_settings() {
        let value = serde_json::json!({
            "debug": true,
            "venv_path": "/path/to/venv",
            "django_settings_module": "myproject.settings"
        });
        let (opts, err) = super::ClientOptions::from_value(Some(value));
        assert!(err.is_none());
        assert!(opts.settings.debug());
        assert_eq!(opts.settings.venv_path(), Some("/path/to/venv"));
        assert_eq!(
            opts.settings.django_settings_module(),
            Some("myproject.settings")
        );
        assert!(opts.unknown.is_empty());
    }

    #[test]
    fn test_client_options_from_value_unknown_fields() {
        let value = serde_json::json!({
            "debug": true,
            "unknownOption": "value",
            "anotherUnknown": 42
        });
        let (opts, err) = super::ClientOptions::from_value(Some(value));
        assert!(err.is_none());
        assert!(opts.settings.debug());
        assert_eq!(opts.unknown.len(), 2);
        assert!(opts.unknown.contains_key("unknownOption"));
        assert!(opts.unknown.contains_key("anotherUnknown"));
    }

    #[test]
    fn test_client_options_from_value_invalid_json() {
        let value = serde_json::json!({
            "debug": "not_a_boolean"
        });
        let (opts, err) = super::ClientOptions::from_value(Some(value));
        assert!(err.is_some());
        assert!(!opts.settings.debug());
        assert!(opts.unknown.is_empty());
    }

    #[test]
    fn test_client_options_from_value_partial_settings() {
        let value = serde_json::json!({
            "venv_path": "/custom/venv"
        });
        let (opts, err) = super::ClientOptions::from_value(Some(value));
        assert!(err.is_none());
        assert!(!opts.settings.debug());
        assert_eq!(opts.settings.venv_path(), Some("/custom/venv"));
        assert_eq!(opts.settings.django_settings_module(), None);
    }
}
