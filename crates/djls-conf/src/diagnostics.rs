use std::collections::HashMap;

use serde::Deserialize;

/// Diagnostic severity level for LSP diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Off,
    Error,
    Warning,
    Info,
    Hint,
}

impl DiagnosticSeverity {
    /// Convert to LSP diagnostic severity.
    /// Returns None for Off (diagnostic should not be shown).
    pub fn to_lsp_severity(self) -> Option<tower_lsp_server::lsp_types::DiagnosticSeverity> {
        match self {
            DiagnosticSeverity::Off => None,
            DiagnosticSeverity::Error => {
                Some(tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR)
            }
            DiagnosticSeverity::Warning => {
                Some(tower_lsp_server::lsp_types::DiagnosticSeverity::WARNING)
            }
            DiagnosticSeverity::Info => {
                Some(tower_lsp_server::lsp_types::DiagnosticSeverity::INFORMATION)
            }
            DiagnosticSeverity::Hint => {
                Some(tower_lsp_server::lsp_types::DiagnosticSeverity::HINT)
            }
        }
    }
}

/// Configuration for diagnostic severity levels.
///
/// All diagnostics are enabled by default at "error" severity.
/// Configure severity per diagnostic code or prefix pattern.
/// Specific codes override prefix patterns.
///
/// Example configuration:
/// ```toml
/// [tool.djls.diagnostics.severity]
/// # Individual codes
/// S101 = "warning"
/// S102 = "off"
///
/// # Prefixes for bulk configuration
/// "T" = "off"     # Disable all template errors
/// T100 = "hint"   # But show parser errors as hints (specific overrides prefix)
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct DiagnosticsConfig {
    /// Map of diagnostic codes/prefixes to severity levels.
    /// Supports:
    /// - Specific codes: "S100", "T100"
    /// - Prefixes: "S" (all S-series), "T" (all T-series), "S1" (S100-S199)
    /// - More specific patterns override less specific ones
    #[serde(default)]
    pub severity: HashMap<String, DiagnosticSeverity>,
}

impl DiagnosticsConfig {
    /// Get the severity level for a diagnostic code.
    ///
    /// Resolution order (most specific wins):
    /// 1. Exact match (e.g., "S100")
    /// 2. Longest prefix match (e.g., "S1" over "S")
    /// 3. Default: Error
    ///
    /// # Examples
    /// ```
    /// # use djls_conf::diagnostics::{DiagnosticsConfig, DiagnosticSeverity};
    /// # use std::collections::HashMap;
    /// let mut severity = HashMap::new();
    /// severity.insert("S".to_string(), DiagnosticSeverity::Warning);
    /// severity.insert("S1".to_string(), DiagnosticSeverity::Off);
    /// severity.insert("S100".to_string(), DiagnosticSeverity::Error);
    ///
    /// let config = DiagnosticsConfig { severity };
    ///
    /// assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Error);  // Exact
    /// assert_eq!(config.get_severity("S101"), DiagnosticSeverity::Off);    // "S1" prefix
    /// assert_eq!(config.get_severity("S200"), DiagnosticSeverity::Warning); // "S" prefix
    /// assert_eq!(config.get_severity("T100"), DiagnosticSeverity::Error);   // Default
    /// ```
    pub fn get_severity(&self, code: &str) -> DiagnosticSeverity {
        // First, check for exact match
        if let Some(&severity) = self.severity.get(code) {
            return severity;
        }

        // Then, find the longest matching prefix
        let mut best_match: Option<(&str, DiagnosticSeverity)> = None;

        for (pattern, &severity) in &self.severity {
            if code.starts_with(pattern) {
                match best_match {
                    None => best_match = Some((pattern, severity)),
                    Some((existing_pattern, _)) => {
                        // Longer patterns are more specific
                        if pattern.len() > existing_pattern.len() {
                            best_match = Some((pattern, severity));
                        }
                    }
                }
            }
        }

        best_match
            .map(|(_, severity)| severity)
            .unwrap_or(DiagnosticSeverity::Error)
    }

    /// Check if a diagnostic should be shown (severity is not Off).
    pub fn is_enabled(&self, code: &str) -> bool {
        self.get_severity(code) != DiagnosticSeverity::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_severity_default() {
        let config = DiagnosticsConfig::default();
        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Error);
        assert_eq!(config.get_severity("T100"), DiagnosticSeverity::Error);
    }

    #[test]
    fn test_get_severity_exact_match() {
        let mut severity = HashMap::new();
        severity.insert("S100".to_string(), DiagnosticSeverity::Warning);
        severity.insert("S101".to_string(), DiagnosticSeverity::Off);

        let config = DiagnosticsConfig { severity };

        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Warning);
        assert_eq!(config.get_severity("S101"), DiagnosticSeverity::Off);
        assert_eq!(config.get_severity("S102"), DiagnosticSeverity::Error);
    }

    #[test]
    fn test_get_severity_prefix_match() {
        let mut severity = HashMap::new();
        severity.insert("S".to_string(), DiagnosticSeverity::Warning);
        severity.insert("T".to_string(), DiagnosticSeverity::Off);

        let config = DiagnosticsConfig { severity };

        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Warning);
        assert_eq!(config.get_severity("S101"), DiagnosticSeverity::Warning);
        assert_eq!(config.get_severity("T100"), DiagnosticSeverity::Off);
        assert_eq!(config.get_severity("T900"), DiagnosticSeverity::Off);
    }

    #[test]
    fn test_get_severity_longest_prefix_wins() {
        let mut severity = HashMap::new();
        severity.insert("S".to_string(), DiagnosticSeverity::Warning);
        severity.insert("S1".to_string(), DiagnosticSeverity::Off);
        severity.insert("S10".to_string(), DiagnosticSeverity::Hint);

        let config = DiagnosticsConfig { severity };

        // S10 is most specific for S100
        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Hint);
        assert_eq!(config.get_severity("S101"), DiagnosticSeverity::Hint);
        // S1 is most specific for S110
        assert_eq!(config.get_severity("S110"), DiagnosticSeverity::Off);
        assert_eq!(config.get_severity("S199"), DiagnosticSeverity::Off);
        // S is most specific for S200
        assert_eq!(config.get_severity("S200"), DiagnosticSeverity::Warning);
    }

    #[test]
    fn test_get_severity_exact_overrides_prefix() {
        let mut severity = HashMap::new();
        severity.insert("S".to_string(), DiagnosticSeverity::Warning);
        severity.insert("S1".to_string(), DiagnosticSeverity::Off);
        severity.insert("S100".to_string(), DiagnosticSeverity::Error);

        let config = DiagnosticsConfig { severity };

        // Exact match wins
        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Error);
        // S1 prefix for other S1xx codes
        assert_eq!(config.get_severity("S101"), DiagnosticSeverity::Off);
        // S prefix for S2xx codes
        assert_eq!(config.get_severity("S200"), DiagnosticSeverity::Warning);
    }

    #[test]
    fn test_is_enabled_default() {
        let config = DiagnosticsConfig::default();
        assert!(config.is_enabled("S100"));
        assert!(config.is_enabled("T100"));
    }

    #[test]
    fn test_is_enabled_with_off() {
        let mut severity = HashMap::new();
        severity.insert("S100".to_string(), DiagnosticSeverity::Off);

        let config = DiagnosticsConfig { severity };

        assert!(!config.is_enabled("S100"));
        assert!(config.is_enabled("S101"));
    }

    #[test]
    fn test_is_enabled_with_prefix_off() {
        let mut severity = HashMap::new();
        severity.insert("T".to_string(), DiagnosticSeverity::Off);

        let config = DiagnosticsConfig { severity };

        assert!(!config.is_enabled("T100"));
        assert!(!config.is_enabled("T900"));
        assert!(config.is_enabled("S100"));
    }

    #[test]
    fn test_is_enabled_prefix_off_with_specific_override() {
        let mut severity = HashMap::new();
        severity.insert("T".to_string(), DiagnosticSeverity::Off);
        severity.insert("T100".to_string(), DiagnosticSeverity::Hint);

        let config = DiagnosticsConfig { severity };

        // T100 has specific override, so it's enabled
        assert!(config.is_enabled("T100"));
        // Other T codes are off
        assert!(!config.is_enabled("T900"));
        assert!(!config.is_enabled("T901"));
    }

    #[test]
    fn test_to_lsp_severity() {
        assert_eq!(DiagnosticSeverity::Off.to_lsp_severity(), None);
        assert_eq!(
            DiagnosticSeverity::Error.to_lsp_severity(),
            Some(tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR)
        );
        assert_eq!(
            DiagnosticSeverity::Warning.to_lsp_severity(),
            Some(tower_lsp_server::lsp_types::DiagnosticSeverity::WARNING)
        );
        assert_eq!(
            DiagnosticSeverity::Info.to_lsp_severity(),
            Some(tower_lsp_server::lsp_types::DiagnosticSeverity::INFORMATION)
        );
        assert_eq!(
            DiagnosticSeverity::Hint.to_lsp_severity(),
            Some(tower_lsp_server::lsp_types::DiagnosticSeverity::HINT)
        );
    }

    #[test]
    fn test_deserialize_diagnostics_config() {
        let toml = r#"
            [severity]
            S100 = "off"
            S101 = "warning"
            S102 = "hint"
            "T" = "off"
            T100 = "info"
        "#;

        let config: DiagnosticsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.severity.get("S100"), Some(&DiagnosticSeverity::Off));
        assert_eq!(
            config.severity.get("S101"),
            Some(&DiagnosticSeverity::Warning)
        );
        assert_eq!(config.severity.get("S102"), Some(&DiagnosticSeverity::Hint));
        assert_eq!(config.severity.get("T"), Some(&DiagnosticSeverity::Off));
        assert_eq!(config.severity.get("T100"), Some(&DiagnosticSeverity::Info));
    }

    #[test]
    fn test_complex_scenario() {
        let mut severity = HashMap::new();
        // Disable all template errors
        severity.insert("T".to_string(), DiagnosticSeverity::Off);
        // But show parser errors as hints
        severity.insert("T100".to_string(), DiagnosticSeverity::Hint);
        // Make all semantic errors warnings
        severity.insert("S".to_string(), DiagnosticSeverity::Warning);
        // Except S100 which is completely off
        severity.insert("S100".to_string(), DiagnosticSeverity::Off);
        // And S10x (S100-S109) should be info
        severity.insert("S10".to_string(), DiagnosticSeverity::Info);

        let config = DiagnosticsConfig { severity };

        // S100 is exact match - off
        assert_eq!(config.get_severity("S100"), DiagnosticSeverity::Off);
        assert!(!config.is_enabled("S100"));

        // S101 matches S10 prefix - info
        assert_eq!(config.get_severity("S101"), DiagnosticSeverity::Info);
        assert!(config.is_enabled("S101"));

        // S200 matches S prefix - warning
        assert_eq!(config.get_severity("S200"), DiagnosticSeverity::Warning);
        assert!(config.is_enabled("S200"));

        // T100 has exact match - hint
        assert_eq!(config.get_severity("T100"), DiagnosticSeverity::Hint);
        assert!(config.is_enabled("T100"));

        // T900 matches T prefix - off
        assert_eq!(config.get_severity("T900"), DiagnosticSeverity::Off);
        assert!(!config.is_enabled("T900"));
    }
}
