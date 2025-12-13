use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileKind;
use djls_source::LineCol;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Range;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::DocumentChange;
use tower_lsp_server::ls_types;

use crate::client::Client;
use crate::client::ClientOptions;

pub(crate) trait PositionExt {
    fn to_line_col(&self) -> LineCol;
    fn to_offset(&self, text: &str, line_index: &LineIndex, encoding: PositionEncoding) -> Offset;
}

impl PositionExt for ls_types::Position {
    fn to_line_col(&self) -> LineCol {
        LineCol::new(self.line, self.character)
    }

    fn to_offset(&self, text: &str, line_index: &LineIndex, encoding: PositionEncoding) -> Offset {
        let line_col = self.to_line_col();
        line_index.offset(text, line_col, encoding)
    }
}

pub(crate) trait PositionEncodingExt {
    fn to_lsp(&self) -> ls_types::PositionEncodingKind;
}

impl PositionEncodingExt for PositionEncoding {
    fn to_lsp(&self) -> ls_types::PositionEncodingKind {
        match self {
            PositionEncoding::Utf8 => ls_types::PositionEncodingKind::new("utf-8"),
            PositionEncoding::Utf16 => ls_types::PositionEncodingKind::new("utf-16"),
            PositionEncoding::Utf32 => ls_types::PositionEncodingKind::new("utf-32"),
        }
    }
}

pub(crate) trait PositionEncodingKindExt {
    fn to_position_encoding(&self) -> Option<PositionEncoding>;
}

impl PositionEncodingKindExt for ls_types::PositionEncodingKind {
    fn to_position_encoding(&self) -> Option<PositionEncoding> {
        match self.as_str() {
            "utf-8" => Some(PositionEncoding::Utf8),
            "utf-16" => Some(PositionEncoding::Utf16),
            "utf-32" => Some(PositionEncoding::Utf32),
            _ => None,
        }
    }
}

pub(crate) trait RangeExt {
    fn to_source_range(&self) -> Range;
}

impl RangeExt for ls_types::Range {
    fn to_source_range(&self) -> Range {
        let start_line_col = self.start.to_line_col();
        let end_line_col = self.end.to_line_col();
        Range::new(start_line_col, end_line_col)
    }
}

pub(crate) trait TextDocumentContentChangeEventExt {
    fn to_document_changes(self) -> Vec<DocumentChange>;
}

impl TextDocumentContentChangeEventExt for Vec<ls_types::TextDocumentContentChangeEvent> {
    fn to_document_changes(self) -> Vec<DocumentChange> {
        self.into_iter()
            .map(|change| {
                DocumentChange::new(change.range.map(|r| r.to_source_range()), change.text)
            })
            .collect()
    }
}

pub(crate) trait TextDocumentIdentifierExt {
    fn to_file(&self, db: &mut dyn WorkspaceDb) -> Option<File>;
}

impl TextDocumentIdentifierExt for ls_types::TextDocumentIdentifier {
    fn to_file(&self, db: &mut dyn WorkspaceDb) -> Option<File> {
        let path = self.uri.to_utf8_path_buf()?;
        Some(db.get_or_create_file(&path))
    }
}

pub(crate) trait InitializeParamsExt {
    fn client_options(&self) -> ClientOptions;
}

impl InitializeParamsExt for ls_types::InitializeParams {
    fn client_options(&self) -> ClientOptions {
        let client_options: ClientOptions = self
            .initialization_options
            .as_ref()
            .and_then(|v| match serde_json::from_value(v.clone()) {
                Ok(opts) => Some(opts),
                Err(err) => {
                    tracing::error!(
                        "Failed to deserialize initialization options: {}. Using defaults.",
                        err
                    );
                    None
                }
            })
            .unwrap_or_default();

        if !client_options.unknown.is_empty() {
            tracing::warn!(
                "Received unknown initialization options: {}",
                client_options
                    .unknown
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        client_options
    }
}

pub(crate) trait ClientInfoExt {
    fn to_client(&self) -> Client;
}

impl ClientInfoExt for Option<&ls_types::ClientInfo> {
    fn to_client(&self) -> Client {
        match self.map(|info| info.name.as_str()) {
            Some("Sublime Text LSP") => Client::SublimeText,
            _ => Client::Default,
        }
    }
}

pub(crate) trait TextDocumentItemExt {
    fn language_id_to_file_kind(&self, client: Client) -> FileKind;
}

impl TextDocumentItemExt for ls_types::TextDocumentItem {
    fn language_id_to_file_kind(&self, client: Client) -> FileKind {
        match (client, self.language_id.as_str()) {
            (_, "python") => FileKind::Python,
            (_, "django-html" | "htmldjango") | (Client::SublimeText, "html") => FileKind::Template,
            _ => FileKind::Other,
        }
    }
}

pub(crate) trait UriExt {
    /// Convert `Utf8Path` to LSP Uri
    fn from_path(path: &Utf8Path) -> Option<Self>
    where
        Self: Sized;

    // TODO(virtual-paths): Step 2 - Add wrapper for DocumentPath → Uri conversion:
    // fn from_document_path(path: &DocumentPath) -> Option<Self> where Self: Sized;
    // This will call DocumentPath::to_uri() internally. The main API boundary is
    // DocumentPath::from_uri() / to_uri(), not here.

    /// Convert LSP URI directly to `Utf8PathBuf` (convenience)
    fn to_utf8_path_buf(&self) -> Option<Utf8PathBuf>;
}

impl UriExt for ls_types::Uri {
    fn from_path(path: &Utf8Path) -> Option<Self> {
        ls_types::Uri::from_file_path(path.as_std_path())
    }

    fn to_utf8_path_buf(&self) -> Option<Utf8PathBuf> {
        // TODO(virtual-paths): Step 2 - This entire method becomes a compatibility wrapper:
        //   DocumentPath::from_uri(self)?.as_file_path()
        // The real scheme branching logic will live in DocumentPath::from_uri(), not here.
        // For now (Step 1), only handle file:// URIs
        if self.scheme().as_str() != "file" {
            tracing::trace!(
                "URI conversion to path failed for: {} (non-file scheme)",
                self.as_str()
            );
            return None;
        }

        let path = self.to_file_path()?;

        Utf8PathBuf::from_path_buf(path.into_owned())
            .inspect_err(|_| {
                tracing::trace!(
                    "URI conversion to path failed for: {} (non-UTF-8 path)",
                    self.as_str()
                );
            })
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_position_encoding_kind_unknown_returns_none() {
        assert_eq!(
            ls_types::PositionEncodingKind::new("unknown").to_position_encoding(),
            None
        );
    }

    #[test]
    fn test_non_file_uri_returns_none() {
        // Step 1: Non-file URIs are rejected at the LSP boundary
        let uri = ls_types::Uri::from_str("untitled:Untitled-1").unwrap();
        assert!(uri.to_utf8_path_buf().is_none());

        // TODO(virtual-paths): In Step 2, this should return Some(DocumentPath::Virtual(...))
    }

    #[test]
    fn test_client_info_sublime_to_client() {
        let client_info = ls_types::ClientInfo {
            name: "Sublime Text LSP".to_string(),
            version: Some("1.0.0".to_string()),
        };
        assert_eq!(Some(&client_info).to_client(), Client::SublimeText);
    }

    #[test]
    fn test_client_info_other_to_client() {
        let client_info = ls_types::ClientInfo {
            name: "Other Client".to_string(),
            version: None,
        };
        assert_eq!(Some(&client_info).to_client(), Client::Default);
    }

    #[test]
    fn test_text_document_item_sublime_html_to_template() {
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(Client::SublimeText),
            FileKind::Template
        );
    }

    #[test]
    fn test_text_document_item_default_html_to_other() {
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(Client::Default),
            FileKind::Other
        );
    }

    #[test]
    fn test_text_document_item_django_html_to_template() {
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "django-html".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(Client::Default),
            FileKind::Template
        );
    }

    #[test]
    fn test_text_document_item_htmldjango_to_template() {
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.html").unwrap(),
            language_id: "htmldjango".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(Client::Default),
            FileKind::Template
        );
    }

    #[test]
    fn test_text_document_item_python_to_python() {
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.py").unwrap(),
            language_id: "python".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(Client::Default),
            FileKind::Python
        );
    }

    #[test]
    fn test_text_document_item_unknown_to_other() {
        let doc = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_str("file:///test.rs").unwrap(),
            language_id: "rust".to_string(),
            version: 1,
            text: String::new(),
        };
        assert_eq!(
            doc.language_id_to_file_kind(Client::Default),
            FileKind::Other
        );
    }
}
