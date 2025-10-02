use std::fmt;

/// Specifies how column positions are counted in text.
///
/// While motivated by LSP (Language Server Protocol) requirements, this enum
/// represents a fundamental choice about text position measurement that any
/// text processing system must make. Different systems count "column" positions
/// differently:
///
/// - Some count bytes (fast but breaks on multi-byte characters)
/// - Some count UTF-16 code units (common in JavaScript/Windows ecosystems)
/// - Some count Unicode codepoints (intuitive but slower)
///
/// This crate provides encoding-aware position conversion to support different
/// client expectations without coupling to specific protocol implementations.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    /// Column positions count UTF-8 code units (bytes from line start)
    Utf8,
    /// Column positions count UTF-16 code units (common in VS Code and Windows editors)
    #[default]
    Utf16,
    /// Column positions count Unicode scalar values (codepoints)
    Utf32,
}

impl fmt::Display for PositionEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utf8 => write!(f, "utf-8"),
            Self::Utf16 => write!(f, "utf-16"),
            Self::Utf32 => write!(f, "utf-32"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_utf16() {
        assert_eq!(PositionEncoding::default(), PositionEncoding::Utf16);
    }

    #[test]
    fn test_position_encoding_display() {
        assert_eq!(PositionEncoding::Utf8.to_string(), "utf-8");
        assert_eq!(PositionEncoding::Utf16.to_string(), "utf-16");
        assert_eq!(PositionEncoding::Utf32.to_string(), "utf-32");
    }
}
