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

pub(crate) fn project_dirs() -> Option<ProjectDirs> {
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
    #[serde(default, deserialize_with = "deserialize_tagspecs")]
    tagspecs: TagSpecDef,
    #[serde(default)]
    diagnostics: DiagnosticsConfig,
}

// DEPRECATION: Remove in v6.2.0
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

    if let Ok(legacy) = Vec::<tagspecs::legacy::LegacyTagSpecDef>::deserialize(&value) {
        tracing::warn!(concat!(
            "DEPRECATED: TagSpecs v0.4.0 format detected. Please migrate to v0.6.0 format. ",
            "The old format will be removed in v6.2.0. ",
            "See migration guide: https://djls.joshthomas.dev/tagspecs/#migration-from-v040",
        ));
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
            // Add assertions for future default fields here
            assert_eq!(
                settings,
                Settings {
                    debug: false,
                    venv_path: None,
                    django_settings_module: None,
                    pythonpath: vec![],
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
            // Write the setting under [tool.djls]
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
            // Test via public API
            assert_eq!(
                settings.diagnostics.get_severity("S100"),
                DiagnosticSeverity::Off
            );
            assert_eq!(
                settings.diagnostics.get_severity("S101"),
                DiagnosticSeverity::Warning
            );
            // T prefix applies to T900
            assert_eq!(
                settings.diagnostics.get_severity("T900"),
                DiagnosticSeverity::Off
            );
            // T100 has specific override
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();
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

            let settings = Settings::load_from_paths(
                Utf8Path::from_path(project_dir.path()).unwrap(),
                Some(&user_conf_path),
            )
            .unwrap();
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

            let settings = Settings::load_from_paths(
                Utf8Path::from_path(project_dir.path()).unwrap(),
                Some(&user_conf_path),
            )
            .unwrap();
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
            let user_dir = tempdir().unwrap(); // Exists, but no config.toml inside
            let project_dir = tempdir().unwrap();
            let user_conf_path = user_dir.path().join("config.toml"); // Path exists, file doesn't
            let pyproject_content = "[tool.djls]\ndebug = true\n";
            fs::write(project_dir.path().join("pyproject.toml"), pyproject_content).unwrap();

            // Should load project settings fine, ignoring non-existent user config
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
            // Simulates ProjectDirs::from returning None
            let project_dir = tempdir().unwrap();
            fs::write(project_dir.path().join("djls.toml"), "debug = true").unwrap();

            // Call helper with None for user path
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

    mod errors {
        use super::*;

        #[test]
        fn test_invalid_toml_content() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("djls.toml"), "debug = not_a_boolean").unwrap();
            // Need to call Settings::new here as load_from_paths doesn't involve ProjectDirs
            let result = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None);
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), ConfigError::Config(_)));
        }
    }

    mod tagspecs {
        use super::*;
        use crate::tagspecs::ArgKindDef;

        #[test]
        fn test_load_tagspecs_from_djls_toml() {
            let dir = tempdir().unwrap();
            let content = r#"
[tagspecs]
version = "0.6.0"

[[tagspecs.libraries]]
module = "myapp.templatetags.custom"

[[tagspecs.libraries.tags]]
name = "mytag"
type = "block"

[tagspecs.libraries.tags.end]
name = "endmytag"

[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "for"
type = "block"

[tagspecs.libraries.tags.end]
name = "endfor"

[[tagspecs.libraries.tags.intermediates]]
name = "empty"

[[tagspecs.libraries.tags.args]]
name = "item"
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "in"
kind = "literal"

[[tagspecs.libraries.tags.args]]
name = "items"
kind = "variable"
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            assert_eq!(settings.tagspecs().libraries.len(), 2);

            let lib0 = &settings.tagspecs().libraries[0];
            assert_eq!(lib0.module, "myapp.templatetags.custom");
            assert_eq!(lib0.tags.len(), 1);
            let mytag = &lib0.tags[0];
            assert_eq!(mytag.name, "mytag");
            assert_eq!(mytag.end.as_ref().unwrap().name, "endmytag");

            let lib1 = &settings.tagspecs().libraries[1];
            assert_eq!(lib1.module, "django.template.defaulttags");
            assert_eq!(lib1.tags.len(), 1);
            let for_tag = &lib1.tags[0];
            assert_eq!(for_tag.name, "for");
            assert_eq!(for_tag.intermediates.len(), 1);
            assert_eq!(for_tag.args.len(), 3);
        }

        #[test]
        fn test_load_tagspecs_from_pyproject() {
            let dir = tempdir().unwrap();
            let content = r#"
[tool.djls]
debug = true

[tool.djls.tagspecs]
version = "0.6.0"

[[tool.djls.tagspecs.libraries]]
module = "django.templatetags.cache"

[[tool.djls.tagspecs.libraries.tags]]
name = "cache"
type = "block"

[tool.djls.tagspecs.libraries.tags.end]
name = "endcache"
required = true

[[tool.djls.tagspecs.libraries.tags.args]]
name = "expire_time"
kind = "variable"

[[tool.djls.tagspecs.libraries.tags.args]]
name = "fragment_name"
kind = "variable"
"#;
            fs::write(dir.path().join("pyproject.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            assert_eq!(settings.tagspecs().libraries.len(), 1);
            let lib = &settings.tagspecs().libraries[0];
            assert_eq!(lib.module, "django.templatetags.cache");
            assert_eq!(lib.tags.len(), 1);
            let cache = &lib.tags[0];
            assert_eq!(cache.name, "cache");
            assert_eq!(cache.args.len(), 2);
        }

        #[test]
        fn test_arg_types() {
            let dir = tempdir().unwrap();
            let content = r#"
[[tagspecs.libraries]]
module = "test.module"

[[tagspecs.libraries.tags]]
name = "test"
type = "standalone"

[[tagspecs.libraries.tags.args]]
name = "simple"
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "choice"
kind = "choice"

[tagspecs.libraries.tags.args.extra]
choices = ["on", "off"]

[[tagspecs.libraries.tags.args]]
name = "optional"
required = false
kind = "variable"
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            let lib = &settings.tagspecs().libraries[0];
            let test = &lib.tags[0];
            assert_eq!(test.args.len(), 3);

            // Check simple kind
            assert!(matches!(test.args[0].kind, ArgKindDef::Variable));

            // Check choice kind
            assert!(matches!(test.args[1].kind, ArgKindDef::Choice));

            // Check optional arg
            assert!(!test.args[2].required);
        }

        #[test]
        fn test_intermediate_tags() {
            let dir = tempdir().unwrap();
            let content = r#"
[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "if"
type = "block"

[tagspecs.libraries.tags.end]
name = "endif"

[[tagspecs.libraries.tags.intermediates]]
name = "elif"

[[tagspecs.libraries.tags.intermediates]]
name = "else"

[[tagspecs.libraries.tags.args]]
name = "condition"
kind = "any"
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            let lib = &settings.tagspecs().libraries[0];
            let if_tag = &lib.tags[0];
            assert_eq!(if_tag.name, "if");

            assert_eq!(if_tag.intermediates.len(), 2);
            assert_eq!(if_tag.intermediates[0].name, "elif");
            assert_eq!(if_tag.intermediates[1].name, "else");
        }

        #[test]
        fn test_end_tag_with_args() {
            let dir = tempdir().unwrap();
            let content = r#"
[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "block"
type = "block"

[tagspecs.libraries.tags.end]
name = "endblock"

[[tagspecs.libraries.tags.end.args]]
name = "name"
required = false
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "name"
kind = "variable"
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            let lib = &settings.tagspecs().libraries[0];
            let block_tag = &lib.tags[0];
            assert_eq!(block_tag.name, "block");

            let end_tag = block_tag.end.as_ref().unwrap();
            assert_eq!(end_tag.name, "endblock");
            assert_eq!(end_tag.args.len(), 1);
            assert!(!end_tag.args[0].required);
        }

        #[test]
        fn test_tagspecs_with_other_settings() {
            let dir = tempdir().unwrap();
            let content = r#"
debug = true
venv_path = "/path/to/venv"

[tagspecs]

[[tagspecs.libraries]]
module = "myapp.tags"

[[tagspecs.libraries.tags]]
name = "custom"
type = "standalone"
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            assert_eq!(settings.tagspecs().libraries.len(), 1);
            assert_eq!(settings.tagspecs().libraries[0].tags[0].name, "custom");
            assert_eq!(settings.venv_path(), Some("/path/to/venv"));
            assert!(settings.debug());
        }

        #[test]
        fn test_all_arg_kinds() {
            let dir = tempdir().unwrap();
            let content = r#"
[tagspecs]

[[tagspecs.libraries]]
module = "test.module"

[[tagspecs.libraries.tags]]
name = "test_all_kinds"
type = "standalone"

[[tagspecs.libraries.tags.args]]
name = "literal"
kind = "literal"

[[tagspecs.libraries.tags.args]]
name = "variable"
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "any"
kind = "any"

[[tagspecs.libraries.tags.args]]
name = "syntax"
kind = "syntax"

[[tagspecs.libraries.tags.args]]
name = "assignment"
kind = "assignment"

[[tagspecs.libraries.tags.args]]
name = "modifier"
kind = "modifier"

[[tagspecs.libraries.tags.args]]
name = "choice"
kind = "choice"
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

            let lib = &settings.tagspecs().libraries[0];
            let test = &lib.tags[0];
            assert_eq!(test.args.len(), 7);

            assert!(matches!(test.args[0].kind, ArgKindDef::Literal));
            assert!(matches!(test.args[1].kind, ArgKindDef::Variable));
            assert!(matches!(test.args[2].kind, ArgKindDef::Any));
            assert!(matches!(test.args[3].kind, ArgKindDef::Syntax));
            assert!(matches!(test.args[4].kind, ArgKindDef::Assignment));
            assert!(matches!(test.args[5].kind, ArgKindDef::Modifier));
            assert!(matches!(test.args[6].kind, ArgKindDef::Choice));
        }

        // DEPRECATION TESTS: Remove in v6.2.0
        mod legacy_format {
            use super::*;

            #[test]
            fn test_load_legacy_flat_format() {
                let dir = tempdir().unwrap();
                let content = r#"
[[tagspecs]]
name = "mytag"
module = "myapp.templatetags.custom"
end_tag = { name = "endmytag" }

[[tagspecs]]
name = "for"
module = "django.template.defaulttags"
end_tag = { name = "endfor" }
intermediate_tags = [{ name = "empty" }]
args = [
    { name = "item", type = "variable" },
    { name = "in", type = "literal" },
    { name = "items", type = "variable" }
]
"#;
                fs::write(dir.path().join("djls.toml"), content).unwrap();
                let settings =
                    Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

                // Should be converted to new hierarchical format
                assert_eq!(settings.tagspecs().version, "0.6.0");
                assert_eq!(settings.tagspecs().libraries.len(), 2);

                // Find libraries (order not guaranteed)
                let myapp_lib = settings
                    .tagspecs()
                    .libraries
                    .iter()
                    .find(|lib| lib.module == "myapp.templatetags.custom")
                    .unwrap();
                let django_lib = settings
                    .tagspecs()
                    .libraries
                    .iter()
                    .find(|lib| lib.module == "django.template.defaulttags")
                    .unwrap();

                // Verify mytag conversion
                assert_eq!(myapp_lib.tags.len(), 1);
                let mytag = &myapp_lib.tags[0];
                assert_eq!(mytag.name, "mytag");
                assert!(matches!(mytag.tag_type, TagTypeDef::Block));
                assert_eq!(mytag.end.as_ref().unwrap().name, "endmytag");

                // Verify for tag conversion
                assert_eq!(django_lib.tags.len(), 1);
                let for_tag = &django_lib.tags[0];
                assert_eq!(for_tag.name, "for");
                assert_eq!(for_tag.intermediates.len(), 1);
                assert_eq!(for_tag.intermediates[0].name, "empty");
                assert_eq!(for_tag.args.len(), 3);
            }

            #[test]
            fn test_legacy_optional_end_tag_conversion() {
                let dir = tempdir().unwrap();
                let content = r#"
[[tagspecs]]
name = "cache"
module = "django.templatetags.cache"
end_tag = { name = "endcache", optional = false }
args = [
    { name = "expire_time", type = "variable" },
    { name = "fragment_name", type = "string" }
]
"#;
                fs::write(dir.path().join("djls.toml"), content).unwrap();
                let settings =
                    Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

                let lib = &settings.tagspecs().libraries[0];
                let cache = &lib.tags[0];

                // optional: false should convert to required: true
                assert!(cache.end.as_ref().unwrap().required);
            }

            #[test]
            fn test_legacy_choice_arg_conversion() {
                let dir = tempdir().unwrap();
                let content = r#"
[[tagspecs]]
name = "test"
module = "test.module"
args = [
    { name = "choice", type = { choice = ["on", "off"] } }
]
"#;
                fs::write(dir.path().join("djls.toml"), content).unwrap();
                let settings =
                    Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

                let lib = &settings.tagspecs().libraries[0];
                let test = &lib.tags[0];

                assert_eq!(test.args.len(), 1);
                assert!(matches!(test.args[0].kind, ArgKindDef::Choice));

                // Verify choices are in extra metadata
                assert!(test.args[0].extra.is_some());
                let choices = test.args[0].extra.as_ref().unwrap().get("choices");
                assert!(choices.is_some());
            }

            #[test]
            fn test_legacy_multiple_tags_same_module() {
                let dir = tempdir().unwrap();
                let content = r#"
[[tagspecs]]
name = "tag1"
module = "myapp.tags"

[[tagspecs]]
name = "tag2"
module = "myapp.tags"

[[tagspecs]]
name = "tag3"
module = "other.tags"
"#;
                fs::write(dir.path().join("djls.toml"), content).unwrap();
                let settings =
                    Settings::new(Utf8Path::from_path(dir.path()).unwrap(), None).unwrap();

                // Should group tags by module
                assert_eq!(settings.tagspecs().libraries.len(), 2);

                let myapp_lib = settings
                    .tagspecs()
                    .libraries
                    .iter()
                    .find(|lib| lib.module == "myapp.tags")
                    .unwrap();
                assert_eq!(myapp_lib.tags.len(), 2);

                let other_lib = settings
                    .tagspecs()
                    .libraries
                    .iter()
                    .find(|lib| lib.module == "other.tags")
                    .unwrap();
                assert_eq!(other_lib.tags.len(), 1);
            }
        }
    }
}
