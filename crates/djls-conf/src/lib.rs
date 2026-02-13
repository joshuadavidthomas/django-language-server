pub mod diagnostics;
pub mod tagspecs;

use std::fs;
use std::path::Path;

use anyhow::Context;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use config::Config;
use config::ConfigError as ExternalConfigError;
use config::File;
use config::FileFormat;
use directories::ProjectDirs;
use serde::Deserialize;
use serde::Deserializer;
use thiserror::Error;

pub use crate::diagnostics::DiagnosticSeverity;
pub use crate::diagnostics::DiagnosticsConfig;
pub use crate::tagspecs::ArgKindDef;
pub use crate::tagspecs::ArgTypeDef;
pub use crate::tagspecs::EndTagDef;
pub use crate::tagspecs::IntermediateTagDef;
pub use crate::tagspecs::PositionDef;
pub use crate::tagspecs::TagArgDef;
pub use crate::tagspecs::TagDef;
pub use crate::tagspecs::TagLibraryDef;
pub use crate::tagspecs::TagSpecDef;
pub use crate::tagspecs::TagTypeDef;

#[must_use]
pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "djls")
}

/// Get the log directory for the application and ensure it exists.
///
/// Returns the XDG cache directory (e.g., ~/.cache/djls on Linux) if available,
/// otherwise falls back to /tmp. Creates the directory if it doesn't exist.
///
/// # Errors
///
/// Returns an error if the directory cannot be created.
pub fn log_dir() -> anyhow::Result<Utf8PathBuf> {
    let dir = project_dirs()
        .and_then(|proj_dirs| Utf8PathBuf::from_path_buf(proj_dirs.cache_dir().to_path_buf()).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("/tmp"));

    fs::create_dir_all(&dir).with_context(|| format!("Failed to create log directory: {dir}"))?;

    Ok(dir)
}

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

#[derive(Debug, Deserialize, Default, PartialEq, Clone)]
pub struct Settings {
    #[serde(default)]
    debug: bool,
    venv_path: Option<String>,
    django_settings_module: Option<String>,
    #[serde(default)]
    pythonpath: Vec<String>,
    env_file: Option<String>,
    #[serde(default, deserialize_with = "deserialize_tagspecs")]
    tagspecs: TagSpecDef,
    #[serde(default)]
    diagnostics: DiagnosticsConfig,
}

fn deserialize_tagspecs<'de, D>(deserializer: D) -> Result<TagSpecDef, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    use serde_json::Value;

    let value = Value::deserialize(deserializer)?;

    if let Ok(new_format) = TagSpecDef::deserialize(&value) {
        return Ok(new_format);
    }

    #[allow(deprecated)]
    if let Ok(legacy) = Vec::<tagspecs::legacy::LegacyTagSpecDef>::deserialize(&value) {
        tracing::warn!(concat!(
            "DEPRECATED: TagSpecs v0.4.0 format detected. Please migrate to v0.6.0 format. ",
            "The old format will be removed in v6.0.3. ",
            "See migration guide: https://djls.joshthomas.dev/configuration/tagspecs/#migration-from-v040",
        ));
        #[allow(deprecated)]
        return Ok(tagspecs::legacy::convert_legacy_tagspecs(legacy));
    }

    Err(D::Error::custom(
        "Invalid tagspecs format. Expected v0.6.0 hierarchical format or legacy v0.4.0 array format",
    ))
}

impl Settings {
    pub fn new(project_root: &Utf8Path, overrides: Option<Settings>) -> Result<Self, ConfigError> {
        let user_config_file =
            project_dirs().map(|proj_dirs| proj_dirs.config_dir().join("djls.toml"));

        let mut settings = Self::load_from_paths(project_root, user_config_file.as_deref())?;

        if let Some(overrides) = overrides {
            settings.debug = overrides.debug || settings.debug;
            settings.venv_path = overrides.venv_path.or(settings.venv_path);
            settings.django_settings_module = overrides
                .django_settings_module
                .or(settings.django_settings_module);
            if !overrides.pythonpath.is_empty() {
                settings.pythonpath = overrides.pythonpath;
            }
            settings.env_file = overrides.env_file.or(settings.env_file);
            if !overrides.tagspecs.libraries.is_empty() {
                settings.tagspecs = overrides.tagspecs;
            }
            // For diagnostics, override if the config is non-default
            if overrides.diagnostics != DiagnosticsConfig::default() {
                settings.diagnostics = overrides.diagnostics;
            }
        }

        Ok(settings)
    }

    fn load_from_paths(
        project_root: &Utf8Path,
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
            File::from(project_root.join(".djls.toml").as_std_path())
                .format(FileFormat::Toml)
                .required(false),
        );

        builder = builder.add_source(
            File::from(project_root.join("djls.toml").as_std_path())
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

    #[must_use]
    pub fn django_settings_module(&self) -> Option<&str> {
        self.django_settings_module.as_deref()
    }

    #[must_use]
    pub fn pythonpath(&self) -> &[String] {
        &self.pythonpath
    }

    #[must_use]
    pub fn env_file(&self) -> Option<&str> {
        self.env_file.as_deref()
    }

    #[must_use]
    pub fn tagspecs(&self) -> &TagSpecDef {
        &self.tagspecs
    }

    #[must_use]
    pub fn diagnostics(&self) -> &DiagnosticsConfig {
        &self.diagnostics
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: false,
                    venv_path: None,
                    django_settings_module: None,
                    pythonpath: vec![],
                    env_file: None,
                    tagspecs: TagSpecDef::default(),
                    diagnostics: DiagnosticsConfig::default(),
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            assert_eq!(
                settings,
                Settings {
                    venv_path: Some("/path/to/venv".to_string()),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_load_pythonpath_config() {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("djls.toml"),
                r#"pythonpath = ["/path/to/lib", "/another/path"]"#,
            )
            .unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            assert_eq!(
                settings,
                Settings {
                    pythonpath: vec!["/path/to/lib".to_string(), "/another/path".to_string()],
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_load_dot_djls_toml_only() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join(".djls.toml"), "debug = true").unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let content = "[tool.djls]\ndebug = true\n";
            fs::write(dir.path().join("pyproject.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_load_diagnostics_config() {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("djls.toml"),
                r#"
[diagnostics.severity]
S100 = "off"
S101 = "warning"
"T" = "off"
T100 = "hint"
"#,
            )
            .unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            assert_eq!(
                settings.diagnostics.get_severity("S100"),
                DiagnosticSeverity::Off
            );
            assert_eq!(
                settings.diagnostics.get_severity("S101"),
                DiagnosticSeverity::Warning
            );
            assert_eq!(
                settings.diagnostics.get_severity("T900"),
                DiagnosticSeverity::Off
            );
            assert_eq!(
                settings.diagnostics.get_severity("T100"),
                DiagnosticSeverity::Hint
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            fs::write(&user_conf_path, "debug = true").unwrap();
            let pyproject_content = "[tool.djls]\ndebug = false\n";
            fs::write(project_dir.path().join("pyproject.toml"), pyproject_content).unwrap();

            let settings = Settings::load_from_paths(
                Utf8Path::from_path(project_dir.path()).unwrap(),
                Some(&user_conf_path),
            )
            .unwrap();
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
            fs::write(&user_conf_path, "debug = true").unwrap();
            fs::write(project_dir.path().join("djls.toml"), "debug = false").unwrap();

            let settings = Settings::load_from_paths(
                Utf8Path::from_path(project_dir.path()).unwrap(),
                Some(&user_conf_path),
            )
            .unwrap();
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
            let project_dir = tempdir().unwrap();
            let user_conf_path = user_dir.path().join("config.toml");
            fs::write(&user_conf_path, "debug = true").unwrap();

            let settings = Settings::load_from_paths(
                Utf8Path::from_path(project_dir.path()).unwrap(),
                Some(&user_conf_path),
            )
            .unwrap();
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
            let user_dir = tempdir().unwrap();
            let project_dir = tempdir().unwrap();
            let user_conf_path = user_dir.path().join("config.toml");
            let pyproject_content = "[tool.djls]\ndebug = true\n";
            fs::write(project_dir.path().join("pyproject.toml"), pyproject_content).unwrap();

            let settings = Settings::load_from_paths(
                Utf8Path::from_path(project_dir.path()).unwrap(),
                Some(&user_conf_path),
            )
            .unwrap();
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
            let project_dir = tempdir().unwrap();
            fs::write(project_dir.path().join("djls.toml"), "debug = true").unwrap();

            let settings =
                Settings::load_from_paths(Utf8Path::from_path(project_dir.path()).unwrap(), None)
                    .unwrap();
            assert_eq!(
                settings,
                Settings {
                    debug: true,
                    ..Default::default()
                }
            );
        }
    }

    mod tagspecs {
        use super::*;

        #[test]
        fn test_load_tagspecs_v060_from_djls_toml() {
            let dir = tempdir().unwrap();

            fs::write(
                dir.path().join("djls.toml"),
                r#"
[tagspecs]
version = "0.6.0"

[[tagspecs.libraries]]
module = "myapp.templatetags.custom"

[[tagspecs.libraries.tags]]
name = "switch"
type = "block"

[tagspecs.libraries.tags.end]
name = "endswitch"

[[tagspecs.libraries.tags.intermediates]]
name = "case"

[[tagspecs.libraries.tags.args]]
name = "value"
kind = "variable"
"#,
            )
            .unwrap();

            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            let doc = settings.tagspecs();

            assert_eq!(doc.version, "0.6.0");
            assert_eq!(doc.libraries.len(), 1);
            assert_eq!(doc.libraries[0].module, "myapp.templatetags.custom");
            assert_eq!(doc.libraries[0].tags.len(), 1);
            assert_eq!(doc.libraries[0].tags[0].name, "switch");
        }

        #[test]
        fn test_load_legacy_tagspecs_v040_array_format() {
            let dir = tempdir().unwrap();

            fs::write(
                dir.path().join("djls.toml"),
                r#"
[[tagspecs]]
name = "block"
module = "django.template.loader_tags"
end_tag = { name = "endblock", optional = false }
"#,
            )
            .unwrap();

            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            let doc = settings.tagspecs();

            assert_eq!(doc.version, "0.6.0");
            assert_eq!(doc.libraries.len(), 1);
            assert_eq!(doc.libraries[0].module, "django.template.loader_tags");
            assert_eq!(doc.libraries[0].tags.len(), 1);
            assert_eq!(doc.libraries[0].tags[0].name, "block");
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_invalid_toml_content() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("djls.toml"), "debug = not_a_boolean").unwrap();
            let result = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None);
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), ConfigError::Config(_)));
        }
    }
}
