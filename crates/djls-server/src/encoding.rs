use djls_source::PositionEncoding;
use tower_lsp_server::lsp_types;

#[derive(Clone, Debug)]
pub struct LspPositionEncoding(lsp_types::PositionEncodingKind);

impl From<LspPositionEncoding> for lsp_types::PositionEncodingKind {
    fn from(value: LspPositionEncoding) -> Self {
        value.0
    }
}

impl From<&lsp_types::PositionEncodingKind> for LspPositionEncoding {
    fn from(value: &lsp_types::PositionEncodingKind) -> Self {
        Self(value.clone())
    }
}

impl From<lsp_types::PositionEncodingKind> for LspPositionEncoding {
    fn from(value: lsp_types::PositionEncodingKind) -> Self {
        Self(value)
    }
}

impl From<PositionEncoding> for LspPositionEncoding {
    fn from(value: PositionEncoding) -> Self {
        Self(match value {
            PositionEncoding::Utf8 => lsp_types::PositionEncodingKind::new("utf-8"),
            PositionEncoding::Utf16 => lsp_types::PositionEncodingKind::new("utf-16"),
            PositionEncoding::Utf32 => lsp_types::PositionEncodingKind::new("utf-32"),
        })
    }
}

impl LspPositionEncoding {
    #[must_use]
    pub fn to_position_encoding(&self) -> Option<PositionEncoding> {
        match self.0.as_str() {
            "utf-8" => Some(PositionEncoding::Utf8),
            "utf-16" => Some(PositionEncoding::Utf16),
            "utf-32" => Some(PositionEncoding::Utf32),
            _ => None,
        }
    }
}

impl From<&lsp_types::InitializeParams> for LspPositionEncoding {
    fn from(params: &lsp_types::InitializeParams) -> Self {
        let client_encodings: &[lsp_types::PositionEncodingKind] = params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
            .map_or(&[], |encodings| encodings.as_slice());

        for preferred in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf32,
            PositionEncoding::Utf16,
        ] {
            if client_encodings.iter().any(|kind| {
                LspPositionEncoding::from(kind).to_position_encoding() == Some(preferred)
            }) {
                return LspPositionEncoding::from(preferred);
            }
        }

        LspPositionEncoding::from(PositionEncoding::Utf16)
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::lsp_types::ClientCapabilities;
    use tower_lsp_server::lsp_types::GeneralClientCapabilities;

    use super::*;

    #[test]
    fn test_lsp_type_conversions() {
        // position_encoding_from_lsp for valid encodings
        assert_eq!(
            LspPositionEncoding::from(&lsp_types::PositionEncodingKind::new("utf-8"))
                .to_position_encoding(),
            Some(PositionEncoding::Utf8)
        );
        assert_eq!(
            LspPositionEncoding::from(&lsp_types::PositionEncodingKind::new("utf-16"))
                .to_position_encoding(),
            Some(PositionEncoding::Utf16)
        );
        assert_eq!(
            LspPositionEncoding::from(&lsp_types::PositionEncodingKind::new("utf-32"))
                .to_position_encoding(),
            Some(PositionEncoding::Utf32)
        );

        // Invalid encoding returns None
        assert_eq!(
            LspPositionEncoding::from(&lsp_types::PositionEncodingKind::new("unknown"))
                .to_position_encoding(),
            None
        );
    }

    #[test]
    fn test_negotiate_prefers_utf8_when_all_available() {
        let params = lsp_types::InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        lsp_types::PositionEncodingKind::new("utf-16"),
                        lsp_types::PositionEncodingKind::new("utf-8"),
                        lsp_types::PositionEncodingKind::new("utf-32"),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            LspPositionEncoding::from(&params).to_position_encoding(),
            Some(PositionEncoding::Utf8)
        );
    }

    #[test]
    fn test_negotiate_prefers_utf32_over_utf16() {
        let params = lsp_types::InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        lsp_types::PositionEncodingKind::new("utf-16"),
                        lsp_types::PositionEncodingKind::new("utf-32"),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            LspPositionEncoding::from(&params).to_position_encoding(),
            Some(PositionEncoding::Utf32)
        );
    }

    #[test]
    fn test_negotiate_accepts_utf16_when_only_option() {
        let params = lsp_types::InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![lsp_types::PositionEncodingKind::new("utf-16")]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            LspPositionEncoding::from(&params).to_position_encoding(),
            Some(PositionEncoding::Utf16)
        );
    }

    #[test]
    fn test_negotiate_fallback_with_empty_encodings() {
        let params = lsp_types::InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            LspPositionEncoding::from(&params).to_position_encoding(),
            Some(PositionEncoding::Utf16)
        );
    }

    #[test]
    fn test_negotiate_fallback_with_no_capabilities() {
        let params = lsp_types::InitializeParams::default();
        assert_eq!(
            LspPositionEncoding::from(&params).to_position_encoding(),
            Some(PositionEncoding::Utf16)
        );
    }

    #[test]
    fn test_negotiate_fallback_with_unknown_encodings() {
        let params = lsp_types::InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        lsp_types::PositionEncodingKind::new("utf-7"),
                        lsp_types::PositionEncodingKind::new("ascii"),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            LspPositionEncoding::from(&params).to_position_encoding(),
            Some(PositionEncoding::Utf16)
        );
    }
}
