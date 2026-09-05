use serde::Deserialize;

#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone)]
pub struct FormatConfig {
    #[serde(default)]
    enabled: bool,
}

impl FormatConfig {
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}
