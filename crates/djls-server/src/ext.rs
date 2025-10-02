use std::str::FromStr;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::LineCol;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_workspace::paths;
use djls_workspace::Db as WorkspaceDb;
use tower_lsp_server::lsp_types;
use url::Url;

pub(crate) trait PositionExt {
    fn to_offset(&self, text: &str, index: &LineIndex, encoding: PositionEncoding) -> Offset;
}

impl PositionExt for lsp_types::Position {
    fn to_offset(&self, text: &str, index: &LineIndex, encoding: PositionEncoding) -> Offset {
        let line_col = LineCol::new(self.line, self.character);
        index.offset(line_col, text, encoding)
    }
}

pub(crate) trait PositionEncodingExt {
    fn to_lsp(&self) -> lsp_types::PositionEncodingKind;
}

impl PositionEncodingExt for PositionEncoding {
    fn to_lsp(&self) -> lsp_types::PositionEncodingKind {
        match self {
            PositionEncoding::Utf8 => lsp_types::PositionEncodingKind::new("utf-8"),
            PositionEncoding::Utf16 => lsp_types::PositionEncodingKind::new("utf-16"),
            PositionEncoding::Utf32 => lsp_types::PositionEncodingKind::new("utf-32"),
        }
    }
}

pub(crate) trait PositionEncodingKindExt {
    fn to_position_encoding(&self) -> Option<PositionEncoding>;
}

impl PositionEncodingKindExt for lsp_types::PositionEncodingKind {
    fn to_position_encoding(&self) -> Option<PositionEncoding> {
        match self.as_str() {
            "utf-8" => Some(PositionEncoding::Utf8),
            "utf-16" => Some(PositionEncoding::Utf16),
            "utf-32" => Some(PositionEncoding::Utf32),
            _ => None,
        }
    }
}

pub(crate) trait TextDocumentIdentifierExt {
    fn to_file(&self, db: &mut dyn WorkspaceDb) -> Option<File>;
}

impl TextDocumentIdentifierExt for lsp_types::TextDocumentIdentifier {
    fn to_file(&self, db: &mut dyn WorkspaceDb) -> Option<File> {
        let path = self.uri.to_utf8_path_buf()?;
        Some(db.get_or_create_file(&path))
    }
}

pub(crate) trait TextDocumentItemExt {
    /// Convert LSP `TextDocumentItem` to internal `TextDocument`
    fn into_text_document(
        self,
        db: &mut dyn djls_source::Db,
    ) -> Option<djls_workspace::TextDocument>;
}

impl TextDocumentItemExt for lsp_types::TextDocumentItem {
    fn into_text_document(
        self,
        db: &mut dyn djls_source::Db,
    ) -> Option<djls_workspace::TextDocument> {
        let path = self.uri.to_utf8_path_buf()?;
        Some(djls_workspace::TextDocument::new(
            self.text,
            self.version,
            djls_workspace::LanguageId::from(self.language_id.as_str()),
            &path,
            db,
        ))
    }
}

pub(crate) trait UriExt {
    /// Convert `uri::Url` to LSP Uri
    fn from_url(url: &Url) -> Option<Self>
    where
        Self: Sized;

    /// Convert `Utf8Path` to LSP Uri
    fn from_path(path: &Utf8Path) -> Option<Self>
    where
        Self: Sized;

    /// Convert LSP URI to `url::Url,` logging errors
    fn to_url(&self) -> Option<Url>;

    /// Convert LSP URI directly to `Utf8PathBuf` (convenience)
    fn to_utf8_path_buf(&self) -> Option<Utf8PathBuf>;
}

impl UriExt for lsp_types::Uri {
    fn from_url(url: &Url) -> Option<Self> {
        let uri_string = url.to_string();
        lsp_types::Uri::from_str(&uri_string)
            .inspect_err(|e| {
                tracing::error!("Failed to convert URL to LSP Uri: {} - Error: {}", url, e);
            })
            .ok()
    }

    fn from_path(path: &Utf8Path) -> Option<Self> {
        let url = paths::path_to_url(path)?;
        Self::from_url(&url)
    }

    fn to_url(&self) -> Option<Url> {
        Url::parse(self.as_str())
            .inspect_err(|e| {
                tracing::error!(
                    "Invalid URI from LSP client: {} - Error: {}",
                    self.as_str(),
                    e
                );
            })
            .ok()
    }

    fn to_utf8_path_buf(&self) -> Option<Utf8PathBuf> {
        let url = self.to_url()?;
        paths::url_to_path(&url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_encoding_kind_unknown_returns_none() {
        assert_eq!(
            lsp_types::PositionEncodingKind::new("unknown").to_position_encoding(),
            None
        );
    }
}
