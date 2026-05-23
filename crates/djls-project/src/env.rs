use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;

use crate::discovery::EnvFileLoadIssueKind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvFileLoadOutcome {
    source: Utf8PathBuf,
    entries: Vec<(String, String)>,
    issue: Option<EnvFileLoadIssueKind>,
}

impl EnvFileLoadOutcome {
    #[must_use]
    pub fn source(&self) -> &Utf8Path {
        &self.source
    }

    #[must_use]
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    #[must_use]
    pub fn issue(&self) -> Option<EnvFileLoadIssueKind> {
        self.issue
    }
}

#[must_use]
pub fn load_env_file(root: &Utf8Path, settings: &Settings) -> Vec<(String, String)> {
    load_env_file_outcome(root, settings).entries
}

pub fn load_env_file_outcome(root: &Utf8Path, settings: &Settings) -> EnvFileLoadOutcome {
    let env_path = match settings.env_file() {
        Some(path) => root.join(path),
        None => root.join(".env"),
    };

    if !env_path.exists() {
        if settings.env_file().is_some() {
            tracing::warn!("Configured env_file not found: {}", env_path);
            return EnvFileLoadOutcome {
                source: env_path,
                entries: Vec::new(),
                issue: Some(EnvFileLoadIssueKind::Missing),
            };
        }
        tracing::debug!("No .env file found at {}", env_path);
        return EnvFileLoadOutcome {
            source: env_path,
            entries: Vec::new(),
            issue: None,
        };
    }

    match dotenvy::from_path_iter(env_path.as_std_path()) {
        Ok(iter) => {
            let mut vars = Vec::new();
            let mut issue = None;
            for item in iter {
                match item {
                    Ok((key, value)) => {
                        tracing::debug!("Loaded env var from file: {}", key);
                        vars.push((key, value));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse env file entry: {}", e);
                        issue = Some(EnvFileLoadIssueKind::Parse);
                    }
                }
            }
            if !vars.is_empty() {
                tracing::info!(
                    "Loaded {} environment variable(s) from env file",
                    vars.len()
                );
            }
            EnvFileLoadOutcome {
                source: env_path,
                entries: vars,
                issue,
            }
        }
        Err(e) => {
            tracing::warn!("Failed to read env file {}: {}", env_path, e);
            EnvFileLoadOutcome {
                source: env_path,
                entries: Vec::new(),
                issue: Some(EnvFileLoadIssueKind::Io),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use camino::Utf8Path;
    use djls_conf::Settings;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn loads_default_env_file() {
        let dir = tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap();
        fs::write(
            root.join(".env"),
            "DJANGO_SETTINGS_MODULE=config.settings\n",
        )
        .unwrap();

        let vars = load_env_file(root, &Settings::default());

        assert_eq!(
            vars,
            vec![(
                "DJANGO_SETTINGS_MODULE".to_string(),
                "config.settings".to_string()
            )]
        );
    }

    #[test]
    fn returns_empty_when_default_env_file_is_missing() {
        let dir = tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap();

        let outcome = load_env_file_outcome(root, &Settings::default());

        assert!(outcome.entries().is_empty());
        assert_eq!(outcome.issue(), None);
    }
}
