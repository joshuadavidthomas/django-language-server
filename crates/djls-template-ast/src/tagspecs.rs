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
    #[serde(default)]
    pub branches: Option<Vec<String>>,
    pub args: Option<Vec<ArgSpec>>,
}

impl TagSpec {
    pub fn load_builtin_specs() -> Result<HashMap<String, TagSpec>> {
        let mut specs = HashMap::new();

        let specs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tagspecs");

        for entry in fs::read_dir(&specs_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                let content = fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read {:?}", path))?;

                let value: Value = toml::from_str(&content)
                    .with_context(|| format!("Failed to parse {:?}", path))?;

                Self::extract_specs(&value, None, &mut specs)?;
            }
        }

        eprintln!("specs: {:?}", specs);

        Ok(specs)
    }

    fn extract_specs(
        value: &Value,
        prefix: Option<&str>,
        specs: &mut HashMap<String, TagSpec>,
    ) -> Result<()> {
        // Try to deserialize as a tag spec first
        match TagSpec::deserialize(value.clone()) {
            Ok(tag_spec) => {
                let name = prefix.map_or_else(String::new, |p| {
                    p.split('.').last().unwrap_or(p).to_string()
                });
                eprintln!("Found tag spec at '{}', using name '{}'", prefix.unwrap_or(""), name);
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
    fn test_specs_are_valid() -> Result<()> {
        let specs = TagSpec::load_builtin_specs()?;

        assert!(!specs.is_empty(), "Should have loaded at least one spec");

        println!("Loaded {} tag specs:", specs.len());
        for (name, spec) in &specs {
            println!("  {} ({:?})", name, spec.tag_type);
        }

        Ok(())
    }

    #[test]
    fn test_builtin_django_tags() -> Result<()> {
        let specs = TagSpec::load_builtin_specs()?;

        let if_tag = specs.get("if").expect("if tag should be present");
        assert_eq!(if_tag.tag_type, TagType::Block);
        assert_eq!(if_tag.closing, Some("endif".to_string()));
        let if_branches = if_tag
            .branches
            .as_ref()
            .expect("if tag should have branches");
        assert!(if_branches.iter().any(|b| b == "elif"));
        assert!(if_branches.iter().any(|b| b == "else"));

        let for_tag = specs.get("for").expect("for tag should be present");
        assert_eq!(for_tag.tag_type, TagType::Block);
        assert_eq!(for_tag.closing, Some("endfor".to_string()));
        let for_branches = for_tag
            .branches
            .as_ref()
            .expect("for tag should have branches");
        assert!(for_branches.iter().any(|b| b == "empty"));

        let block_tag = specs.get("block").expect("block tag should be present");
        assert_eq!(block_tag.tag_type, TagType::Block);
        assert_eq!(block_tag.closing, Some("endblock".to_string()));

        Ok(())
    }
}
