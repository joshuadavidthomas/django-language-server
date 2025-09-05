use std::fmt;
use std::str::FromStr;

use tower_lsp_server::lsp_types::InitializeParams;
use tower_lsp_server::lsp_types::PositionEncodingKind;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl PositionEncoding {
    /// Negotiate the best encoding with the client based on their capabilities.
    /// Prefers UTF-8 > UTF-32 > UTF-16 for performance reasons.
    pub fn negotiate(params: &InitializeParams) -> Self {
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
                .any(|kind| PositionEncoding::try_from(kind.clone()).ok() == Some(preferred))
            {
                return preferred;
            }
        }

        // Fallback to UTF-16 if client doesn't specify encodings
        PositionEncoding::Utf16
    }
}

impl FromStr for PositionEncoding {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "utf-8" => Ok(PositionEncoding::Utf8),
            "utf-16" => Ok(PositionEncoding::Utf16),
            "utf-32" => Ok(PositionEncoding::Utf32),
            _ => Err(()),
        }
    }
}

impl fmt::Display for PositionEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            PositionEncoding::Utf8 => "utf-8",
            PositionEncoding::Utf16 => "utf-16",
            PositionEncoding::Utf32 => "utf-32",
        };
        write!(f, "{s}")
    }
}

impl From<PositionEncoding> for PositionEncodingKind {
    fn from(encoding: PositionEncoding) -> Self {
        match encoding {
            PositionEncoding::Utf8 => PositionEncodingKind::new("utf-8"),
            PositionEncoding::Utf16 => PositionEncodingKind::new("utf-16"),
            PositionEncoding::Utf32 => PositionEncodingKind::new("utf-32"),
        }
    }
}

impl TryFrom<PositionEncodingKind> for PositionEncoding {
    type Error = ();

    fn try_from(kind: PositionEncodingKind) -> Result<Self, Self::Error> {
        kind.as_str().parse()
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::lsp_types::ClientCapabilities;
    use tower_lsp_server::lsp_types::GeneralClientCapabilities;

    use super::*;

    #[test]
    fn test_string_parsing_and_display() {
        // Valid encodings parse correctly
        assert_eq!(
            "utf-8".parse::<PositionEncoding>(),
            Ok(PositionEncoding::Utf8)
        );
        assert_eq!(
            "utf-16".parse::<PositionEncoding>(),
            Ok(PositionEncoding::Utf16)
        );
        assert_eq!(
            "utf-32".parse::<PositionEncoding>(),
            Ok(PositionEncoding::Utf32)
        );

        // Invalid encoding returns error
        assert!("invalid".parse::<PositionEncoding>().is_err());
        assert!("UTF-8".parse::<PositionEncoding>().is_err()); // case sensitive

        // Display produces correct strings
        assert_eq!(PositionEncoding::Utf8.to_string(), "utf-8");
        assert_eq!(PositionEncoding::Utf16.to_string(), "utf-16");
        assert_eq!(PositionEncoding::Utf32.to_string(), "utf-32");
    }

    #[test]
    fn test_lsp_type_conversions() {
        // TryFrom<PositionEncodingKind> for valid encodings
        assert_eq!(
            PositionEncoding::try_from(PositionEncodingKind::new("utf-8")),
            Ok(PositionEncoding::Utf8)
        );
        assert_eq!(
            PositionEncoding::try_from(PositionEncodingKind::new("utf-16")),
            Ok(PositionEncoding::Utf16)
        );
        assert_eq!(
            PositionEncoding::try_from(PositionEncodingKind::new("utf-32")),
            Ok(PositionEncoding::Utf32)
        );

        // Invalid encoding returns error
        assert!(PositionEncoding::try_from(PositionEncodingKind::new("unknown")).is_err());

        // From<PositionEncoding> produces correct LSP types
        assert_eq!(
            PositionEncodingKind::from(PositionEncoding::Utf8).as_str(),
            "utf-8"
        );
        assert_eq!(
            PositionEncodingKind::from(PositionEncoding::Utf16).as_str(),
            "utf-16"
        );
        assert_eq!(
            PositionEncodingKind::from(PositionEncoding::Utf32).as_str(),
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

        assert_eq!(PositionEncoding::negotiate(&params), PositionEncoding::Utf8);
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
            PositionEncoding::negotiate(&params),
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
            PositionEncoding::negotiate(&params),
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
            PositionEncoding::negotiate(&params),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_negotiate_fallback_with_no_capabilities() {
        let params = InitializeParams::default();
        assert_eq!(
            PositionEncoding::negotiate(&params),
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
            PositionEncoding::negotiate(&params),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn test_default_is_utf16() {
        assert_eq!(PositionEncoding::default(), PositionEncoding::Utf16);
    }
}
