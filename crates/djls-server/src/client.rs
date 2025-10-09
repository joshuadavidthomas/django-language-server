//! LSP client identification for client-specific behavioral overrides.

use djls_source::PositionEncoding;
use tower_lsp_server::lsp_types;

use crate::ext::ClientInfoExt;
use crate::ext::PositionEncodingKindExt;

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
    client: Client,
    position_encoding: PositionEncoding,
    pull_diagnostics: bool,
    snippets: bool,
}

impl ClientCapabilities {
    pub fn negotiate(
        capabilities: &lsp_types::ClientCapabilities,
        client_info: Option<&lsp_types::ClientInfo>,
    ) -> Self {
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

        let client = client_info.to_client();

        Self {
            client,
            position_encoding,
            pull_diagnostics,
            snippets,
        }
    }

    #[must_use]
    pub fn position_encoding(self) -> PositionEncoding {
        self.position_encoding
    }

    #[must_use]
    pub fn client(self) -> Client {
        self.client
    }

    #[must_use]
    pub fn supports_pull_diagnostics(self) -> bool {
        self.pull_diagnostics
    }

    #[must_use]
    pub fn supports_snippets(self) -> bool {
        self.snippets
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
        assert_eq!(
            ClientCapabilities::negotiate(&capabilities, None).position_encoding(),
            PositionEncoding::Utf8
        );
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
        assert_eq!(
            ClientCapabilities::negotiate(&capabilities, None).position_encoding(),
            PositionEncoding::Utf32
        );
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
        assert_eq!(
            ClientCapabilities::negotiate(&capabilities, None).position_encoding(),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_negotiate_fallback_with_no_capabilities() {
        let capabilities = lsp_types::ClientCapabilities::default();
        assert_eq!(
            ClientCapabilities::negotiate(&capabilities, None).position_encoding(),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_negotiate_detects_sublime_client() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_info = lsp_types::ClientInfo {
            name: "Sublime Text LSP".to_string(),
            version: Some("1.0.0".to_string()),
        };
        assert_eq!(
            ClientCapabilities::negotiate(&capabilities, Some(&client_info)).client(),
            Client::SublimeText
        );
    }

    #[test]
    fn test_negotiate_defaults_to_default_client() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_info = lsp_types::ClientInfo {
            name: "Other Client".to_string(),
            version: None,
        };
        assert_eq!(
            ClientCapabilities::negotiate(&capabilities, Some(&client_info)).client(),
            Client::Default
        );
    }

    #[test]
    fn test_map_language_id_sublime_html_to_template() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_info = lsp_types::ClientInfo {
            name: "Sublime Text LSP".to_string(),
            version: None,
        };
        let client_caps = ClientCapabilities::negotiate(&capabilities, Some(&client_info));
        let doc = lsp_types::TextDocumentItem {
            uri: lsp_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(client_caps.client()),
            FileKind::Template
        );
    }

    #[test]
    fn test_map_language_id_default_html_to_other() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_caps = ClientCapabilities::negotiate(&capabilities, None);
        let doc = lsp_types::TextDocumentItem {
            uri: lsp_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(client_caps.client()),
            FileKind::Other
        );
    }

    #[test]
    fn test_map_language_id_django_html_always_template() {
        let capabilities = lsp_types::ClientCapabilities::default();
        let client_caps = ClientCapabilities::negotiate(&capabilities, None);
        let doc = lsp_types::TextDocumentItem {
            uri: lsp_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "django-html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(client_caps.client()),
            FileKind::Template
        );
    }
}
