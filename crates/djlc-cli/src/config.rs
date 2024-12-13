use clap::Args;
use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CONFIG_FILES: [&str; 3] = [
    "djls.toml", // highest priority
    ".djls.toml",
    "pyproject.toml", // lowest priority
];

#[derive(Args, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Override the virtual environment path
    #[arg(long, env = "DJLS_VENV_PATH")]
    pub venv_path: Option<PathBuf>,

    /// Django settings module
    #[arg(long, env = "DJANGO_SETTINGS_MODULE")]
    pub django_settings_module: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            venv_path: std::env::var("VIRTUAL_ENV").ok().map(PathBuf::from),
            django_settings_module: std::env::var("DJANGO_SETTINGS_MODULE").unwrap_or_default(),
        }
    }
}

impl Config {
    fn validate(self) -> Result<Self, ConfigError> {
        if self.django_settings_module.is_empty() {
            return Err(ConfigError::MissingDjangoSettings);
        }
        Ok(self)
    }
}

fn find_config_up_tree(start_dir: &Path) -> Vec<PathBuf> {
    let mut configs = Vec::new();
    let mut current_dir = start_dir.to_path_buf();

    while let Some(parent) = current_dir.parent() {
        for &config_name in CONFIG_FILES.iter() {
            let config_path = current_dir.join(config_name);
            if config_path.exists() {
                configs.push(config_path);
            }
        }
        current_dir = parent.to_path_buf();
    }

    configs
}

pub fn load_config(cli_config: &Config) -> Result<Config, ConfigError> {
    let platform_config = dirs::config_dir()
        .map(|dir| dir.join("djls/config.toml"))
        .filter(|p| p.exists());

    let mut figment = Figment::new()
        .merge(Serialized::defaults(Config::default()))
        .merge(
            platform_config
                .map(|p| Toml::file(&p))
                .unwrap_or_else(|| Toml::file("/dev/null")),
        );

    let current_dir = std::env::current_dir().map_err(ConfigError::CurrentDir)?;
    for path in find_config_up_tree(&current_dir) {
        figment = figment.merge(Toml::file(&path));
    }

    let config: Config = figment
        .merge(Env::raw().only(&["DJANGO_SETTINGS_MODULE"]))
        .merge(Env::prefixed("DJLS_").split("_"))
        .merge(Serialized::defaults(cli_config))
        .extract()?;

    config.validate()
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Django settings module not specified")]
    MissingDjangoSettings,
    #[error("could not determine current directory: {0}")]
    CurrentDir(std::io::Error),
    #[error("figment error: {0}")]
    Figment(#[from] figment::Error),
}
