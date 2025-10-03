use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::LineCol;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_workspace::Db as WorkspaceDb;
use tower_lsp_server::lsp_types;
use tower_lsp_server::UriExt as TowerUriExt;

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

impl UriExt for lsp_types::Uri {
    fn from_path(path: &Utf8Path) -> Option<Self> {
        <lsp_types::Uri as TowerUriExt>::from_file_path(path.as_std_path())
    }

    fn to_utf8_path_buf(&self) -> Option<Utf8PathBuf> {
        // TODO(virtual-paths): Step 2 - This entire method becomes a compatibility wrapper:
        //   DocumentPath::from_uri(self)?.as_file_path()
        // The real scheme branching logic will live in DocumentPath::from_uri(), not here.
        // For now (Step 1), only handle file:// URIs
        // we don't have fluent_uri as a dep, just transitive, so allow this
        #[allow(clippy::redundant_closure_for_method_calls)]
        if self.scheme().map(|s| s.as_str()) != Some("file") {
            tracing::trace!(
                "URI conversion to path failed for: {} (non-file scheme)",
                self.as_str()
            );
            return None;
        }

        let path = <lsp_types::Uri as TowerUriExt>::to_file_path(self)?;

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
            lsp_types::PositionEncodingKind::new("unknown").to_position_encoding(),
            None
        );
    }

    #[test]
    fn test_non_file_uri_returns_none() {
        // Step 1: Non-file URIs are rejected at the LSP boundary
        let uri = lsp_types::Uri::from_str("untitled:Untitled-1").unwrap();
        assert!(uri.to_utf8_path_buf().is_none());

        // TODO(virtual-paths): In Step 2, this should return Some(DocumentPath::Virtual(...))
    }
}
