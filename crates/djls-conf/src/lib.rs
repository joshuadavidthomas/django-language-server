use config::{Config, File, FileFormat, Value};
use djls_templates::TagSpecs;
use serde::Deserialize;
use thiserror::Error;

#[derive(Deserialize, Debug, Default, Clone)]
pub struct Settings {
    #[serde(default)]
    pub debug: bool,
    #[serde(default)]
    pub tagspecs: TagSpecs,
}
