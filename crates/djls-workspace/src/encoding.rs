use djls_source::PositionEncoding;
use tower_lsp_server::lsp_types::InitializeParams;
use tower_lsp_server::lsp_types::PositionEncodingKind;

/// Negotiate the best encoding with the client based on their capabilities.
/// Prefers UTF-8 > UTF-32 > UTF-16 for performance reasons.
pub fn negotiate_position_encoding(params: &InitializeParams) -> PositionEncoding {
    let client_encodings: &[PositionEncodingKind] = params
        .capabilities
        .general
        .as_ref()
        .and_then(|general| general.position_encodings.as_ref())
        .map_or(&[], |encodings| encodings.as_slice());

    // Try to find the best encoding in preference order
    for preferred in [
        PositionEncoding::Utf8,
        PositionEncoding::Utf32,
        PositionEncoding::Utf16,
    ] {
        if client_encodings
            .iter()
            .any(|kind| position_encoding_from_lsp(kind) == Some(preferred))
        {
            return preferred;
        }
    }

    // Fallback to UTF-16 if client doesn't specify encodings
    PositionEncoding::Utf16
}

// Helper functions to convert between LSP types and our PositionEncoding
#[must_use]
pub fn position_encoding_to_lsp(encoding: PositionEncoding) -> PositionEncodingKind {
    match encoding {
        PositionEncoding::Utf8 => PositionEncodingKind::new("utf-8"),
        PositionEncoding::Utf16 => PositionEncodingKind::new("utf-16"),
        PositionEncoding::Utf32 => PositionEncodingKind::new("utf-32"),
    }
}

#[must_use]
pub fn position_encoding_from_lsp(kind: &PositionEncodingKind) -> Option<PositionEncoding> {
    match kind.as_str() {
        "utf-8" => Some(PositionEncoding::Utf8),
        "utf-16" => Some(PositionEncoding::Utf16),
        "utf-32" => Some(PositionEncoding::Utf32),
        _ => None,
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
            position_encoding_from_lsp(&PositionEncodingKind::new("utf-8")),
            Some(PositionEncoding::Utf8)
        );
        assert_eq!(
            position_encoding_from_lsp(&PositionEncodingKind::new("utf-16")),
            Some(PositionEncoding::Utf16)
        );
        assert_eq!(
            position_encoding_from_lsp(&PositionEncodingKind::new("utf-32")),
            Some(PositionEncoding::Utf32)
        );

        // Invalid encoding returns None
        assert_eq!(
            position_encoding_from_lsp(&PositionEncodingKind::new("unknown")),
            None
        );

        // position_encoding_to_lsp produces correct LSP types
        assert_eq!(
            position_encoding_to_lsp(PositionEncoding::Utf8).as_str(),
            "utf-8"
        );
        assert_eq!(
            position_encoding_to_lsp(PositionEncoding::Utf16).as_str(),
            "utf-16"
        );
        assert_eq!(
            position_encoding_to_lsp(PositionEncoding::Utf32).as_str(),
            "utf-32"
        );
    }

    #[test]
    fn test_negotiate_prefers_utf8_when_all_available() {
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        PositionEncodingKind::new("utf-16"),
                        PositionEncodingKind::new("utf-8"),
                        PositionEncodingKind::new("utf-32"),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(negotiate_position_encoding(&params), PositionEncoding::Utf8);
    }

    #[test]
    fn test_negotiate_prefers_utf32_over_utf16() {
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        PositionEncodingKind::new("utf-16"),
                        PositionEncodingKind::new("utf-32"),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            negotiate_position_encoding(&params),
            PositionEncoding::Utf32
        );
    }

    #[test]
    fn test_negotiate_accepts_utf16_when_only_option() {
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![PositionEncodingKind::new("utf-16")]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            negotiate_position_encoding(&params),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_negotiate_fallback_with_empty_encodings() {
        let params = InitializeParams {
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
            negotiate_position_encoding(&params),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_negotiate_fallback_with_no_capabilities() {
        let params = InitializeParams::default();
        assert_eq!(
            negotiate_position_encoding(&params),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_negotiate_fallback_with_unknown_encodings() {
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        PositionEncodingKind::new("utf-7"),
                        PositionEncodingKind::new("ascii"),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            negotiate_position_encoding(&params),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_default_is_utf16() {
        assert_eq!(PositionEncoding::default(), PositionEncoding::Utf16);
    }
}
