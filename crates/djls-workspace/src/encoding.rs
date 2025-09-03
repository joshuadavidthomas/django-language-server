use tower_lsp_server::lsp_types::{InitializeParams, PositionEncodingKind};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl PositionEncoding {
    /// Get the LSP string representation of this encoding
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PositionEncoding::Utf8 => "utf-8",
            PositionEncoding::Utf16 => "utf-16",
            PositionEncoding::Utf32 => "utf-32",
        }
    }

    /// Convert from LSP [`PositionEncodingKind`](tower_lsp_server::lsp_types::PositionEncodingKind)
    #[must_use]
    pub fn from_lsp_kind(kind: &PositionEncodingKind) -> Option<Self> {
        match kind.as_str() {
            "utf-8" => Some(PositionEncoding::Utf8),
            "utf-16" => Some(PositionEncoding::Utf16),
            "utf-32" => Some(PositionEncoding::Utf32),
            _ => None,
        }
    }

    /// Convert to LSP [`PositionEncodingKind`](tower_lsp_server::lsp_types::PositionEncodingKind)
    #[must_use]
    pub fn to_lsp_kind(&self) -> PositionEncodingKind {
        PositionEncodingKind::new(self.as_str())
    }

    /// Negotiate the best encoding with the client based on their capabilities.
    /// Prefers UTF-8 > UTF-32 > UTF-16 for performance reasons.
    pub fn negotiate(params: &InitializeParams) -> Self {
        let client_encodings = params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
            .map(|encodings| encodings.as_slice())
            .unwrap_or(&[]);

        // Try to find the best encoding in preference order
        for preferred in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf32,
            PositionEncoding::Utf16,
        ] {
            if client_encodings
                .iter()
                .any(|kind| Self::from_lsp_kind(kind) == Some(preferred))
            {
                return preferred;
            }
        }

        // Fallback to UTF-16 if client doesn't specify encodings
        PositionEncoding::Utf16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::lsp_types::{ClientCapabilities, GeneralClientCapabilities};

    #[test]
    fn test_encoding_str_conversion() {
        assert_eq!(PositionEncoding::Utf8.as_str(), "utf-8");
        assert_eq!(PositionEncoding::Utf16.as_str(), "utf-16");
        assert_eq!(PositionEncoding::Utf32.as_str(), "utf-32");
    }

    #[test]
    fn test_from_lsp_kind() {
        assert_eq!(
            PositionEncoding::from_lsp_kind(&PositionEncodingKind::new("utf-8")),
            Some(PositionEncoding::Utf8)
        );
        assert_eq!(
            PositionEncoding::from_lsp_kind(&PositionEncodingKind::new("utf-16")),
            Some(PositionEncoding::Utf16)
        );
        assert_eq!(
            PositionEncoding::from_lsp_kind(&PositionEncodingKind::new("utf-32")),
            Some(PositionEncoding::Utf32)
        );
        assert_eq!(
            PositionEncoding::from_lsp_kind(&PositionEncodingKind::new("unknown")),
            None
        );
    }

    #[test]
    fn test_negotiate_prefers_utf8() {
        let mut params = InitializeParams::default();
        params.capabilities = ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: Some(vec![
                    PositionEncodingKind::new("utf-16"),
                    PositionEncodingKind::new("utf-8"),
                    PositionEncodingKind::new("utf-32"),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(PositionEncoding::negotiate(&params), PositionEncoding::Utf8);
    }

    #[test]
    fn test_negotiate_fallback_utf16() {
        let params = InitializeParams::default();
        assert_eq!(
            PositionEncoding::negotiate(&params),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_negotiate_prefers_utf32_over_utf16() {
        let mut params = InitializeParams::default();
        params.capabilities = ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: Some(vec![
                    PositionEncodingKind::new("utf-16"),
                    PositionEncodingKind::new("utf-32"),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(
            PositionEncoding::negotiate(&params),
            PositionEncoding::Utf32
        );
    }
}

