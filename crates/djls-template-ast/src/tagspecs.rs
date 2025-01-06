use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::ops::{Deref, Index};
use std::path::Path;
use toml::Value;

#[derive(Debug, Default)]
pub struct TagSpecs(HashMap<String, TagSpec>);

impl TagSpecs {
    pub fn get(&self, key: &str) -> Option<&TagSpec> {
        self.0.get(key)
    }
}

impl From<&Path> for TagSpecs {
    fn from(specs_dir: &Path) -> Self {
        let mut specs = HashMap::new();

        for entry in fs::read_dir(specs_dir).expect("Failed to read specs directory") {
            let entry = entry.expect("Failed to read directory entry");
            let path = entry.path();

            if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                let content = fs::read_to_string(&path).expect("Failed to read spec file");

                let value: Value = toml::from_str(&content).expect("Failed to parse TOML");

                TagSpec::extract_specs(&value, None, &mut specs).expect("Failed to extract specs");
            }
        }

        TagSpecs(specs)
    }
}

impl Deref for TagSpecs {
    type Target = HashMap<String, TagSpec>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for TagSpecs {
    type Item = (String, TagSpec);
    type IntoIter = std::collections::hash_map::IntoIter<String, TagSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a TagSpecs {
    type Item = (&'a String, &'a TagSpec);
    type IntoIter = std::collections::hash_map::Iter<'a, String, TagSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl Index<&str> for TagSpecs {
    type Output = TagSpec;

    fn index(&self, index: &str) -> &Self::Output {
        &self.0[index]
    }
}

impl AsRef<HashMap<String, TagSpec>> for TagSpecs {
    fn as_ref(&self) -> &HashMap<String, TagSpec> {
        &self.0
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
    pub fn load_builtin_specs() -> Result<TagSpecs> {
        let specs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tagspecs");
        Ok(TagSpecs::from(specs_dir.as_path()))
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
    fn test_specs_are_valid() -> Result<()> {
        let specs = TagSpec::load_builtin_specs()?;

        assert!(!specs.0.is_empty(), "Should have loaded at least one spec");

        println!("Loaded {} tag specs:", specs.0.len());
        for (name, spec) in &specs.0 {
            println!("  {} ({:?})", name, spec.tag_type);
        }

        Ok(())
    }

    #[test]
    fn test_builtin_django_tags() -> Result<()> {
        let specs = TagSpec::load_builtin_specs()?;

        // Test using Index trait
        let if_tag = &specs["if"];
        assert_eq!(if_tag.tag_type, TagType::Block);
        assert_eq!(if_tag.closing, Some("endif".to_string()));

        let if_branches = if_tag
            .branches
            .as_ref()
            .expect("if tag should have branches");
        assert!(if_branches.iter().any(|b| b == "elif"));
        assert!(if_branches.iter().any(|b| b == "else"));

        // Test using get method
        let for_tag = specs.get("for").expect("for tag should be present");
        assert_eq!(for_tag.tag_type, TagType::Block);
        assert_eq!(for_tag.closing, Some("endfor".to_string()));

        let for_branches = for_tag
            .branches
            .as_ref()
            .expect("for tag should have branches");
        assert!(for_branches.iter().any(|b| b == "empty"));

        // Test using HashMap method directly via Deref
        let block_tag = specs.get("block").expect("block tag should be present");
        assert_eq!(block_tag.tag_type, TagType::Block);
        assert_eq!(block_tag.closing, Some("endblock".to_string()));

        // Test iteration
        let mut count = 0;
        for (name, spec) in &specs {
            println!("Found tag: {} ({:?})", name, spec.tag_type);
            count += 1;
        }
        assert!(count > 0, "Should have found some tags");

        // Test as_ref
        let map_ref: &HashMap<_, _> = specs.as_ref();
        assert_eq!(map_ref.len(), count);

        Ok(())
    }
}
