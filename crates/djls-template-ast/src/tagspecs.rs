use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use toml::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct TagSpec {
    #[serde(rename = "type")]
    pub tag_type: TagType,
    pub closing: Option<String>,
    #[serde(rename = "intermediates")]
    pub branches: Option<Vec<BranchSpec>>,
    pub args: Option<Vec<ArgSpec>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BranchSpec {
    pub name: String,
    pub args: bool,
}

impl TagSpec {
    pub fn load_builtin_specs() -> Result<HashMap<String, TagSpec>> {
        let specs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tagspecs");

        let mut all_specs = HashMap::new();

        for entry in fs::read_dir(&specs_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                let content = fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read {:?}", path))?;

                let value: Value = toml::from_str(&content)
                    .with_context(|| format!("Failed to parse {:?}", path))?;

                Self::extract_specs(&value, "", &mut all_specs)?;
            }
        }

        Ok(all_specs)
    }

    fn extract_specs(
        value: &Value,
        prefix: &str,
        specs: &mut HashMap<String, TagSpec>,
    ) -> Result<()> {
        if let Value::Table(table) = value {
            // If this table has a 'type' field, try to parse it as a TagSpec
            if table.contains_key("type") {
                if let Ok(tag_spec) = TagSpec::deserialize(value.clone()) {
                    let name = prefix.split('.').last().unwrap_or(prefix);
                    specs.insert(name.to_string(), tag_spec);
                    return Ok(());
                }
            }

            // Otherwise, recursively process each field
            for (key, value) in table {
                let new_prefix = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                Self::extract_specs(value, &new_prefix, specs)?;
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
    Assignment,
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
    fn test_specs_are_valid() -> Result<()> {
        let specs = TagSpec::load_builtin_specs()?;

        assert!(!specs.is_empty(), "Should have loaded at least one spec");

        println!("Loaded {} tag specs:", specs.len());
        for (name, spec) in &specs {
            println!("  {} ({:?})", name, spec.tag_type);
        }

        Ok(())
    }
}
