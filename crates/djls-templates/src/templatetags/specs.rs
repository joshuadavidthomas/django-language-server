use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use thiserror::Error;
use toml::Value;

#[derive(Debug, Error)]
pub enum TagSpecError {
    #[error("Failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("Failed to extract specs: {0}")]
    #[allow(dead_code)]
    Extract(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct TagSpecs(HashMap<String, TagSpec>);

impl TagSpecs {
    #[allow(dead_code)]
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&TagSpec> {
        self.0.get(key)
    }

    /// Iterate over all tag specs
    pub fn iter(&self) -> impl Iterator<Item = (&String, &TagSpec)> {
        self.0.iter()
    }

    /// Find the opener tag for a given closer tag
    #[must_use]
    pub fn find_opener_for_closer(&self, closer: &str) -> Option<String> {
        for (tag_name, spec) in &self.0 {
            if let Some(end_spec) = &spec.end_tag {
                if end_spec.name == closer {
                    return Some(tag_name.clone());
                }
            }
        }
        None
    }

    /// Get the end tag spec for a given closer tag
    #[must_use]
    pub fn get_end_spec_for_closer(&self, closer: &str) -> Option<&EndTag> {
        for spec in self.0.values() {
            if let Some(end_spec) = &spec.end_tag {
                if end_spec.name == closer {
                    return Some(end_spec);
                }
            }
        }
        None
    }

    #[must_use]
    pub fn is_opener(&self, name: &str) -> bool {
        self.0
            .get(name)
            .and_then(|spec| spec.end_tag.as_ref())
            .is_some()
    }

    #[must_use]
    pub fn is_intermediate(&self, name: &str) -> bool {
        self.0.values().any(|spec| {
            spec.intermediate_tags
                .as_ref()
                .is_some_and(|intermediate_tags| {
                    intermediate_tags.iter().any(|tag| tag.name == name)
                })
        })
    }

    #[must_use]
    pub fn is_closer(&self, name: &str) -> bool {
        self.0.values().any(|spec| {
            spec.end_tag
                .as_ref()
                .is_some_and(|end_tag| end_tag.name == name)
        })
    }

    /// Get the parent tags that can contain this intermediate tag
    #[must_use]
    pub fn get_parent_tags_for_intermediate(&self, intermediate: &str) -> Vec<String> {
        let mut parents = Vec::new();
        for (opener_name, spec) in &self.0 {
            if let Some(intermediate_tags) = &spec.intermediate_tags {
                if intermediate_tags.iter().any(|tag| tag.name == intermediate) {
                    parents.push(opener_name.clone());
                }
            }
        }
        parents
    }

    /// Load specs from a TOML string
    #[allow(dead_code)]
    pub fn from_toml(toml_str: &str) -> Result<Self, TagSpecError> {
        let value: Value = toml::from_str(toml_str)?;
        let mut specs = HashMap::new();

        // Look for tagspecs table
        if let Some(tagspecs) = value.get("tagspecs") {
            TagSpec::extract_specs(tagspecs, Some("tagspecs"), &mut specs)
                .map_err(TagSpecError::Extract)?;
        }

        Ok(TagSpecs(specs))
    }

    /// Load specs from a TOML file, looking under the specified table path
    #[allow(dead_code)]
    fn load_from_toml(path: &Path, table_path: &[&str]) -> Result<Self, TagSpecError> {
        let content = fs::read_to_string(path)?;
        let value: Value = toml::from_str(&content)?;

        let start_node = table_path
            .iter()
            .try_fold(&value, |current, &key| current.get(key));

        let mut specs = HashMap::new();

        if let Some(node) = start_node {
            let initial_prefix = if table_path.is_empty() {
                None
            } else {
                Some(table_path.join("."))
            };
            TagSpec::extract_specs(node, initial_prefix.as_deref(), &mut specs)
                .map_err(TagSpecError::Extract)?;
        }

        Ok(TagSpecs(specs))
    }

    /// Load specs from a user's project directory
    #[allow(dead_code)]
    pub fn load_user_specs(project_root: &Path) -> Result<Self, anyhow::Error> {
        let config_files = ["djls.toml", ".djls.toml", "pyproject.toml"];

        for &file in &config_files {
            let path = project_root.join(file);
            if path.exists() {
                let result = match file {
                    "pyproject.toml" => Self::load_from_toml(&path, &["tool", "djls", "tagspecs"]),
                    _ => Self::load_from_toml(&path, &["tagspecs"]),
                };
                return result.map_err(anyhow::Error::from);
            }
        }
        Ok(Self::default())
    }

    /// Load builtin specs from the crate's tagspecs directory
    #[allow(dead_code)]
    pub fn load_builtin_specs() -> Result<Self, anyhow::Error> {
        let specs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tagspecs");
        let mut specs = HashMap::new();

        for entry in fs::read_dir(&specs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                let file_specs = Self::load_from_toml(&path, &["tagspecs"])?;
                specs.extend(file_specs.0);
            }
        }

        Ok(TagSpecs(specs))
    }

    /// Merge another `TagSpecs` into this one, with the other taking precedence
    #[allow(dead_code)]
    pub fn merge(&mut self, other: TagSpecs) -> &mut Self {
        self.0.extend(other.0);
        self
    }

    /// Load both builtin and user specs, with user specs taking precedence
    #[allow(dead_code)]
    pub fn load_all(project_root: &Path) -> Result<Self, anyhow::Error> {
        let mut specs = Self::load_builtin_specs()?;
        let user_specs = Self::load_user_specs(project_root)?;
        Ok(specs.merge(user_specs).clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(alias = "end")]
    pub end_tag: Option<EndTag>,
    #[serde(default, alias = "intermediates")]
    pub intermediate_tags: Option<Vec<IntermediateTag>>,
    #[serde(default)]
    pub args: Vec<Arg>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Arg {
    pub name: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(rename = "type")]
    pub arg_type: ArgType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ArgType {
    Simple(SimpleArgType),
    Choice { choice: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SimpleArgType {
    Literal,
    Variable,
    String,
    Expression,
    Assignment,
    VarArgs,
}

fn default_true() -> bool {
    true
}

// Keep ArgSpec for backward compatibility in EndTag
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndTag {
    #[serde(alias = "tag")]
    pub name: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub args: Vec<Arg>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IntermediateTag {
    pub name: String,
}

impl<'de> Deserialize<'de> for IntermediateTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum IntermediateTagHelper {
            String(String),
            Object { name: String },
        }

        match IntermediateTagHelper::deserialize(deserializer)? {
            IntermediateTagHelper::String(s) => Ok(IntermediateTag { name: s }),
            IntermediateTagHelper::Object { name } => Ok(IntermediateTag { name }),
        }
    }
}

impl TagSpec {
    /// Recursive extraction: Check if node is spec, otherwise recurse if table.
    #[allow(dead_code)]
    fn extract_specs(
        value: &Value,
        prefix: Option<&str>, // Path *to* this value node
        specs: &mut HashMap<String, TagSpec>,
    ) -> Result<(), String> {
        // Check if this is an array of TagSpec entries (new format)
        if let Some(array) = value.as_array() {
            for item in array {
                if let Some(table) = item.as_table() {
                    // Check if it has a 'name' field (new format)
                    if table.contains_key("name") {
                        match TagSpec::deserialize(item.clone()) {
                            Ok(mut tag_spec) => {
                                if let Some(name) = tag_spec.name.take() {
                                    specs.insert(name, tag_spec);
                                } else {
                                    return Err(
                                        "TagSpec has 'name' field but it's None".to_string()
                                    );
                                }
                            }
                            Err(e) => {
                                return Err(format!("Failed to deserialize TagSpec in array: {e}"));
                            }
                        }
                    }
                }
            }
            return Ok(());
        }

        // Check if the current node *itself* represents a TagSpec definition (old format)
        // We can be more specific: check if it's a table containing 'end'/'end_tag', 'intermediates'/'intermediate_tags', or 'args'
        let mut is_spec_node = false;
        if let Some(table) = value.as_table() {
            if table.contains_key("end")
                || table.contains_key("end_tag")
                || table.contains_key("intermediates")
                || table.contains_key("intermediate_tags")
                || table.contains_key("args")
            {
                // Looks like a spec, try to deserialize
                match TagSpec::deserialize(value.clone()) {
                    Ok(tag_spec) => {
                        // It is a TagSpec. Get name from prefix.
                        if let Some(p) = prefix {
                            if let Some(name) = p.split('.').next_back().filter(|s| !s.is_empty()) {
                                specs.insert(name.to_string(), tag_spec);
                                is_spec_node = true;
                            } else {
                                return Err(format!(
                                    "Invalid prefix '{p}' resulted in empty tag name component."
                                ));
                            }
                        } else {
                            return Err("Cannot determine tag name for TagSpec: prefix is None."
                                .to_string());
                        }
                    }
                    Err(e) => {
                        // Looked like a spec but failed to deserialize. This is an error.
                        return Err(format!(
                            "Failed to deserialize potential TagSpec at prefix '{}': {}",
                            prefix.unwrap_or("<root>"),
                            e
                        ));
                    }
                }
            }
        }

        // If the node was successfully processed as a spec, DO NOT recurse into its fields.
        // Otherwise, if it's a table, recurse into its children.
        if !is_spec_node {
            if let Some(table) = value.as_table() {
                for (key, inner_value) in table {
                    let new_prefix = match prefix {
                        None => key.clone(),
                        Some(p) => format!("{p}.{key}"),
                    };
                    Self::extract_specs(inner_value, Some(&new_prefix), specs)?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn test_can_load_builtins() -> Result<(), anyhow::Error> {
        let specs = TagSpecs::load_builtin_specs()?;

        assert!(!specs.0.is_empty(), "Should have loaded at least one spec");

        assert!(specs.get("if").is_some(), "'if' tag should be present");

        for name in specs.0.keys() {
            assert!(!name.is_empty(), "Tag name should not be empty");
        }
        Ok(())
    }

    #[test]
    fn test_builtin_django_tags() -> Result<(), anyhow::Error> {
        let specs = TagSpecs::load_builtin_specs()?;

        let expected_tags = [
            "autoescape",
            "block",
            "comment",
            "filter",
            "for",
            "if",
            "ifchanged",
            "spaceless",
            "verbatim",
            "with",
            "cache",
            "localize",
            "blocktranslate",
            "localtime",
            "timezone",
        ];
        // These are single tags that should also be present
        let single_tags = [
            "csrf_token",
            "cycle",
            "extends",
            "include",
            "load",
            "now",
            "templatetag",
            "url",
        ];

        for tag in expected_tags {
            assert!(specs.get(tag).is_some(), "{tag} tag should be present");
        }

        for tag in single_tags {
            assert!(specs.get(tag).is_some(), "{tag} tag should be present");
        }

        // Check that newly added tags are present
        let additional_tags = ["debug", "firstof", "lorem", "regroup", "widthratio"];

        for tag in additional_tags {
            assert!(specs.get(tag).is_some(), "{tag} tag should be present");
        }

        // Check that some tags are still missing
        let missing_tags = [
            "querystring", // Django 5.1+
            "resetcycle",
        ];

        for tag in missing_tags {
            assert!(
                specs.get(tag).is_none(),
                "{tag} tag should not be present yet"
            );
        }

        Ok(())
    }

    #[test]
    fn test_user_defined_tags() -> Result<(), anyhow::Error> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();

        let pyproject_content = r#"
[tool.djls.tagspecs.mytag]
end = { tag = "endmytag" }
intermediates = ["mybranch"]

[tool.djls.tagspecs.anothertag]
end = { tag = "endanothertag", optional = true }
"#;
        fs::write(root.join("pyproject.toml"), pyproject_content)?;

        // Load all (built-in + user)
        let specs = TagSpecs::load_all(root)?;

        assert!(specs.get("if").is_some(), "'if' tag should be present");

        let my_tag = specs.get("mytag").expect("mytag should be present");
        assert_eq!(
            my_tag.end_tag,
            Some(EndTag {
                name: "endmytag".to_string(),
                optional: false,
                args: vec![],
            })
        );
        assert_eq!(
            my_tag.intermediate_tags,
            Some(vec![IntermediateTag {
                name: "mybranch".to_string()
            }])
        );

        let another_tag = specs
            .get("anothertag")
            .expect("anothertag should be present");
        assert_eq!(
            another_tag.end_tag,
            Some(EndTag {
                name: "endanothertag".to_string(),
                optional: true,
                args: vec![],
            })
        );
        assert_eq!(
            my_tag.intermediate_tags,
            Some(vec![IntermediateTag {
                name: "mybranch".to_string()
            }])
        );

        let another_tag = specs
            .get("anothertag")
            .expect("anothertag should be present");
        assert_eq!(
            another_tag.end_tag,
            Some(EndTag {
                name: "endanothertag".to_string(),
                optional: true,
                args: vec![],
            })
        );
        assert!(
            another_tag.intermediate_tags.is_none(),
            "anothertag should have no intermediate_tags"
        );

        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_config_file_priority() -> Result<(), anyhow::Error> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();

        // djls.toml has higher priority
        let djls_content = r#"
[tagspecs.mytag1]
end = { tag = "endmytag1_from_djls" }
"#;
        fs::write(root.join("djls.toml"), djls_content)?;

        // pyproject.toml has lower priority
        let pyproject_content = r#"
[tool.djls.tagspecs.mytag1]
end = { tag = "endmytag1_from_pyproject" }

[tool.djls.tagspecs.mytag2]
end = { tag = "endmytag2_from_pyproject" }
"#;
        fs::write(root.join("pyproject.toml"), pyproject_content)?;

        let specs = TagSpecs::load_user_specs(root)?;

        let tag1 = specs.get("mytag1").expect("mytag1 should be present");
        assert_eq!(tag1.end_tag.as_ref().unwrap().name, "endmytag1_from_djls");

        // Should not find mytag2 because djls.toml was found first
        assert!(
            specs.get("mytag2").is_none(),
            "mytag2 should not be present when djls.toml exists"
        );

        // Remove djls.toml, now pyproject.toml should be used
        fs::remove_file(root.join("djls.toml"))?;
        let specs = TagSpecs::load_user_specs(root)?;

        let tag1 = specs.get("mytag1").expect("mytag1 should be present now");
        assert_eq!(
            tag1.end_tag.as_ref().unwrap().name,
            "endmytag1_from_pyproject"
        );

        assert!(
            specs.get("mytag2").is_some(),
            "mytag2 should be present when only pyproject.toml exists"
        );

        dir.close()?;
        Ok(())
    }
}
