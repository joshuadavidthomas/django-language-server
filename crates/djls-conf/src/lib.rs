mod diagnostics;
mod django_environments;
mod format;
mod tagspecs;

use std::fs;

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
pub enum SettingsLoadError {
    #[error("failed to read DJLS config")]
    Io(Utf8PathBuf),
    #[error("failed to parse DJLS config")]
    Parse(Utf8PathBuf),
    #[error("DJLS config schema is invalid")]
    Schema(Option<Utf8PathBuf>),
    #[error("DJLS config shape is unsupported")]
    Unsupported(Utf8PathBuf),
}

impl SettingsLoadError {
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

trait ConfigErrorExt {
    fn source_path(&self) -> Option<Utf8PathBuf>;
    fn to_settings_load_error(&self) -> SettingsLoadError;
}

impl ConfigErrorExt for config::ConfigError {
    fn source_path(&self) -> Option<Utf8PathBuf> {
        match self {
            Self::FileParse { uri, .. } | Self::Type { origin: uri, .. } => {
                uri.as_deref().map(config_origin_path)
            }
            Self::At { origin, error, .. } => origin
                .as_deref()
                .map(config_origin_path)
                .or_else(|| error.source_path()),
            _ => None,
        }
    }

    fn to_settings_load_error(&self) -> SettingsLoadError {
        match self {
            Self::FileParse { uri: Some(uri), .. } => {
                SettingsLoadError::Parse(config_origin_path(uri))
            }
            _ => SettingsLoadError::Schema(self.source_path()),
        }
    }
}

fn config_origin_path(uri: &str) -> Utf8PathBuf {
    let path = Utf8PathBuf::from(uri);
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .ok()
        .and_then(|cwd| Utf8PathBuf::from_path_buf(cwd).ok())
        .map_or_else(
            || Utf8PathBuf::from(uri),
            |cwd| {
                let path = cwd.join(path);
                path.canonicalize_utf8().unwrap_or(path)
            },
        )
}

enum ConfigLayer {
    File(Utf8PathBuf),
    TomlString(String),
}

impl ConfigLayer {
    fn add_to(
        self,
        builder: config::ConfigBuilder<config::builder::DefaultState>,
    ) -> config::ConfigBuilder<config::builder::DefaultState> {
        match self {
            Self::File(path) => builder.add_source(
                File::from(path.as_std_path())
                    .format(FileFormat::Toml)
                    .required(true),
            ),
            Self::TomlString(content) => {
                builder.add_source(File::from_str(&content, FileFormat::Toml))
            }
        }
    }
}

enum ConfigSource {
    User(Utf8PathBuf),
    PyprojectToolDjls(Utf8PathBuf),
    DotDjls(Utf8PathBuf),
    Djls(Utf8PathBuf),
}

impl ConfigSource {
    fn sources(project_root: &Utf8Path) -> impl Iterator<Item = Self> {
        let user_source = project_dirs()
            .map(|proj_dirs| proj_dirs.config_dir().join("djls.toml"))
            .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
            .map(Self::User);
        let project_sources = [
            Self::PyprojectToolDjls(project_root.join("pyproject.toml")),
            Self::DotDjls(project_root.join(".djls.toml")),
            Self::Djls(project_root.join("djls.toml")),
        ];

        user_source.into_iter().chain(project_sources)
    }

    fn path(&self) -> &Utf8Path {
        match self {
            Self::User(path)
            | Self::PyprojectToolDjls(path)
            | Self::DotDjls(path)
            | Self::Djls(path) => path,
        }
    }

    fn layer(&self) -> Result<Option<ConfigLayer>, SettingsLoadError> {
        let path = self.path();
        if !path.exists() {
            return Ok(None);
        }

        let content =
            fs::read_to_string(path).map_err(|_error| SettingsLoadError::Io(path.to_owned()))?;

        match self {
            Self::User(_) | Self::DotDjls(_) | Self::Djls(_) => {
                toml::from_str::<toml::Value>(&content)
                    .map_err(|_error| SettingsLoadError::Parse(path.to_owned()))?;
                Ok(Some(ConfigLayer::File(path.to_owned())))
            }
            Self::PyprojectToolDjls(_) => {
                let toml_str: toml::Value = toml::from_str(&content)
                    .map_err(|_error| SettingsLoadError::Parse(path.to_owned()))?;
                let tool_djls_value: Option<&toml::Value> = ["tool", "djls"]
                    .iter()
                    .try_fold(&toml_str, |val, &key| val.get(key));
                let Some(tool_djls_table) = tool_djls_value.and_then(|value| value.as_table())
                else {
                    return Ok(None);
                };
                let content = toml::to_string(tool_djls_table)
                    .map_err(|_error| SettingsLoadError::Unsupported(path.to_owned()))?;
                Ok(Some(ConfigLayer::TomlString(content)))
            }
        }
    }
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
    pub fn load(
        project_root: &Utf8Path,
        overrides: Option<Settings>,
    ) -> Result<Self, Vec<SettingsLoadError>> {
        let (loaded_layers, errors) = ConfigSource::sources(project_root).fold(
            (Vec::new(), Vec::new()),
            |(mut loaded_layers, mut errors), source| {
                match source.layer() {
                    Ok(Some(layer)) => loaded_layers.push(layer),
                    Ok(None) => {}
                    Err(error) => errors.push(error),
                }
                (loaded_layers, errors)
            },
        );
        if !errors.is_empty() {
            return Err(errors);
        }
        let builder = loaded_layers
            .into_iter()
            .fold(Config::builder(), |builder, layer| layer.add_to(builder));

        let config = builder
            .build()
            .map_err(|error| vec![error.to_settings_load_error()])?;
        let mut settings: Self = config
            .try_deserialize()
            .map_err(|error| vec![SettingsLoadError::Schema(error.source_path())])?;

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

            let settings = Settings::load(root, Some(overrides.clone())).unwrap();

            assert_eq!(settings, overrides);
        }

        #[test]
        fn loads_project_config_without_error() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(root.join("djls.toml"), "debug = true").unwrap();

            let settings = Settings::load(root, None).unwrap();

            assert!(settings.debug());
        }

        #[test]
        fn unrelated_pyproject_does_not_hide_djls_toml_source() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(root.join("pyproject.toml"), "[project]\nname = 'demo'").unwrap();
            fs::write(root.join("djls.toml"), "debug = true").unwrap();

            let settings = Settings::load(root, None).unwrap();

            assert!(settings.debug());
        }

        #[test]
        fn invalid_djls_toml_reports_parse_error_for_that_source() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            let source = root.join("djls.toml");
            fs::write(&source, "debug = [true").unwrap();

            let errors = Settings::load(root, None).unwrap_err();

            assert_eq!(errors.len(), 1);
            assert!(matches!(
                &errors[0],
                SettingsLoadError::Parse(source_path) if source_path == &source
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

            let settings = Settings::load(root, Some(client_settings)).unwrap();

            assert!(settings.debug());
        }

        #[test]
        fn parse_failure_preserves_source() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            let source = root.join("pyproject.toml");
            fs::write(&source, "not = [valid").unwrap();

            let errors = Settings::load(root, None).unwrap_err();

            assert_eq!(errors.len(), 1);
            assert!(matches!(
                &errors[0],
                SettingsLoadError::Parse(source_path) if source_path == &source
            ));
        }

        #[test]
        fn reports_parse_errors_for_each_invalid_source() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            let pyproject = root.join("pyproject.toml");
            let djls = root.join("djls.toml");
            fs::write(&pyproject, "not = [valid").unwrap();
            fs::write(&djls, "debug = [true").unwrap();

            let errors = Settings::load(root, None).unwrap_err();

            assert_eq!(errors.len(), 2);
            assert!(matches!(
                &errors[0],
                SettingsLoadError::Parse(source_path) if source_path == &pyproject
            ));
            assert!(matches!(
                &errors[1],
                SettingsLoadError::Parse(source_path) if source_path == &djls
            ));
        }
    }

    mod defaults {
        use super::*;

        #[test]
        fn test_load_no_files() {
            let dir = tempdir().unwrap();
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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

            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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

            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let raw_source = dir.path().join("djls.toml");

            fs::write(
                &raw_source,
                r#"
[[tagspecs]]
name = "block"
module = "django.template.loader_tags"
end_tag = { name = "endblock", optional = false }
"#,
            )
            .unwrap();
            let source = Utf8PathBuf::from_path_buf(fs::canonicalize(raw_source).unwrap()).unwrap();

            let result = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None);

            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert_eq!(errors.len(), 1);
            assert!(
                matches!(
                    &errors[0],
                    SettingsLoadError::Schema(Some(source_path)) if source_path == &source
                ),
                "unexpected errors: {errors:?}"
            );
        }

        #[test]
        fn test_invalid_toml_content() {
            let dir = tempdir().unwrap();
            let source = Utf8PathBuf::from_path_buf(dir.path().join("djls.toml")).unwrap();
            fs::write(&source, "debug = not_a_boolean").unwrap();
            let result = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None);
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert_eq!(errors.len(), 1);
            assert!(matches!(
                &errors[0],
                SettingsLoadError::Parse(source_path) if source_path == &source
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

            let settings = Settings::load(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            assert_eq!(settings.django_environments().len(), 1);
            assert_eq!(settings.django_environments()[0].root(), "site");
            assert_eq!(
                settings.django_environments()[0].django_settings_module(),
                None
            );
        }
    }
}
