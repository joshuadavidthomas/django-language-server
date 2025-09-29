pub mod tagspecs;

use std::fs;
use std::path::Path;

use camino::Utf8Path;
use config::Config;
use config::ConfigError as ExternalConfigError;
use config::File;
use config::FileFormat;
use directories::ProjectDirs;
use serde::Deserialize;
use thiserror::Error;

pub use crate::tagspecs::ArgTypeDef;
pub use crate::tagspecs::EndTagDef;
pub use crate::tagspecs::IntermediateTagDef;
pub use crate::tagspecs::SimpleArgTypeDef;
pub use crate::tagspecs::TagArgDef;
pub use crate::tagspecs::TagSpecDef;

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
    tagspecs: Vec<TagSpecDef>,
}

impl Settings {
    pub fn new(project_root: &Utf8Path) -> Result<Self, ConfigError> {
        let user_config_file = ProjectDirs::from("com.github", "joshuadavidthomas", "djls")
            .map(|proj_dirs| proj_dirs.config_dir().join("djls.toml"));

        Self::load_from_paths(project_root, user_config_file.as_deref())
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
    pub fn tagspecs(&self) -> &[TagSpecDef] {
        &self.tagspecs
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
            // Add assertions for future default fields here
            assert_eq!(
                settings,
                Settings {
                    debug: false,
                    venv_path: None,
                    django_settings_module: None,
                    tagspecs: vec![],
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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
            let result = Settings::new(Utf8Path::from_path(dir.path()).unwrap());
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), ConfigError::Config(_)));
        }
    }

    mod tagspecs {
        use super::*;
        use crate::tagspecs::ArgTypeDef;
        use crate::tagspecs::SimpleArgTypeDef;

        #[test]
        fn test_load_tagspecs_from_djls_toml() {
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
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();

            assert_eq!(settings.tagspecs().len(), 2);

            let mytag = &settings.tagspecs()[0];
            assert_eq!(mytag.name, "mytag");
            assert_eq!(mytag.module, "myapp.templatetags.custom");
            assert_eq!(mytag.end_tag.as_ref().unwrap().name, "endmytag");

            let for_tag = &settings.tagspecs()[1];
            assert_eq!(for_tag.name, "for");
            assert_eq!(for_tag.module, "django.template.defaulttags");
            assert_eq!(for_tag.intermediate_tags.len(), 1);
            assert_eq!(for_tag.args.len(), 3);
        }

        #[test]
        fn test_load_tagspecs_from_pyproject() {
            let dir = tempdir().unwrap();
            let content = r#"
[tool.djls]
debug = true

[[tool.djls.tagspecs]]
name = "cache"
module = "django.templatetags.cache"
end_tag = { name = "endcache", optional = false }
args = [
    { name = "expire_time", type = "variable" },
    { name = "fragment_name", type = "string" }
]
"#;
            fs::write(dir.path().join("pyproject.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();

            assert_eq!(settings.tagspecs().len(), 1);
            let cache = &settings.tagspecs()[0];
            assert_eq!(cache.name, "cache");
            assert_eq!(cache.module, "django.templatetags.cache");
            assert_eq!(cache.args.len(), 2);
        }

        #[test]
        fn test_arg_types() {
            let dir = tempdir().unwrap();
            let content = r#"
[[tagspecs]]
name = "test"
module = "test.module"
args = [
    { name = "simple", type = "variable" },
    { name = "choice", type = { choice = ["on", "off"] } },
    { name = "optional", required = false, type = "string" }
]
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();

            let test = &settings.tagspecs()[0];
            assert_eq!(test.args.len(), 3);

            // Check simple type
            assert!(matches!(
                test.args[0].arg_type,
                ArgTypeDef::Simple(SimpleArgTypeDef::Variable)
            ));

            // Check choice type
            if let ArgTypeDef::Choice { ref choice } = test.args[1].arg_type {
                assert_eq!(choice, &vec!["on".to_string(), "off".to_string()]);
            } else {
                panic!("Expected choice type");
            }

            // Check optional arg
            assert!(!test.args[2].required);
        }

        #[test]
        fn test_intermediate_tags() {
            let dir = tempdir().unwrap();
            let content = r#"
[[tagspecs]]
name = "if"
module = "django.template.defaulttags"
end_tag = { name = "endif" }
intermediate_tags = [
    { name = "elif" },
    { name = "else" }
]
args = [
    { name = "condition", type = "expression" }
]
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();

            let if_tag = &settings.tagspecs()[0];
            assert_eq!(if_tag.name, "if");

            assert_eq!(if_tag.intermediate_tags.len(), 2);
            assert_eq!(if_tag.intermediate_tags[0].name, "elif");
            assert_eq!(if_tag.intermediate_tags[1].name, "else");
        }

        #[test]
        fn test_end_tag_with_args() {
            let dir = tempdir().unwrap();
            let content = r#"
[[tagspecs]]
name = "block"
module = "django.template.defaulttags"
end_tag = { name = "endblock", args = [{ name = "name", required = false, type = "variable" }] }
args = [
    { name = "name", type = "variable" }
]
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();

            let block_tag = &settings.tagspecs()[0];
            assert_eq!(block_tag.name, "block");

            let end_tag = block_tag.end_tag.as_ref().unwrap();
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

[[tagspecs]]
name = "custom"
module = "myapp.tags"
args = []
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();

            assert!(settings.debug());
            assert_eq!(settings.venv_path(), Some("/path/to/venv"));
            assert_eq!(settings.tagspecs().len(), 1);
            assert_eq!(settings.tagspecs()[0].name, "custom");
        }

        #[test]
        fn test_all_arg_types() {
            let dir = tempdir().unwrap();
            let content = r#"
[[tagspecs]]
name = "test_all_types"
module = "test.module"
args = [
    { name = "literal", type = "literal" },
    { name = "variable", type = "variable" },
    { name = "string", type = "string" },
    { name = "expression", type = "expression" },
    { name = "assignment", type = "assignment" },
    { name = "varargs", type = "varargs" }
]
"#;
            fs::write(dir.path().join("djls.toml"), content).unwrap();
            let settings = Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();

            let test = &settings.tagspecs()[0];
            assert_eq!(test.args.len(), 6);

            assert!(matches!(
                test.args[0].arg_type,
                ArgTypeDef::Simple(SimpleArgTypeDef::Literal)
            ));
            assert!(matches!(
                test.args[1].arg_type,
                ArgTypeDef::Simple(SimpleArgTypeDef::Variable)
            ));
            assert!(matches!(
                test.args[2].arg_type,
                ArgTypeDef::Simple(SimpleArgTypeDef::String)
            ));
            assert!(matches!(
                test.args[3].arg_type,
                ArgTypeDef::Simple(SimpleArgTypeDef::Expression)
            ));
            assert!(matches!(
                test.args[4].arg_type,
                ArgTypeDef::Simple(SimpleArgTypeDef::Assignment)
            ));
            assert!(matches!(
                test.args[5].arg_type,
                ArgTypeDef::Simple(SimpleArgTypeDef::VarArgs)
            ));
        }
    }
}
