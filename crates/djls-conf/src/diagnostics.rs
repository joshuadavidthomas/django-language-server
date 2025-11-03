use std::collections::HashMap;

use serde::Deserialize;

/// Diagnostic severity level for LSP diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl DiagnosticSeverity {
    /// Convert to LSP diagnostic severity
    pub fn to_lsp_severity(self) -> tower_lsp_server::lsp_types::DiagnosticSeverity {
        match self {
            DiagnosticSeverity::Error => tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR,
            DiagnosticSeverity::Warning => {
                tower_lsp_server::lsp_types::DiagnosticSeverity::WARNING
            }
            DiagnosticSeverity::Info => tower_lsp_server::lsp_types::DiagnosticSeverity::INFORMATION,
            DiagnosticSeverity::Hint => tower_lsp_server::lsp_types::DiagnosticSeverity::HINT,
        }
    }
}

/// Configuration for diagnostic rules, inspired by Ruff's approach.
///
/// Example configuration:
/// ```toml
/// [tool.djls.diagnostics]
/// select = ["ALL"]
/// ignore = ["S100", "T100"]
///
/// [tool.djls.diagnostics.severity]
/// S101 = "warning"
/// S103 = "hint"
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DiagnosticsConfig {
    /// Diagnostic codes to enable. Supports prefixes like "S", "T", "S1", "T9".
    /// Default: ["ALL"] (all diagnostics enabled)
    #[serde(default = "default_select")]
    pub select: Vec<String>,

    /// Diagnostic codes to disable. Supports prefixes like "S", "T".
    /// Applied after `select`.
    #[serde(default)]
    pub ignore: Vec<String>,

    /// Override severity for specific diagnostic codes.
    /// Maps diagnostic code to severity level (error, warning, info, hint).
    #[serde(default)]
    pub severity: HashMap<String, DiagnosticSeverity>,
}

fn default_select() -> Vec<String> {
    vec!["ALL".to_string()]
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            select: default_select(),
            ignore: Vec::new(),
            severity: HashMap::new(),
        }
    }
}

impl DiagnosticsConfig {
    /// Check if a diagnostic code is enabled based on select and ignore rules.
    ///
    /// # Examples
    /// ```
    /// # use djls_conf::diagnostics::DiagnosticsConfig;
    /// let config = DiagnosticsConfig {
    ///     select: vec!["ALL".to_string()],
    ///     ignore: vec!["S100".to_string()],
    ///     severity: Default::default(),
    /// };
    /// assert!(!config.is_enabled("S100"));
    /// assert!(config.is_enabled("S101"));
    /// ```
    pub fn is_enabled(&self, code: &str) -> bool {
        // First check if it's selected
        let selected = self.select.iter().any(|pattern| {
            if pattern == "ALL" {
                true
            } else {
                matches_prefix(code, pattern)
            }
        });

        if !selected {
            return false;
        }

        // Then check if it's ignored
        let ignored = self
            .ignore
            .iter()
            .any(|pattern| matches_prefix(code, pattern));

        !ignored
    }

    /// Get the severity level for a diagnostic code.
    /// Returns the overridden severity if configured, otherwise returns Error as default.
    pub fn get_severity(&self, code: &str) -> DiagnosticSeverity {
        self.severity
            .get(code)
            .copied()
            .unwrap_or(DiagnosticSeverity::Error)
    }
}

/// Check if a diagnostic code matches a pattern/prefix.
///
/// Patterns can be:
/// - Exact match: "S100" matches "S100"
/// - Prefix match: "S" matches "S100", "S101", etc.
/// - Prefix match: "S1" matches "S100", "S101", etc. but not "S200"
/// - Prefix match: "T9" matches "T900", "T901", etc. but not "T100"
fn matches_prefix(code: &str, pattern: &str) -> bool {
    if pattern == "ALL" {
        return true;
    }
    code.starts_with(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_prefix_exact() {
        assert!(matches_prefix("S100", "S100"));
        assert!(!matches_prefix("S100", "S101"));
    }

    #[test]
    fn test_matches_prefix_single_letter() {
        assert!(matches_prefix("S100", "S"));
        assert!(matches_prefix("S101", "S"));
        assert!(matches_prefix("T100", "T"));
        assert!(!matches_prefix("S100", "T"));
    }

    #[test]
    fn test_matches_prefix_two_chars() {
        assert!(matches_prefix("S100", "S1"));
        assert!(matches_prefix("S101", "S1"));
        assert!(matches_prefix("S199", "S1"));
        assert!(!matches_prefix("S200", "S1"));
        assert!(matches_prefix("T900", "T9"));
        assert!(!matches_prefix("T100", "T9"));
    }

    #[test]
    fn test_matches_prefix_all() {
        assert!(matches_prefix("S100", "ALL"));
        assert!(matches_prefix("T900", "ALL"));
    }

    #[test]
    fn test_is_enabled_default() {
        let config = DiagnosticsConfig::default();
        assert!(config.is_enabled("S100"));
        assert!(config.is_enabled("T100"));
    }

    #[test]
    fn test_is_enabled_with_ignore() {
        let config = DiagnosticsConfig {
            select: vec!["ALL".to_string()],
            ignore: vec!["S100".to_string()],
            severity: HashMap::new(),
        };
        assert!(!config.is_enabled("S100"));
        assert!(config.is_enabled("S101"));
    }

    #[test]
    fn test_is_enabled_with_prefix_ignore() {
        let config = DiagnosticsConfig {
            select: vec!["ALL".to_string()],
            ignore: vec!["S".to_string()],
            severity: HashMap::new(),
        };
        assert!(!config.is_enabled("S100"));
        assert!(!config.is_enabled("S101"));
        assert!(config.is_enabled("T100"));
    }

    #[test]
    fn test_is_enabled_with_select_only_semantic() {
        let config = DiagnosticsConfig {
            select: vec!["S".to_string()],
            ignore: vec![],
            severity: HashMap::new(),
        };
        assert!(config.is_enabled("S100"));
        assert!(config.is_enabled("S101"));
        assert!(!config.is_enabled("T100"));
    }

    #[test]
    fn test_is_enabled_with_select_and_ignore() {
        let config = DiagnosticsConfig {
            select: vec!["S".to_string()],
            ignore: vec!["S100".to_string(), "S101".to_string()],
            severity: HashMap::new(),
        };
        assert!(!config.is_enabled("S100"));
        assert!(!config.is_enabled("S101"));
        assert!(config.is_enabled("S102"));
        assert!(!config.is_enabled("T100"));
    }

    #[test]
    fn test_is_enabled_with_prefix_select_and_ignore() {
        let config = DiagnosticsConfig {
            select: vec!["S1".to_string()],
            ignore: vec!["S10".to_string()],
            severity: HashMap::new(),
        };
        assert!(!config.is_enabled("S100")); // Matched by S10 ignore
        assert!(!config.is_enabled("S101")); // Matched by S10 ignore
        assert!(config.is_enabled("S110")); // Not matched by S10, but by S1
        assert!(config.is_enabled("S199")); // Not matched by S10, but by S1
        assert!(!config.is_enabled("S200")); // Not selected by S1
    }

    #[test]
    fn test_get_severity_default() {
        let config = DiagnosticsConfig::default();
        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Error);
    }

    #[test]
    fn test_get_severity_override() {
        let mut severity = HashMap::new();
        severity.insert("S100".to_string(), DiagnosticSeverity::Warning);
        severity.insert("S101".to_string(), DiagnosticSeverity::Hint);

        let config = DiagnosticsConfig {
            select: vec!["ALL".to_string()],
            ignore: vec![],
            severity,
        };

        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Warning);
        assert_eq!(config.get_severity("S101"), DiagnosticSeverity::Hint);
        assert_eq!(config.get_severity("S102"), DiagnosticSeverity::Error);
    }

    #[test]
    fn test_deserialize_diagnostics_config() {
        let toml = r#"
            select = ["S", "T"]
            ignore = ["S100"]

            [severity]
            S101 = "warning"
            S102 = "hint"
        "#;

        let config: DiagnosticsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.select, vec!["S", "T"]);
        assert_eq!(config.ignore, vec!["S100"]);
        assert_eq!(
            config.severity.get("S101"),
            Some(&DiagnosticSeverity::Warning)
        );
        assert_eq!(config.severity.get("S102"), Some(&DiagnosticSeverity::Hint));
    }
}
