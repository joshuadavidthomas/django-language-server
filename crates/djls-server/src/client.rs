use djls_conf::Settings;
use djls_source::PositionEncoding;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::Value;
use tower_lsp_server::ls_types;

use crate::ext::ClientInfoExt;
use crate::ext::PositionEncodingKindExt;

#[derive(Debug, Clone)]
pub(crate) struct ClientInfo {
    client: Client,
    position_encoding: PositionEncoding,
    capabilities: ClientCapabilities,
    options: ClientOptions,
}

impl ClientInfo {
    #[must_use]
    pub(crate) fn new(
        capabilities: &ls_types::ClientCapabilities,
        client_info: Option<&ls_types::ClientInfo>,
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
    pub(crate) fn client(&self) -> Client {
        self.client
    }

    #[must_use]
    pub(crate) fn config_overrides(&self) -> &Settings {
        &self.options.settings
    }

    #[must_use]
    pub(crate) fn position_encoding(&self) -> PositionEncoding {
        self.position_encoding
    }

    #[must_use]
    pub(crate) fn supports_pull_diagnostics(&self) -> bool {
        self.capabilities.pull_diagnostics
    }

    #[must_use]
    pub(crate) fn supports_snippets(&self) -> bool {
        self.capabilities.snippets
    }

    #[must_use]
    pub(crate) fn supports_work_done_progress(&self) -> bool {
        self.capabilities.work_done_progress
    }
}

/// LSP client identification for client-specific behavioral overrides.
///
/// Most clients work fine with standard LSP behavior, but some require
/// specific workarounds (e.g., language ID mappings, capability quirks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Client {
    /// Standard LSP client behavior (no special overrides needed)
    Default,
    /// Sublime Text LSP - uses "html" language ID for Django templates
    SublimeText,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClientCapabilities {
    pull_diagnostics: bool,
    snippets: bool,
    work_done_progress: bool,
}

impl ClientCapabilities {
    #[must_use]
    pub(crate) fn new(capabilities: &ls_types::ClientCapabilities) -> Self {
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

        let work_done_progress = capabilities
            .window
            .as_ref()
            .and_then(|window| window.work_done_progress)
            .unwrap_or(false);

        Self {
            pull_diagnostics,
            snippets,
            work_done_progress,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct ClientOptions {
    #[serde(flatten)]
    pub settings: Settings,

    #[serde(flatten)]
    pub unknown: FxHashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use djls_source::FileKind;

    use super::*;
    use crate::ext::TextDocumentItemExt;

    #[test]
    fn test_negotiate_prefers_utf8_when_available() {
        let capabilities = ls_types::ClientCapabilities {
            general: Some(ls_types::GeneralClientCapabilities {
                position_encodings: Some(vec![
                    ls_types::PositionEncodingKind::new("utf-16"),
                    ls_types::PositionEncodingKind::new("utf-8"),
                    ls_types::PositionEncodingKind::new("utf-32"),
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
        let capabilities = ls_types::ClientCapabilities {
            general: Some(ls_types::GeneralClientCapabilities {
                position_encodings: Some(vec![
                    ls_types::PositionEncodingKind::new("utf-16"),
                    ls_types::PositionEncodingKind::new("utf-32"),
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
        let capabilities = ls_types::ClientCapabilities {
            general: Some(ls_types::GeneralClientCapabilities {
                position_encodings: Some(vec![
                    ls_types::PositionEncodingKind::new("ascii"),
                    ls_types::PositionEncodingKind::new("utf-7"),
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
        let capabilities = ls_types::ClientCapabilities::default();
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        assert_eq!(client_info.position_encoding(), PositionEncoding::Utf16);
    }

    #[test]
    fn test_negotiate_detects_sublime_client() {
        let capabilities = ls_types::ClientCapabilities::default();
        let lsp_client_info = ls_types::ClientInfo {
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
        let capabilities = ls_types::ClientCapabilities::default();
        let lsp_client_info = ls_types::ClientInfo {
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
    fn work_done_progress_capability_is_enabled_when_explicitly_true() {
        let capabilities = ls_types::ClientCapabilities {
            window: Some(ls_types::WindowClientCapabilities {
                work_done_progress: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());

        assert!(client_info.supports_work_done_progress());
    }

    #[test]
    fn work_done_progress_capability_is_disabled_when_explicitly_false() {
        let capabilities = ls_types::ClientCapabilities {
            window: Some(ls_types::WindowClientCapabilities {
                work_done_progress: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());

        assert!(!client_info.supports_work_done_progress());
    }

    #[test]
    fn work_done_progress_capability_is_disabled_when_missing() {
        let capabilities = ls_types::ClientCapabilities::default();
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());

        assert!(!client_info.supports_work_done_progress());
    }

    #[test]
    fn test_map_language_id_sublime_html_to_template() {
        let capabilities = ls_types::ClientCapabilities::default();
        let lsp_client_info = ls_types::ClientInfo {
            name: "Sublime Text LSP".to_string(),
            version: None,
        };
        let client_info = ClientInfo::new(
            &capabilities,
            Some(&lsp_client_info),
            ClientOptions::default(),
        );
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.html").unwrap(),
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
        let capabilities = ls_types::ClientCapabilities::default();
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.html").unwrap(),
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
        let capabilities = ls_types::ClientCapabilities::default();
        let client_info = ClientInfo::new(&capabilities, None, ClientOptions::default());
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "django-html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(client_info.client()),
            FileKind::Template
        );
    }
}
