use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fs;
use std::ops::{Deref, Index};
use std::path::Path;
use thiserror::Error;
use toml::Value;

#[derive(Debug, Error)]
pub enum TagSpecError {
    #[error("Failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("Failed to extract specs: {0}")]
    Extract(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Clone, Debug, Default)]
pub struct TagSpecs(HashMap<String, TagSpec>);

impl TagSpecs {
    pub fn get(&self, key: &str) -> Option<&TagSpec> {
        self.0.get(key)
    }

    /// Load specs from a TOML file, looking under the specified table path
    fn load_from_toml(path: &Path, table_path: &[&str]) -> Result<Self, anyhow::Error> {
        let content = fs::read_to_string(path)?;
        let value: Value = toml::from_str(&content)?;

        // Navigate to the specified table
        let table = table_path
            .iter()
            .try_fold(&value, |current, &key| {
                current
                    .get(key)
                    .ok_or_else(|| anyhow::anyhow!("Missing table: {}", key))
            })
            .unwrap_or(&value);

        let mut specs = HashMap::new();
        TagSpec::extract_specs(table, None, &mut specs)
            .map_err(|e| TagSpecError::Extract(e.to_string()))?;
        Ok(TagSpecs(specs))
    }

    /// Load specs from a user's project directory
    pub fn load_user_specs(project_root: &Path) -> Result<Self, anyhow::Error> {
        // List of config files to try, in priority order
        let config_files = ["djls.toml", ".djls.toml", "pyproject.toml"];

        for &file in &config_files {
            let path = project_root.join(file);
            if path.exists() {
                return match file {
                    "pyproject.toml" => {
                        Self::load_from_toml(&path, &["tool", "djls", "template", "tags"])
                    }
                    _ => Self::load_from_toml(&path, &[]), // Root level for other files
                };
            }
        }
        Ok(Self::default())
    }

    /// Load builtin specs from the crate's tagspecs directory
    pub fn load_builtin_specs() -> Result<Self, anyhow::Error> {
        let specs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tagspecs");
        let mut specs = HashMap::new();

        for entry in fs::read_dir(&specs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                let file_specs = Self::load_from_toml(&path, &[])?;
                specs.extend(file_specs.0);
            }
        }

        Ok(TagSpecs(specs))
    }

    /// Merge another TagSpecs into this one, with the other taking precedence
    pub fn merge(&mut self, other: TagSpecs) -> &mut Self {
        self.0.extend(other.0);
        self
    }

    /// Load both builtin and user specs, with user specs taking precedence
    pub fn load_all(project_root: &Path) -> Result<Self, anyhow::Error> {
        let mut specs = Self::load_builtin_specs()?;
        let user_specs = Self::load_user_specs(project_root)?;
        Ok(specs.merge(user_specs).clone())
    }
}

impl TryFrom<&Path> for TagSpecs {
    type Error = TagSpecError;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        Self::load_from_toml(path, &[]).map_err(Into::into)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TagSpec {
    #[serde(rename = "type")]
    pub tag_type: TagType,
    pub closing: Option<String>,
    #[serde(default)]
    pub branches: Option<Vec<String>>,
    pub args: Option<Vec<ArgSpec>>,
}

impl TagSpec {
    fn extract_specs(
        value: &Value,
        prefix: Option<&str>,
        specs: &mut HashMap<String, TagSpec>,
    ) -> Result<(), String> {
        // Try to deserialize as a tag spec first
        match TagSpec::deserialize(value.clone()) {
            Ok(tag_spec) => {
                let name = prefix.map_or_else(String::new, |p| {
                    p.split('.').last().unwrap_or(p).to_string()
                });
                eprintln!(
                    "Found tag spec at '{}', using name '{}'",
                    prefix.unwrap_or(""),
                    name
                );
                specs.insert(name, tag_spec);
            }
            Err(_) => {
                // Not a tag spec, try recursing into any table values
                for (key, value) in value.as_table().iter().flat_map(|t| t.iter()) {
                    let new_prefix = match prefix {
                        None => key.clone(),
                        Some(p) => format!("{}.{}", p, key),
                    };
                    eprintln!("Recursing into prefix: {}", new_prefix);
                    Self::extract_specs(value, Some(&new_prefix), specs)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TagType {
    Block,
    Tag,
    Inclusion,
    Variable,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ArgSpec {
    pub name: String,
    pub required: bool,
}

impl ArgSpec {
    pub fn is_placeholder(arg: &str) -> bool {
        arg.starts_with('{') && arg.ends_with('}')
    }

    pub fn get_placeholder_name(arg: &str) -> Option<&str> {
        if Self::is_placeholder(arg) {
            Some(&arg[1..arg.len() - 1])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_specs_are_valid() -> Result<(), anyhow::Error> {
        let specs = TagSpecs::load_builtin_specs()?;

        assert!(!specs.0.is_empty(), "Should have loaded at least one spec");

        for (name, spec) in &specs.0 {
            assert!(!name.is_empty(), "Tag name should not be empty");
            assert!(
                spec.tag_type == TagType::Block || spec.tag_type == TagType::Variable,
                "Tag type should be block or variable"
            );
        }
        Ok(())
    }

    #[test]
    fn test_builtin_django_tags() -> Result<(), anyhow::Error> {
        let specs = TagSpecs::load_builtin_specs()?;

        // Test using get method
        let if_tag = specs.get("if").expect("if tag should be present");
        assert_eq!(if_tag.tag_type, TagType::Block);
        assert_eq!(if_tag.closing.as_deref(), Some("endif"));
        assert_eq!(if_tag.branches.as_ref().map(|b| b.len()), Some(2));
        assert!(if_tag
            .branches
            .as_ref()
            .unwrap()
            .contains(&"elif".to_string()));
        assert!(if_tag
            .branches
            .as_ref()
            .unwrap()
            .contains(&"else".to_string()));

        let for_tag = specs.get("for").expect("for tag should be present");
        assert_eq!(for_tag.tag_type, TagType::Block);
        assert_eq!(for_tag.closing.as_deref(), Some("endfor"));
        assert_eq!(for_tag.branches.as_ref().map(|b| b.len()), Some(1));
        assert!(for_tag
            .branches
            .as_ref()
            .unwrap()
            .contains(&"empty".to_string()));

        let block_tag = specs.get("block").expect("block tag should be present");
        assert_eq!(block_tag.tag_type, TagType::Block);
        assert_eq!(block_tag.closing.as_deref(), Some("endblock"));

        Ok(())
    }

    #[test]
    fn test_user_defined_tags() -> Result<(), anyhow::Error> {
        // Create a temporary directory for our test project
        let dir = tempfile::tempdir()?;
        let root = dir.path();

        // Create a pyproject.toml with custom tags
        let pyproject_content = r#"
[tool.djls.template.tags.mytag]
type = "block"
closing = "endmytag"
branches = ["mybranch"]
args = [{ name = "myarg", required = true }]
"#;
        fs::write(root.join("pyproject.toml"), pyproject_content)?;

        // Load both builtin and user specs
        let specs = TagSpecs::load_all(root)?;

        // Check that builtin tags are still there
        let if_tag = specs.get("if").expect("if tag should be present");
        assert_eq!(if_tag.tag_type, TagType::Block);

        // Check our custom tag
        let my_tag = specs.get("mytag").expect("mytag should be present");
        assert_eq!(my_tag.tag_type, TagType::Block);
        assert_eq!(my_tag.closing, Some("endmytag".to_string()));

        let branches = my_tag
            .branches
            .as_ref()
            .expect("mytag should have branches");
        assert!(branches.iter().any(|b| b == "mybranch"));

        let args = my_tag.args.as_ref().expect("mytag should have args");
        let arg = &args[0];
        assert_eq!(arg.name, "myarg");
        assert!(arg.required);

        // Clean up temp dir
        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_config_file_priority() -> Result<(), anyhow::Error> {
        // Create a temporary directory for our test project
        let dir = tempfile::tempdir()?;
        let root = dir.path();

        // Create all config files with different tags
        let djls_content = r#"
[mytag1]
type = "block"
closing = "endmytag1"
"#;
        fs::write(root.join("djls.toml"), djls_content)?;

        let pyproject_content = r#"
[tool.djls.template.tags]
mytag2.type = "block"
mytag2.closing = "endmytag2"
"#;
        fs::write(root.join("pyproject.toml"), pyproject_content)?;

        // Load user specs
        let specs = TagSpecs::load_user_specs(root)?;

        // Should only have mytag1 since djls.toml has highest priority
        assert!(specs.get("mytag1").is_some(), "mytag1 should be present");
        assert!(
            specs.get("mytag2").is_none(),
            "mytag2 should not be present"
        );

        // Remove djls.toml and try again
        fs::remove_file(root.join("djls.toml"))?;
        let specs = TagSpecs::load_user_specs(root)?;

        // Should now have mytag2 since pyproject.toml has second priority
        assert!(
            specs.get("mytag1").is_none(),
            "mytag1 should not be present"
        );
        assert!(specs.get("mytag2").is_some(), "mytag2 should be present");

        dir.close()?;
        Ok(())
    }
}
