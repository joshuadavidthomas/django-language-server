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
    fn test_encoding_str_conversion() {
        // Test FromStr trait
        assert_eq!("utf-8".parse(), Ok(PositionEncoding::Utf8));
        assert_eq!("utf-16".parse(), Ok(PositionEncoding::Utf16));
        assert_eq!("utf-32".parse(), Ok(PositionEncoding::Utf32));
        assert!("invalid".parse::<PositionEncoding>().is_err());

        // Test ToString trait
        assert_eq!(PositionEncoding::Utf8.to_string(), "utf-8");
        assert_eq!(PositionEncoding::Utf16.to_string(), "utf-16");
        assert_eq!(PositionEncoding::Utf32.to_string(), "utf-32");
    }

    #[test]
    fn test_from_lsp_kind() {
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
        assert!(PositionEncoding::try_from(PositionEncodingKind::new("unknown")).is_err());
    }

    #[test]
    fn test_trait_conversions() {
        // Test TryFrom<PositionEncodingKind> for PositionEncoding
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
        assert!(PositionEncoding::try_from(PositionEncodingKind::new("unknown")).is_err());

        // Test From<PositionEncoding> for PositionEncodingKind
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
    fn test_negotiate_prefers_utf8() {
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
    fn test_negotiate_fallback_utf16() {
        let params = InitializeParams::default();
        assert_eq!(
            PositionEncoding::negotiate(&params),
            PositionEncoding::Utf16
        );
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
}
