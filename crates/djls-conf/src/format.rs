use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
pub struct FormatConfig {
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    backend: FormatBackend,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: FormatBackend::Djangofmt,
        }
    }
}

impl FormatConfig {
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn backend(&self) -> FormatBackend {
        self.backend
    }
}

#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum FormatBackend {
    #[default]
    Djangofmt,
}

fn default_enabled() -> bool {
    false
}
