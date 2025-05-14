use std::fs;
use std::path::Path;

use config::Config;
use config::ConfigError as ExternalConfigError;
use config::File;
use config::FileFormat;
use directories::ProjectDirs;
use serde::Deserialize;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Configuration build/deserialize error")]
    Config(#[from] ExternalConfigError),
    #[error("Failed to read pyproject.toml")]
    PyprojectIo(#[from] std::io::Error),
    #[error("Failed to parse pyproject.toml TOML")]
    PyprojectParse(#[from] toml::de::Error),
    #[error("Failed to serialize extracted pyproject.toml data")]
    PyprojectSerialize(#[from] toml::ser::Error),
}

#[derive(Debug, Deserialize, Default, PartialEq)]
pub struct Settings {
    #[serde(default)]
    debug: bool,
    venv_path: Option<String>,
}

impl Settings {
    pub fn new(project_root: &Path) -> Result<Self, ConfigError> {
        let user_config_file = ProjectDirs::from("com.github", "joshuadavidthomas", "djls")
            .map(|proj_dirs| proj_dirs.config_dir().join("djls.toml"));

        Self::load_from_paths(project_root, user_config_file.as_deref())
    }

    fn load_from_paths(
        project_root: &Path,
        user_config_path: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let mut builder = Config::builder();

        if let Some(path) = user_config_path {
            builder = builder.add_source(File::from(path).format(FileFormat::Toml).required(false));
        }

        let pyproject_path = project_root.join("pyproject.toml");
        if pyproject_path.exists() {
            let content = fs::read_to_string(&pyproject_path)?;
            let toml_str: toml::Value = toml::from_str(&content)?;
            let tool_djls_value: Option<&toml::Value> =
                ["tool", "djls"].iter().try_fold(&toml_str, |val, &key| {
                    // Attempt to get the next key. If it exists, return Some(value) to continue.
                    // If get returns None, try_fold automatically stops and returns None overall.
                    val.get(key)
                });
            if let Some(tool_djls_table) = tool_djls_value.and_then(|v| v.as_table()) {
                let tool_djls_string = toml::to_string(tool_djls_table)?;
                builder = builder.add_source(File::from_str(&tool_djls_string, FileFormat::Toml));
            }
        }

        builder = builder.add_source(
            File::from(project_root.join(".djls.toml"))
                .format(FileFormat::Toml)
                .required(false),
        );

        builder = builder.add_source(
            File::from(project_root.join("djls.toml"))
                .format(FileFormat::Toml)
                .required(false),
        );

        let config = builder.build()?;
        let settings = config.try_deserialize()?;
        Ok(settings)
    }

    #[must_use]
    pub fn debug(&self) -> bool {
        self.debug
    }

    #[must_use]
    pub fn venv_path(&self) -> Option<&str> {
        self.venv_path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    mod defaults {
        use super::*;

        #[test]
        fn test_load_no_files() {
            let dir = tempdir().unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            // Add assertions for future default fields here
            assert_eq!(
                settings,
                Settings {
                    debug: false,
                    venv_path: None
                }
            );
        }
    }

    mod project_files {
        use super::*;

        #[test]
        fn test_load_djls_toml_only() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("djls.toml"), "debug = true").unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_load_venv_path_config() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("djls.toml"), "venv_path = '/path/to/venv'").unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            assert_eq!(
                settings,
                Settings {
                    venv_path: Some("/path/to/venv".to_string()),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_load_dot_djls_toml_only() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join(".djls.toml"), "debug = true").unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_load_pyproject_toml_only() {
            let dir = tempdir().unwrap();
            // Write the setting under [tool.djls]
            let content = "[tool.djls]\ndebug = true\n";
            fs::write(dir.path().join("pyproject.toml"), content).unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }
    }

    mod priority {
        use super::*;

        #[test]
        fn test_project_priority_djls_overrides_dot_djls() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join(".djls.toml"), "debug = false").unwrap();
            fs::write(dir.path().join("djls.toml"), "debug = true").unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            // djls.toml wins
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_project_priority_dot_djls_overrides_pyproject() {
            let dir = tempdir().unwrap();
            let pyproject_content = "[tool.djls]\ndebug = false\n";
            fs::write(dir.path().join("pyproject.toml"), pyproject_content).unwrap();
            fs::write(dir.path().join(".djls.toml"), "debug = true").unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            // .djls.toml wins
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_project_priority_all_files_djls_wins() {
            let dir = tempdir().unwrap();
            let pyproject_content = "[tool.djls]\ndebug = false\n";
            fs::write(dir.path().join("pyproject.toml"), pyproject_content).unwrap();
            fs::write(dir.path().join(".djls.toml"), "debug = false").unwrap();
            fs::write(dir.path().join("djls.toml"), "debug = true").unwrap();
            let settings = Settings::new(dir.path()).unwrap();
            // djls.toml wins
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_user_priority_project_overrides_user() {
            let user_dir = tempdir().unwrap();
            let project_dir = tempdir().unwrap();
            let user_conf_path = user_dir.path().join("config.toml");
            fs::write(&user_conf_path, "debug = true").unwrap(); // User: true
            let pyproject_content = "[tool.djls]\ndebug = false\n"; // Project: false
            fs::write(project_dir.path().join("pyproject.toml"), pyproject_content).unwrap();

            let settings =
                Settings::load_from_paths(project_dir.path(), Some(&user_conf_path)).unwrap();
            // pyproject.toml overrides user
            assert_eq!(
                settings,
                Settings {
                    debug: false,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_user_priority_djls_overrides_user() {
            let user_dir = tempdir().unwrap();
            let project_dir = tempdir().unwrap();
            let user_conf_path = user_dir.path().join("config.toml");
            fs::write(&user_conf_path, "debug = true").unwrap(); // User: true
            fs::write(project_dir.path().join("djls.toml"), "debug = false").unwrap(); // Project: false

            let settings =
                Settings::load_from_paths(project_dir.path(), Some(&user_conf_path)).unwrap();
            // djls.toml overrides user
            assert_eq!(
                settings,
                Settings {
                    debug: false,
                    ..Default::default()
                }
            );
        }
    }

    mod user_config {
        use super::*;

        #[test]
        fn test_load_user_config_only() {
            let user_dir = tempdir().unwrap();
            let project_dir = tempdir().unwrap(); // Empty project dir
            let user_conf_path = user_dir.path().join("config.toml");
            fs::write(&user_conf_path, "debug = true").unwrap();

            let settings =
                Settings::load_from_paths(project_dir.path(), Some(&user_conf_path)).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_no_user_config_file_present() {
            let user_dir = tempdir().unwrap(); // Exists, but no config.toml inside
            let project_dir = tempdir().unwrap();
            let user_conf_path = user_dir.path().join("config.toml"); // Path exists, file doesn't
            let pyproject_content = "[tool.djls]\ndebug = true\n";
            fs::write(project_dir.path().join("pyproject.toml"), pyproject_content).unwrap();

            // Should load project settings fine, ignoring non-existent user config
            let settings =
                Settings::load_from_paths(project_dir.path(), Some(&user_conf_path)).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_user_config_path_not_provided() {
            // Simulates ProjectDirs::from returning None
            let project_dir = tempdir().unwrap();
            fs::write(project_dir.path().join("djls.toml"), "debug = true").unwrap();

            // Call helper with None for user path
            let settings = Settings::load_from_paths(project_dir.path(), None).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_invalid_toml_content() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("djls.toml"), "debug = not_a_boolean").unwrap();
            // Need to call Settings::new here as load_from_paths doesn't involve ProjectDirs
            let result = Settings::new(dir.path());
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), ConfigError::Config(_)));
        }
    }
}
