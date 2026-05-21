use camino::Utf8Path;
use djls_conf::Settings;

pub fn load_env_file(root: &Utf8Path, settings: &Settings) -> Vec<(String, String)> {
    let env_path = match settings.env_file() {
        Some(path) => root.join(path),
        None => root.join(".env"),
    };

    if !env_path.exists() {
        if settings.env_file().is_some() {
            tracing::warn!("Configured env_file not found: {}", env_path);
        } else {
            tracing::debug!("No .env file found at {}", env_path);
        }
        return Vec::new();
    }

    match dotenvy::from_path_iter(env_path.as_std_path()) {
        Ok(iter) => {
            let mut vars = Vec::new();
            for item in iter {
                match item {
                    Ok((key, value)) => {
                        tracing::debug!("Loaded env var from file: {}", key);
                        vars.push((key, value));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse env file entry: {}", e);
                    }
                }
            }
            if !vars.is_empty() {
                tracing::info!(
                    "Loaded {} environment variable(s) from env file",
                    vars.len()
                );
            }
            vars
        }
        Err(e) => {
            tracing::warn!("Failed to read env file {}: {}", env_path, e);
            Vec::new()
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
    fn returns_empty_when_env_file_is_missing() {
        let dir = tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap();

        let vars = load_env_file(root, &Settings::default());

        assert!(vars.is_empty());
    }
}
