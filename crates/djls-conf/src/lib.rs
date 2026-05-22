mod diagnostics;
mod django_environments;
mod format;
mod tagspecs;

use std::fs;
use std::path::Path;

use anyhow::Context;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use config::Config;
use config::File;
use config::FileFormat;
use directories::ProjectDirs;
use serde::Deserialize;
use thiserror::Error;

pub use crate::diagnostics::DiagnosticSeverity;
pub use crate::diagnostics::DiagnosticsConfig;
pub use crate::django_environments::DjangoEnvironmentConfig;
pub use crate::format::FormatBackend;
pub use crate::format::FormatConfig;
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

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ConfigError {
    #[error("failed to read DJLS config")]
    Io(Utf8PathBuf),
    #[error("failed to parse DJLS config")]
    Parse(Utf8PathBuf),
    #[error("DJLS config schema is invalid")]
    Schema(Option<Utf8PathBuf>),
    #[error("DJLS config shape is unsupported")]
    Unsupported(Utf8PathBuf),
}

impl ConfigError {
    #[must_use]
    pub fn source_path(&self) -> Option<&Utf8Path> {
        match self {
            Self::Io(source_path) | Self::Parse(source_path) | Self::Unsupported(source_path) => {
                Some(source_path.as_path())
            }
            Self::Schema(source_path) => source_path.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedSettings {
    settings: Settings,
    error: Option<ConfigError>,
}

impl LoadedSettings {
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    #[must_use]
    pub fn error(&self) -> Option<&ConfigError> {
        self.error.as_ref()
    }
}

#[must_use]
pub fn load_settings_for_root(root: &Utf8Path, overrides: &Settings) -> LoadedSettings {
    match load_project_settings(root) {
        Ok(settings) => LoadedSettings {
            settings: settings.with_overrides(overrides.clone()),
            error: None,
        },
        Err(error) => LoadedSettings {
            settings: overrides.clone(),
            error: Some(error),
        },
    }
}

fn load_project_settings(root: &Utf8Path) -> Result<Settings, ConfigError> {
    let mut builder = Config::builder();
    let mut source_path = None;

    for candidate in project_config_sources(root)? {
        source_path = Some(candidate.path.clone());
        builder = builder.add_source(File::from_str(&candidate.content, FileFormat::Toml));
    }

    deserialize_settings(builder, source_path)
}

struct ConfigSource {
    path: Utf8PathBuf,
    content: String,
}

fn project_config_sources(root: &Utf8Path) -> Result<Vec<ConfigSource>, ConfigError> {
    let mut sources = Vec::new();

    if let Some(source) = pyproject_tool_djls_source(root)? {
        sources.push(source);
    }
    for path in [root.join(".djls.toml"), root.join("djls.toml")] {
        if let Some(source) = toml_file_source(path)? {
            sources.push(source);
        }
    }

    Ok(sources)
}

fn pyproject_tool_djls_source(root: &Utf8Path) -> Result<Option<ConfigSource>, ConfigError> {
    let path = root.join("pyproject.toml");
    if !path.exists() {
        return Ok(None);
    }

    let content = read_config_file(&path)?;
    let toml_str: toml::Value =
        toml::from_str(&content).map_err(|_error| ConfigError::Parse(path.clone()))?;
    let tool_djls_value: Option<&toml::Value> = ["tool", "djls"]
        .iter()
        .try_fold(&toml_str, |val, &key| val.get(key));
    let Some(tool_djls_table) = tool_djls_value.and_then(|value| value.as_table()) else {
        return Ok(None);
    };
    let content = toml::to_string(tool_djls_table)
        .map_err(|_error| ConfigError::Unsupported(path.clone()))?;

    Ok(Some(ConfigSource { path, content }))
}

fn toml_file_source(path: Utf8PathBuf) -> Result<Option<ConfigSource>, ConfigError> {
    if !path.exists() {
        return Ok(None);
    }

    let content = read_config_file(&path)?;
    toml::from_str::<toml::Value>(&content).map_err(|_error| ConfigError::Parse(path.clone()))?;
    Ok(Some(ConfigSource { path, content }))
}

fn read_config_file(path: &Utf8Path) -> Result<String, ConfigError> {
    fs::read_to_string(path).map_err(|_error| ConfigError::Io(path.to_owned()))
}

fn deserialize_settings(
    builder: config::ConfigBuilder<config::builder::DefaultState>,
    source_path: Option<Utf8PathBuf>,
) -> Result<Settings, ConfigError> {
    let config = builder
        .build()
        .map_err(|_error| ConfigError::Schema(source_path.clone()))?;
    config
        .try_deserialize()
        .map_err(|_error| ConfigError::Schema(source_path))
}

#[derive(Debug, Deserialize, Default, PartialEq, Clone)]
pub struct Settings {
    #[serde(default)]
    debug: bool,
    venv_path: Option<String>,
    django_settings_module: Option<String>,
    #[serde(default)]
    django_environments: Vec<DjangoEnvironmentConfig>,
    #[serde(default)]
    pythonpath: Vec<String>,
    env_file: Option<String>,
    #[serde(default)]
    tagspecs: TagSpecDef,
    #[serde(default)]
    diagnostics: DiagnosticsConfig,
    #[serde(default)]
    format: FormatConfig,
}

impl Settings {
    pub fn new(project_root: &Utf8Path, overrides: Option<Settings>) -> Result<Self, ConfigError> {
        let user_config_file =
            project_dirs().map(|proj_dirs| proj_dirs.config_dir().join("djls.toml"));

        let mut settings = Self::load_from_paths(project_root, user_config_file.as_deref())?;

        if let Some(overrides) = overrides {
            settings = settings.with_overrides(overrides);
        }

        Ok(settings)
    }

    #[must_use]
    fn with_overrides(mut self, overrides: Settings) -> Self {
        self.debug = overrides.debug || self.debug;
        self.venv_path = overrides.venv_path.or(self.venv_path);
        self.django_settings_module = overrides
            .django_settings_module
            .or(self.django_settings_module);
        if !overrides.django_environments.is_empty() {
            self.django_environments = overrides.django_environments;
        }
        if !overrides.pythonpath.is_empty() {
            self.pythonpath = overrides.pythonpath;
        }
        self.env_file = overrides.env_file.or(self.env_file);
        if !overrides.tagspecs.libraries.is_empty() {
            self.tagspecs = overrides.tagspecs;
        }
        if overrides.diagnostics != DiagnosticsConfig::default() {
            self.diagnostics = overrides.diagnostics;
        }
        if overrides.format != FormatConfig::default() {
            self.format = overrides.format;
        }
        self
    }

    fn load_from_paths(
        project_root: &Utf8Path,
        user_config_path: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let mut builder = Config::builder();

        if let Some(path) = user_config_path {
            builder = builder.add_source(File::from(path).format(FileFormat::Toml).required(false));
        }

        let mut source_path = None;
        for source in project_config_sources(project_root)? {
            source_path = Some(source.path.clone());
            builder = builder.add_source(File::from_str(&source.content, FileFormat::Toml));
        }

        deserialize_settings(builder, source_path)
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
    pub fn django_environments(&self) -> &[DjangoEnvironmentConfig] {
        &self.django_environments
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

    #[must_use]
    pub fn format(&self) -> &FormatConfig {
        &self.format
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    mod settings_load {
        use super::*;

        #[test]
        fn missing_config_uses_overrides_without_error() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            let overrides = Settings::default();

            let load = load_settings_for_root(root, &overrides);

            assert_eq!(load.settings(), &Settings::default());
            assert_eq!(load.error(), None);
        }

        #[test]
        fn loads_project_config_without_error() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(root.join("djls.toml"), "debug = true").unwrap();

            let load = load_settings_for_root(root, &Settings::default());

            assert!(load.settings().debug());
            assert_eq!(load.error(), None);
        }

        #[test]
        fn unrelated_pyproject_does_not_hide_djls_toml_source() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(root.join("pyproject.toml"), "[project]\nname = 'demo'").unwrap();
            fs::write(root.join("djls.toml"), "debug = true").unwrap();

            let load = load_settings_for_root(root, &Settings::default());

            assert!(load.settings().debug());
            assert_eq!(load.error(), None);
        }

        #[test]
        fn invalid_djls_toml_reports_parse_error_for_that_source() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            let source = root.join("djls.toml");
            fs::write(&source, "debug = [true").unwrap();
            let overrides = Settings {
                debug: true,
                ..Settings::default()
            };

            let load = load_settings_for_root(root, &overrides);

            assert_eq!(load.settings(), &overrides);
            assert!(matches!(
                load.error(),
                Some(ConfigError::Parse(source_path)) if source_path == &source
            ));
        }

        #[test]
        fn client_settings_can_override_successful_root_config() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(root.join("djls.toml"), "debug = false").unwrap();
            let client_settings = Settings {
                debug: true,
                ..Settings::default()
            };

            let load = load_settings_for_root(root, &client_settings);

            assert!(load.settings().debug());
            assert_eq!(load.error(), None);
        }

        #[test]
        fn parse_failure_preserves_source() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            let source = root.join("pyproject.toml");
            fs::write(&source, "not = [valid").unwrap();
            let overrides = Settings {
                debug: true,
                ..Settings::default()
            };

            let load = load_settings_for_root(root, &overrides);

            assert_eq!(load.settings(), &overrides);
            assert!(matches!(
                load.error(),
                Some(ConfigError::Parse(source_path)) if source_path == &source
            ));
        }
    }

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
                    django_environments: vec![],
                    pythonpath: vec![],
                    env_file: None,
                    tagspecs: TagSpecDef::default(),
                    diagnostics: DiagnosticsConfig::default(),
                    format: FormatConfig::default(),
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
        fn test_load_env_file_config() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("djls.toml"), r#"env_file = ".env.local""#).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            assert_eq!(
                settings,
                Settings {
                    env_file: Some(".env.local".to_string()),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn test_load_django_environments_config() {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("djls.toml"),
                r#"
[[django_environments]]
root = "projects/site1"
django_settings_module = "projects.site1.settings.dev"

[[django_environments]]
root = "projects/site2"
django_settings_module = "projects.site2.settings.dev"
"#,
            )
            .unwrap();

            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
            let environments = settings.django_environments();

            assert_eq!(environments.len(), 2);
            assert_eq!(environments[0].root(), "projects/site1");
            assert_eq!(
                environments[0].django_settings_module(),
                Some("projects.site1.settings.dev")
            );
            assert_eq!(environments[1].root(), "projects/site2");
            assert_eq!(
                environments[1].django_settings_module(),
                Some("projects.site2.settings.dev")
            );
        }

        #[test]
        fn test_overrides_replace_django_environments() {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("djls.toml"),
                r#"
[[django_environments]]
root = "."
django_settings_module = "project.settings"
"#,
            )
            .unwrap();

            let override_settings = Settings {
                django_environments: vec![DjangoEnvironmentConfig::new(
                    "override",
                    Some("override.settings".to_string()),
                )],
                ..Default::default()
            };
            let settings = Settings::new(
                Utf8Path::from_path(dir.path()).unwrap(),
                Some(override_settings),
            )
            .unwrap();

            assert_eq!(settings.django_environments().len(), 1);
            assert_eq!(settings.django_environments()[0].root(), "override");
            assert_eq!(
                settings.django_environments()[0].django_settings_module(),
                Some("override.settings")
            );
        }

        #[test]
        fn test_load_format_config() {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("djls.toml"),
                r#"
[format]
enabled = true
backend = "djangofmt"
"#,
            )
            .unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            assert!(settings.format().enabled());
            assert_eq!(settings.format().backend(), FormatBackend::Djangofmt);
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
    }

    mod errors {
        use super::*;

        #[test]
        fn test_rejects_legacy_tagspecs_v040_array_format() {
            let dir = tempdir().unwrap();
            let source = Utf8PathBuf::from_path_buf(dir.path().join("djls.toml")).unwrap();

            fs::write(
                &source,
                r#"
[[tagspecs]]
name = "block"
module = "django.template.loader_tags"
end_tag = { name = "endblock", optional = false }
"#,
            )
            .unwrap();

            let result = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                ConfigError::Schema(Some(source_path)) if source_path == source
            ));
        }

        #[test]
        fn test_invalid_toml_content() {
            let dir = tempdir().unwrap();
            let source = Utf8PathBuf::from_path_buf(dir.path().join("djls.toml")).unwrap();
            fs::write(&source, "debug = not_a_boolean").unwrap();
            let result = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None);
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                ConfigError::Parse(source_path) if source_path == source
            ));
        }

        #[test]
        fn test_allows_incomplete_django_environment() {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("djls.toml"),
                r#"
[[django_environments]]
root = "site"
"#,
            )
            .unwrap();

            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            assert_eq!(settings.django_environments().len(), 1);
            assert_eq!(settings.django_environments()[0].root(), "site");
            assert_eq!(
                settings.django_environments()[0].django_settings_module(),
                None
            );
        }
    }
}
