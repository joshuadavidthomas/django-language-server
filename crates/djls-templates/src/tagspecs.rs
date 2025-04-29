use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use thiserror::Error;
use toml::Value;

#[derive(Debug, Error)]
pub enum TagSpecError {
    #[error("Failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("Failed to extract specs from {0}: {1}")]
    Extract(String, String), // Added path context
    #[error("Configuration error in {0}: {1}")]
    Config(String, String), // Added path context
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Clone, Debug, Default)]
pub struct TagSpecs(HashMap<String, TagSpec>);

impl TagSpecs {
    pub fn get(&self, key: &str) -> Option<&TagSpec> {
        self.0.get(key)
    }

    /// Load specs from a TOML file, looking under the specified table path.
    /// Expects the TOML structure: [table_path...namespace.tag_name] with TagSpec fields inside.
    fn load_from_toml(path: &Path, table_path: &[&str]) -> Result<Self, TagSpecError> {
        let content = fs::read_to_string(path).map_err(TagSpecError::Io)?;
        let value: Value = toml::from_str(&content).map_err(TagSpecError::Toml)?;

        let base_table_result = table_path.iter().try_fold(&value, |current, &key| {
            current.get(key).ok_or_else(|| {
                // Use Config error for missing base table path
                TagSpecError::Config(
                    path.display().to_string(),
                    format!("Base table path segment '{}' not found", key),
                )
            })
        });

        match base_table_result {
            Ok(base_table) => {
            Ok(base_table) => {
                // Base table path found, extract specs from it recursively
                let mut specs = HashMap::new();
                // Start recursion with the base table and an empty initial path prefix.
                // Use map_err to ensure the error type matches.
                extract_specs(base_table, "", &mut specs)
                    .map_err(|e| TagSpecError::Extract(path.display().to_string(), e))?;
                // If base_table_result was Ok and extract_specs succeeded, return the populated specs map
                Ok(TagSpecs(specs))
            }
            Err(e @ TagSpecError::Config(_, _)) => {
                // Base table path not found. Check if it's an optional path (like user config)
                // For now, let's treat missing base paths as an error unless handled upstream.
                // Alternatively, could return Ok(TagSpecs::default()) if missing base is acceptable.
                // Let's refine this based on how load_user_specs uses it.
                // For now, propagate the specific Config error.
                // load_user_specs and load_builtin_specs should handle this variant.
                Err(e)
            }
            Err(e) => {
                // Other errors (IO, TOML parse, Extract) should be reported.
                Err(e) // Propagate other errors directly
            }
        }
    }

    /// Load specs from a user's project directory, checking common config files.
    pub fn load_user_specs(project_root: &Path) -> Result<Self, anyhow::Error> {
        let config_files = [
            ("djls.toml", &["tagspecs"] as &[&str]),
            (".djls.toml", &["tagspecs"]),
            ("pyproject.toml", &["tool", "djls", "tagspecs"]),
        ];

        for (filename, table_path) in config_files {
            let path = project_root.join(filename);
            if path.exists() {
                match Self::load_from_toml(&path, table_path) {
                    Ok(specs) => {
                        // If specs were found in this file, return them immediately
                        // (respecting priority order)
                        if !specs.0.is_empty() {
                            return Ok(specs);
                        }
                        // If file exists but specs are empty or base path missing, continue
                    }
                    Err(TagSpecError::Config(_, _)) => {
                        // Config error means base path wasn't found, which is OK for optional user files.
                        // Continue to the next file.
                    }
                    Err(e) => {
                        // Other errors (IO, TOML parse, Extract) should be reported or logged.
                        eprintln!(
                            "Warning: Failed to load tag specs from {}: {}",
                            path.display(),
                            e
                        );
                        // Decide whether to propagate the error or just continue.
                        // Let's continue for now, allowing partial loading.
                        // return Err(e.into()); // Option to propagate error
                    }
                }
            }
        }

        // If no file yielded specs, return default empty specs
        Ok(Self::default())
    }

    /// Load builtin specs from the crate's tagspecs directory.
    pub fn load_builtin_specs() -> Result<Self, anyhow::Error> {
        let specs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tagspecs");
        let mut all_specs = HashMap::new();

        if !specs_dir.is_dir() {
            // Directory doesn't exist, return empty specs
            eprintln!(
                "Warning: Built-in tagspecs directory not found at {}",
                specs_dir.display()
            );
            return Ok(TagSpecs::default());
        }

        for entry in fs::read_dir(&specs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                match Self::load_from_toml(&path, &["tagspecs"]) {
                    Ok(file_specs) => { // Successfully loaded and extracted from this file
                        all_specs.extend(file_specs.0);
                    }
                    Err(e @ TagSpecError::Config(_, _)) | Err(e @ TagSpecError::Extract(_, _)) => {
                        eprintln!(
                            "Warning: Failed to load built-in tag specs from {}: {}",
                            path.display(),
                            e
                        );
                        // Decide whether to propagate or continue. Let's continue.
                        // return Err(e.into()); // Option to propagate
                    }
                }
            }
        }

        Ok(TagSpecs(all_specs))
    }

    /// Merge another TagSpecs into this one, with the other taking precedence.
    pub fn merge(&mut self, other: TagSpecs) -> &mut Self {
        self.0.extend(other.0);
        self
    }

    /// Load both builtin and user specs, with user specs taking precedence.
    pub fn load_all(project_root: &Path) -> Result<Self, anyhow::Error> {
        let mut specs = Self::load_builtin_specs()?;
        let user_specs = Self::load_user_specs(project_root)?;
        // User specs loaded later will overwrite built-ins if keys conflict
        Ok(specs.merge(user_specs).clone())
    }
}


/// Recursive helper function to extract TagSpec definitions from dotted path keys.
/// Expects the TOML structure: [base...namespace.tag_name] containing TagSpec fields.
fn extract_specs(
    current_value: &Value,
    current_path: &str, // The path leading up to current_value, e.g., "django.template.defaulttags"
    specs_map: &mut HashMap<String, TagSpec>,
) -> Result<(), String> {

    // First, check if the current_value *itself* could be a TagSpec definition.
    // This happens when the current_path represents the full path to the tag.
    // We only attempt this if current_path is not empty (i.e., we are not at the root base table).
    if !current_path.is_empty() {
        match TagSpec::deserialize(current_value.clone()) {
            Ok(tag_spec) => {
                // Success! current_value represents a TagSpec definition.
                // Extract tag_name from the *end* of current_path.
                if let Some(tag_name) = current_path.split('.').last().filter(|s| !s.is_empty()) {
                    // Insert into the map. Handle potential duplicates/overrides if needed.
                    specs_map.insert(tag_name.to_string(), tag_spec);
                    // Don't recurse further down this branch, we found the spec.
                    return Ok(());
                } else {
                     // This case should ideally not happen if current_path is not empty,
                     // but handle defensively.
                    return Err(format!("Could not extract tag name from non-empty path '{}'", current_path));
                }
            }
            Err(_) => { // Keep Err(_) to catch deserialization errors gracefully
                // Deserialization as TagSpec failed. It might be a namespace table.
                // Continue below to check if it's a table and recurse.
            }
        }
    }

    // If it wasn't a TagSpec or if we were at the root (empty path),
    // check if it's a table and recurse into its children.
    if let Some(table) = current_value.as_table() {
        for (key, inner_value) in table.iter() {
            // Construct the new path for the recursive call
            let new_path = if current_path.is_empty() { key.clone() } else { format!("{}.{}", current_path, key) };
            // Recurse
            if let Err(e) = extract_specs(inner_value, &new_path, specs_map) {
                // Propagate errors from recursive calls
                // Optionally add more context here if needed
                return Err(e);
            }
        }
    }
    // If it's not a table and not a TagSpec, ignore it.
    Ok(())
}


/// Defines the structure and relationships for a specific template tag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagSpec {
    /// Information about the closing tag, if one exists.
    pub end: Option<EndTag>,

    /// List of intermediate tag names.
    #[serde(default)]
    pub intermediates: Option<Vec<String>>,
}

/// Defines properties of the end tag associated with a TagSpec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndTag {
    /// The name of the closing tag.
    pub tag: String,

    /// If true, the end tag's presence is optional for the block
    /// to be considered validly closed. Defaults to false (required).
    #[serde(default)]
    pub optional: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_temp_spec_dir(
        filename: &str,
        content: &str,
    ) -> Result<tempfile::TempDir, anyhow::Error> {
        let dir = tempfile::tempdir()?; // Create temp dir in system default location
        let specs_dir = dir.path().join("tagspecs");
        fs::create_dir_all(&specs_dir)?; // Use create_dir_all
        fs::write(specs_dir.join(filename), content)?;
        Ok(dir)
    }

    #[test]
    fn test_load_builtin_simple() -> Result<(), anyhow::Error> { // Renamed from test_can_load_builtins
        let content = r#"
# Using dotted path table names under [tagspecs] base
[tagspecs.django.template.defaulttags.if] // Corrected path
end = { tag = "endif" }
[tagspecs.django.template.defaulttags.block]
end = { tag = "endblock" }
intermediates = ["inner"]
"#;
        let dir = setup_temp_spec_dir("django.toml", content)?;
        // Temporarily override CARGO_MANIFEST_DIR for this test
        let original_manifest_dir = std::env::var("CARGO_MANIFEST_DIR");
        std::env::set_var("CARGO_MANIFEST_DIR", dir.path());

        let specs = TagSpecs::load_builtin_specs()?;
        eprintln!("Loaded Builtin Specs: {:?}", specs);

        assert_eq!(specs.0.len(), 2);
        assert!(specs.get("if").is_some());
        assert!(specs.get("block").is_some());

        let if_spec = specs.get("if").unwrap();
        assert_eq!(if_spec.end.as_ref().unwrap().tag, "endif");
        assert!(!if_spec.end.as_ref().unwrap().optional); // Default false
        assert!(if_spec.intermediates.is_none());

        let block_spec = specs.get("block").unwrap();
        assert_eq!(block_spec.end.as_ref().unwrap().tag, "endblock");
        assert!(!block_spec.end.as_ref().unwrap().optional);
        assert_eq!(block_spec.intermediates.as_ref().unwrap(), &["inner"]);

        // Restore original env var if it existed
        if let Ok(val) = original_manifest_dir {
            std::env::set_var("CARGO_MANIFEST_DIR", val);
        } else {
            std::env::remove_var("CARGO_MANIFEST_DIR");
        }
        dir.close()?; // Ensure temp dir is cleaned up
        Ok(())
    }

    #[test]
    fn test_load_builtin_optional_end() -> Result<(), anyhow::Error> {
         let content = r#"
[tagspecs.custom.mytag]
end = { tag = "endmytag", optional = true }
"#;
        let dir = setup_temp_spec_dir("custom.toml", content)?;
        let original_manifest_dir = std::env::var("CARGO_MANIFEST_DIR");
        std::env::set_var("CARGO_MANIFEST_DIR", dir.path());

        let specs = TagSpecs::load_builtin_specs()?;
        eprintln!("Loaded Builtin Specs: {:?}", specs);

        assert_eq!(specs.0.len(), 1);
        let mytag_spec = specs.get("mytag").unwrap();
        assert_eq!(mytag_spec.end.as_ref().unwrap().tag, "endmytag");
        assert!(mytag_spec.end.as_ref().unwrap().optional); // Check optional=true

        if let Ok(val) = original_manifest_dir {
            std::env::set_var("CARGO_MANIFEST_DIR", val);
        } else {
            std::env::remove_var("CARGO_MANIFEST_DIR");
        }
        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_load_user_djls_toml() -> Result<(), anyhow::Error> { // Renamed from test_user_defined_tags
        let dir = tempfile::tempdir()?;
        let root = dir.path();
         // User specs under [tagspecs] base table
        let djls_content = r#"
[tagspecs.custom.app.tags.mytag]
end = { tag = "endmytag" }
"#;
        fs::write(root.join("djls.toml"), djls_content)?;

        let specs = TagSpecs::load_user_specs(root)?;
        eprintln!("Loaded User Specs (djls.toml): {:?}", specs);

        assert_eq!(specs.0.len(), 1);
        assert!(specs.get("mytag").is_some());
        assert_eq!(
            specs.get("mytag").unwrap().end.as_ref().unwrap().tag,
            "endmytag"
        );

        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_load_user_pyproject_toml() -> Result<(), anyhow::Error> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
         // User specs under [tool.djls.tagspecs] base table
        let pyproject_content = r#"
[tool.djls.tagspecs.another.lib.othertag]
end = { tag = "endother" }
intermediates = ["branch"]
"#;
        fs::write(root.join("pyproject.toml"), pyproject_content)?;

        let specs = TagSpecs::load_user_specs(root)?;
        eprintln!("Loaded User Specs (pyproject.toml): {:?}", specs);

        assert_eq!(specs.0.len(), 1);
        assert!(specs.get("othertag").is_some());
        let spec = specs.get("othertag").unwrap();
        assert_eq!(spec.end.as_ref().unwrap().tag, "endother");
        assert_eq!(spec.intermediates.as_ref().unwrap(), &["branch"]);

        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_config_file_priority() -> Result<(), anyhow::Error> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();

        // djls.toml has higher priority
         // Uses [tagspecs] base
        let djls_content = r#"
[tagspecs.common.tag1]
end = { tag = "endtag1_djls" }
[tagspecs.common.tag_djls]
end = { tag = "end_djls_only"}
"#;
         fs::write(root.join("djls.toml"), djls_content)?;

         // pyproject.toml has lower priority, uses [tool.djls.tagspecs] base
        let pyproject_content = r#"
[tool.djls.tagspecs.common.tag1]
end = { tag = "endtag1_pyproj" }
[tool.djls.tagspecs.common.tag_pyproj]
end = { tag = "end_pyproj_only"}
"#;
        fs::write(root.join("pyproject.toml"), pyproject_content)?;

        // Load with both present - djls.toml should win
        let specs = TagSpecs::load_user_specs(root)?;
        eprintln!("Loaded Specs (Priority Test - Both Present): {:?}", specs);

        assert_eq!(specs.0.len(), 2, "Should load 2 specs from djls.toml");
        assert!(specs.get("tag1").is_some(), "tag1 should be present");
        assert_eq!(
            specs.get("tag1").unwrap().end.as_ref().unwrap().tag,
            "endtag1_djls",
            "tag1 should come from djls.toml"
        );
        assert!(
            specs.get("tag_djls").is_some(),
            "tag_djls should be present"
        );
        assert!(
            specs.get("tag_pyproj").is_none(),
            "tag_pyproj should NOT be present"
        );

        // Remove djls.toml, now pyproject.toml should be loaded
        fs::remove_file(root.join("djls.toml"))?;
        let specs = TagSpecs::load_user_specs(root)?;
        eprintln!(
            "Loaded Specs (Priority Test - Only pyproject.toml): {:?}",
            specs
        );

        assert_eq!(specs.0.len(), 2, "Should load 2 specs from pyproject.toml");
        assert!(specs.get("tag1").is_some(), "tag1 should be present");
        assert_eq!(
            specs.get("tag1").unwrap().end.as_ref().unwrap().tag,
            "endtag1_pyproj",
            "tag1 should come from pyproject.toml"
        );
        assert!(
            specs.get("tag_djls").is_none(),
            "tag_djls should NOT be present"
        );
        assert!(
            specs.get("tag_pyproj").is_some(),
            "tag_pyproj should be present"
        );

        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_load_all_merging() -> Result<(), anyhow::Error> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();

        // Create a dummy built-in spec file
        let builtin_content = r#"
[tagspecs.django.template.defaulttags.if]
end = { tag = "endif_builtin" }
[tagspecs.django.template.defaulttags.block]
end = { tag = "endblock_builtin" }
"#;
         let specs_dir = root.join("tagspecs"); // Simulate built-in dir inside temp
        fs::create_dir_all(&specs_dir)?; // Use create_dir_all
        fs::write(specs_dir.join("django.toml"), builtin_content)?;

         // Create a user override file (djls.toml has priority)
        let user_content = r#"
[tagspecs.django.template.defaulttags.if]
end = { tag = "endif_user" } # Override built-in 'if'
[tagspecs.custom.custom]
 end = { tag = "endcustom_user" } # Add user tag
"#;
        fs::write(root.join("djls.toml"), user_content)?;

        // Temporarily override CARGO_MANIFEST_DIR for load_builtin_specs
        let original_manifest_dir = std::env::var("CARGO_MANIFEST_DIR");
        std::env::set_var("CARGO_MANIFEST_DIR", root.to_str().unwrap());

        // Load all, user should override built-in
        let specs = TagSpecs::load_all(root)?;
        eprintln!("Loaded Specs (Load All): {:?}", specs);

        assert_eq!(
            specs.0.len(),
            3,
            "Should have 3 specs total (if, block, custom)"
        );
        assert!(specs.get("if").is_some());
        assert!(specs.get("block").is_some());
        assert!(specs.get("custom").is_some()); // Check the user-added tag name

        // Check override
        assert_eq!(
            specs.get("if").unwrap().end.as_ref().unwrap().tag,
            "endif_user"
        );
        // Check preserved built-in
        assert_eq!(
            specs.get("block").unwrap().end.as_ref().unwrap().tag,
            "endblock_builtin"
        );
        // Check added user tag
        assert_eq!(
            specs.get("custom").unwrap().end.as_ref().unwrap().tag,
            "endcustom_user"
        );

        if let Ok(val) = original_manifest_dir {
            std::env::set_var("CARGO_MANIFEST_DIR", val);
        } else {
            std::env::remove_var("CARGO_MANIFEST_DIR");
        }
        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_load_builtin_missing_dir() -> Result<(), anyhow::Error> {
        // Point CARGO_MANIFEST_DIR to a non-existent path temporarily
        let dir = tempfile::tempdir()?; // Need a valid path for temp env var setting
        let original_manifest_dir = std::env::var("CARGO_MANIFEST_DIR");
        std::env::set_var("CARGO_MANIFEST_DIR", dir.path().join("nonexistent"));

         let specs = TagSpecs::load_builtin_specs()?;
        assert!(
            specs.0.is_empty(),
            "Should return empty specs if dir is missing"
        );

        if let Ok(val) = original_manifest_dir {
            std::env::set_var("CARGO_MANIFEST_DIR", val);
        } else {
            std::env::remove_var("CARGO_MANIFEST_DIR");
        }
        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_load_builtin_missing_base_table() -> Result<(), anyhow::Error> {
        // File exists but doesn't have [tagspecs]
        let content = r#"
[other_table]
key = "value"
"#;
        let dir = setup_temp_spec_dir("invalid.toml", content)?;
        let original_manifest_dir = std::env::var("CARGO_MANIFEST_DIR");
        std::env::set_var("CARGO_MANIFEST_DIR", dir.path());

        // load_builtin_specs expects [tagspecs], so load_from_toml will error
        // Check that load_builtin_specs handles this gracefully (logs warning, returns empty)
         let specs = TagSpecs::load_builtin_specs()?;
        assert!(
            specs.0.is_empty(),
            "Should return empty specs if base table is missing"
        );
        // TODO: Capture stderr to verify warning was printed?

        if let Ok(val) = original_manifest_dir {
            std::env::set_var("CARGO_MANIFEST_DIR", val);
        } else {
            std::env::remove_var("CARGO_MANIFEST_DIR");
        }
        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_load_user_missing_base_table() -> Result<(), anyhow::Error> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        // djls.toml exists but doesn't have [tagspecs]
        let djls_content = r#"
[other_table]
key = "value"
"#;
        fs::write(root.join("djls.toml"), djls_content)?;

        // load_user_specs should ignore this file because base table is missing
         let specs = TagSpecs::load_user_specs(root)?;
        assert!(
            specs.0.is_empty(),
            "Should return empty specs if base table is missing in user file"
        );

        dir.close()?;
        Ok(())
    }
}
