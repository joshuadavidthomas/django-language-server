use std::collections::HashMap;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct TagSpecs(HashMap<String, TagSpec>);

impl TagSpecs {
    /// Create a new `TagSpecs` from a `HashMap`
    #[must_use]
    pub fn new(specs: HashMap<String, TagSpec>) -> Self {
        TagSpecs(specs)
    }

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

    /// Merge another `TagSpecs` into this one, with the other taking precedence
    #[allow(dead_code)]
    pub fn merge(&mut self, other: TagSpecs) -> &mut Self {
        self.0.extend(other.0);
        self
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

impl Arg {
    // Variable types
    pub fn var(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            arg_type: ArgType::Simple(SimpleArgType::Variable),
        }
    }

    pub fn opt_var(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: false,
            arg_type: ArgType::Simple(SimpleArgType::Variable),
        }
    }

    // Literal types
    pub fn literal(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            arg_type: ArgType::Simple(SimpleArgType::Literal),
        }
    }

    pub fn opt_literal(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: false,
            arg_type: ArgType::Simple(SimpleArgType::Literal),
        }
    }

    // String types
    pub fn string(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            arg_type: ArgType::Simple(SimpleArgType::String),
        }
    }

    pub fn opt_string(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: false,
            arg_type: ArgType::Simple(SimpleArgType::String),
        }
    }

    // Expression types
    pub fn expr(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            arg_type: ArgType::Simple(SimpleArgType::Expression),
        }
    }

    // VarArgs types
    pub fn varargs(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            arg_type: ArgType::Simple(SimpleArgType::VarArgs),
        }
    }

    pub fn opt_varargs(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: false,
            arg_type: ArgType::Simple(SimpleArgType::VarArgs),
        }
    }

    // Choice types
    pub fn choice(name: impl Into<String>, choices: Vec<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            arg_type: ArgType::Choice { choice: choices },
        }
    }

    pub fn opt_choice(name: impl Into<String>, choices: Vec<String>) -> Self {
        Self {
            name: name.into(),
            required: false,
            arg_type: ArgType::Choice { choice: choices },
        }
    }
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

// Conversions from djls_conf types to canonical djls_templates types

impl From<djls_conf::SimpleArgTypeDef> for SimpleArgType {
    fn from(value: djls_conf::SimpleArgTypeDef) -> Self {
        match value {
            djls_conf::SimpleArgTypeDef::Literal => SimpleArgType::Literal,
            djls_conf::SimpleArgTypeDef::Variable => SimpleArgType::Variable,
            djls_conf::SimpleArgTypeDef::String => SimpleArgType::String,
            djls_conf::SimpleArgTypeDef::Expression => SimpleArgType::Expression,
            djls_conf::SimpleArgTypeDef::Assignment => SimpleArgType::Assignment,
            djls_conf::SimpleArgTypeDef::VarArgs => SimpleArgType::VarArgs,
        }
    }
}

impl From<djls_conf::ArgTypeDef> for ArgType {
    fn from(value: djls_conf::ArgTypeDef) -> Self {
        match value {
            djls_conf::ArgTypeDef::Simple(simple) => ArgType::Simple(simple.into()),
            djls_conf::ArgTypeDef::Choice { choice } => ArgType::Choice { choice },
        }
    }
}

impl From<djls_conf::TagArgDef> for Arg {
    fn from(value: djls_conf::TagArgDef) -> Self {
        Arg {
            name: value.name,
            required: value.required,
            arg_type: value.arg_type.into(),
        }
    }
}

impl From<djls_conf::IntermediateTagDef> for IntermediateTag {
    fn from(value: djls_conf::IntermediateTagDef) -> Self {
        IntermediateTag {
            name: value.name,
            // Note: IntermediateTagDef has args field but IntermediateTag doesn't
            // This is intentional - we don't support args on intermediate tags yet
        }
    }
}

impl From<djls_conf::EndTagDef> for EndTag {
    fn from(value: djls_conf::EndTagDef) -> Self {
        EndTag {
            name: value.name,
            optional: value.optional,
            args: value.args.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<djls_conf::TagSpecDef> for TagSpec {
    fn from(value: djls_conf::TagSpecDef) -> Self {
        TagSpec {
            name: Some(value.name),
            end_tag: value.end_tag.map(Into::into),
            intermediate_tags: if value.intermediate_tags.is_empty() {
                None
            } else {
                Some(
                    value
                        .intermediate_tags
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                )
            },
            args: value.args.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<&djls_conf::Settings> for TagSpecs {
    fn from(settings: &djls_conf::Settings) -> Self {
        // Start with built-in specs
        let mut specs = crate::templatetags::django_builtin_specs();

        // Convert and merge user-defined tagspecs
        let mut user_specs = HashMap::new();
        for tagspec_def in settings.tagspecs() {
            // Clone because we're consuming the tagspec_def in the conversion
            let tagspec: TagSpec = tagspec_def.clone().into();
            if let Some(name) = &tagspec.name {
                user_specs.insert(name.clone(), tagspec);
            }
        }

        // Merge user specs into built-in specs (user specs override built-ins)
        if !user_specs.is_empty() {
            specs.merge(TagSpecs::new(user_specs));
        }

        specs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a small test TagSpecs
    fn create_test_specs() -> TagSpecs {
        let mut specs = HashMap::new();

        // Add a simple single tag
        specs.insert(
            "csrf_token".to_string(),
            TagSpec {
                name: Some("csrf_token".to_string()),
                end_tag: None,
                intermediate_tags: None,
                args: vec![],
            },
        );

        // Add a block tag with intermediates
        specs.insert(
            "if".to_string(),
            TagSpec {
                name: Some("if".to_string()),
                end_tag: Some(EndTag {
                    name: "endif".to_string(),
                    optional: false,
                    args: vec![],
                }),
                intermediate_tags: Some(vec![
                    IntermediateTag {
                        name: "elif".to_string(),
                    },
                    IntermediateTag {
                        name: "else".to_string(),
                    },
                ]),
                args: vec![],
            },
        );

        // Add another block tag with different intermediate
        specs.insert(
            "for".to_string(),
            TagSpec {
                name: Some("for".to_string()),
                end_tag: Some(EndTag {
                    name: "endfor".to_string(),
                    optional: false,
                    args: vec![],
                }),
                intermediate_tags: Some(vec![
                    IntermediateTag {
                        name: "empty".to_string(),
                    },
                    IntermediateTag {
                        name: "else".to_string(),
                    }, // Note: else is shared
                ]),
                args: vec![],
            },
        );

        // Add a block tag without intermediates
        specs.insert(
            "block".to_string(),
            TagSpec {
                name: Some("block".to_string()),
                end_tag: Some(EndTag {
                    name: "endblock".to_string(),
                    optional: false,
                    args: vec![Arg {
                        name: "name".to_string(),
                        required: false,
                        arg_type: ArgType::Simple(SimpleArgType::Variable),
                    }],
                }),
                intermediate_tags: None,
                args: vec![],
            },
        );

        TagSpecs::new(specs)
    }

    #[test]
    fn test_get() {
        let specs = create_test_specs();

        // Test get with existing keys
        assert!(specs.get("if").is_some());
        assert!(specs.get("for").is_some());
        assert!(specs.get("csrf_token").is_some());
        assert!(specs.get("block").is_some());

        // Test get with non-existing key
        assert!(specs.get("nonexistent").is_none());

        // Verify the content is correct
        let if_spec = specs.get("if").unwrap();
        assert_eq!(if_spec.name, Some("if".to_string()));
    }

    #[test]
    fn test_iter() {
        let specs = create_test_specs();

        let count = specs.iter().count();
        assert_eq!(count, 4);

        let mut found_keys: Vec<String> = specs.iter().map(|(k, _)| k.clone()).collect();
        found_keys.sort();

        let mut expected_keys = ["block", "csrf_token", "for", "if"];
        expected_keys.sort_unstable();

        assert_eq!(
            found_keys,
            expected_keys
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_find_opener_for_closer() {
        let specs = create_test_specs();

        assert_eq!(
            specs.find_opener_for_closer("endif"),
            Some("if".to_string())
        );
        assert_eq!(
            specs.find_opener_for_closer("endfor"),
            Some("for".to_string())
        );
        assert_eq!(
            specs.find_opener_for_closer("endblock"),
            Some("block".to_string())
        );

        assert_eq!(specs.find_opener_for_closer("endnonexistent"), None);

        assert_eq!(specs.find_opener_for_closer("if"), None);
    }

    #[test]
    fn test_get_end_spec_for_closer() {
        let specs = create_test_specs();

        let endif_spec = specs.get_end_spec_for_closer("endif").unwrap();
        assert_eq!(endif_spec.name, "endif");
        assert!(!endif_spec.optional);
        assert_eq!(endif_spec.args.len(), 0);

        let endblock_spec = specs.get_end_spec_for_closer("endblock").unwrap();
        assert_eq!(endblock_spec.name, "endblock");
        assert_eq!(endblock_spec.args.len(), 1);
        assert_eq!(endblock_spec.args[0].name, "name");

        assert!(specs.get_end_spec_for_closer("endnonexistent").is_none());
    }

    #[test]
    fn test_is_opener() {
        let specs = create_test_specs();

        // Tags with end tags are openers
        assert!(specs.is_opener("if"));
        assert!(specs.is_opener("for"));
        assert!(specs.is_opener("block"));

        // Single tags are not openers
        assert!(!specs.is_opener("csrf_token"));

        // Non-existent tags are not openers
        assert!(!specs.is_opener("nonexistent"));

        // Closer tags themselves are not openers
        assert!(!specs.is_opener("endif"));
    }

    #[test]
    fn test_is_intermediate() {
        let specs = create_test_specs();

        // Test valid intermediate tags
        assert!(specs.is_intermediate("elif"));
        assert!(specs.is_intermediate("else")); // Shared by if and for
        assert!(specs.is_intermediate("empty"));

        // Test non-intermediate tags
        assert!(!specs.is_intermediate("if"));
        assert!(!specs.is_intermediate("for"));
        assert!(!specs.is_intermediate("csrf_token"));
        assert!(!specs.is_intermediate("endif"));

        // Test non-existent tag
        assert!(!specs.is_intermediate("nonexistent"));
    }

    #[test]
    fn test_is_closer() {
        let specs = create_test_specs();

        // Test valid closer tags
        assert!(specs.is_closer("endif"));
        assert!(specs.is_closer("endfor"));
        assert!(specs.is_closer("endblock"));

        // Test non-closer tags
        assert!(!specs.is_closer("if"));
        assert!(!specs.is_closer("for"));
        assert!(!specs.is_closer("csrf_token"));
        assert!(!specs.is_closer("elif"));
        assert!(!specs.is_closer("else"));

        // Test non-existent tag
        assert!(!specs.is_closer("nonexistent"));
    }

    #[test]
    fn test_get_parent_tags_for_intermediate() {
        let specs = create_test_specs();

        // Test intermediate with single parent
        let elif_parents = specs.get_parent_tags_for_intermediate("elif");
        assert_eq!(elif_parents.len(), 1);
        assert!(elif_parents.contains(&"if".to_string()));

        // Test intermediate with multiple parents (else is shared)
        let mut else_parents = specs.get_parent_tags_for_intermediate("else");
        else_parents.sort();
        assert_eq!(else_parents.len(), 2);
        assert!(else_parents.contains(&"if".to_string()));
        assert!(else_parents.contains(&"for".to_string()));

        // Test intermediate with single parent
        let empty_parents = specs.get_parent_tags_for_intermediate("empty");
        assert_eq!(empty_parents.len(), 1);
        assert!(empty_parents.contains(&"for".to_string()));

        // Test non-intermediate tag
        let if_parents = specs.get_parent_tags_for_intermediate("if");
        assert_eq!(if_parents.len(), 0);

        // Test non-existent tag
        let nonexistent_parents = specs.get_parent_tags_for_intermediate("nonexistent");
        assert_eq!(nonexistent_parents.len(), 0);
    }

    #[test]
    fn test_merge() {
        let mut specs1 = create_test_specs();

        // Create another TagSpecs with some overlapping and some new tags
        let mut specs2_map = HashMap::new();

        // Add a new tag
        specs2_map.insert(
            "custom".to_string(),
            TagSpec {
                name: Some("custom".to_string()),
                end_tag: None,
                intermediate_tags: None,
                args: vec![],
            },
        );

        // Override an existing tag (if) with different structure
        specs2_map.insert(
            "if".to_string(),
            TagSpec {
                name: Some("if".to_string()),
                end_tag: Some(EndTag {
                    name: "endif".to_string(),
                    optional: true, // Changed to optional
                    args: vec![],
                }),
                intermediate_tags: None, // Removed intermediates
                args: vec![],
            },
        );

        let specs2 = TagSpecs::new(specs2_map);

        // Merge specs2 into specs1
        let result = specs1.merge(specs2);

        // Check that merge returns self for chaining
        assert!(std::ptr::eq(result, std::ptr::from_ref(&specs1)));

        // Check that new tag was added
        assert!(specs1.get("custom").is_some());

        // Check that existing tag was overwritten
        let if_spec = specs1.get("if").unwrap();
        assert!(if_spec.end_tag.as_ref().unwrap().optional); // Should be optional now
        assert!(if_spec.intermediate_tags.is_none()); // Should have no intermediates

        // Check that unaffected tags remain
        assert!(specs1.get("for").is_some());
        assert!(specs1.get("csrf_token").is_some());
        assert!(specs1.get("block").is_some());

        // Total count should be 5 (original 4 + 1 new)
        assert_eq!(specs1.iter().count(), 5);
    }

    #[test]
    fn test_merge_empty() {
        let mut specs = create_test_specs();
        let original_count = specs.iter().count();

        // Merge with empty TagSpecs
        specs.merge(TagSpecs::new(HashMap::new()));

        // Should remain unchanged
        assert_eq!(specs.iter().count(), original_count);
    }

    #[test]
    fn test_conversion_from_conf_types() {
        // Test SimpleArgTypeDef -> SimpleArgType conversion
        assert_eq!(
            SimpleArgType::from(djls_conf::SimpleArgTypeDef::Variable),
            SimpleArgType::Variable
        );
        assert_eq!(
            SimpleArgType::from(djls_conf::SimpleArgTypeDef::Literal),
            SimpleArgType::Literal
        );

        // Test ArgTypeDef -> ArgType conversion
        let simple_arg = djls_conf::ArgTypeDef::Simple(djls_conf::SimpleArgTypeDef::String);
        assert!(matches!(
            ArgType::from(simple_arg),
            ArgType::Simple(SimpleArgType::String)
        ));

        let choice_arg = djls_conf::ArgTypeDef::Choice {
            choice: vec!["on".to_string(), "off".to_string()],
        };
        if let ArgType::Choice { choice } = ArgType::from(choice_arg) {
            assert_eq!(choice, vec!["on".to_string(), "off".to_string()]);
        } else {
            panic!("Expected Choice variant");
        }

        // Test TagArgDef -> Arg conversion
        let tag_arg_def = djls_conf::TagArgDef {
            name: "test_arg".to_string(),
            required: true,
            arg_type: djls_conf::ArgTypeDef::Simple(djls_conf::SimpleArgTypeDef::Variable),
        };
        let arg = Arg::from(tag_arg_def);
        assert_eq!(arg.name, "test_arg");
        assert!(arg.required);
        assert!(matches!(
            arg.arg_type,
            ArgType::Simple(SimpleArgType::Variable)
        ));

        // Test EndTagDef -> EndTag conversion
        let end_tag_def = djls_conf::EndTagDef {
            name: "endtest".to_string(),
            optional: true,
            args: vec![],
        };
        let end_tag = EndTag::from(end_tag_def);
        assert_eq!(end_tag.name, "endtest");
        assert!(end_tag.optional);
        assert_eq!(end_tag.args.len(), 0);

        // Test IntermediateTagDef -> IntermediateTag conversion
        let intermediate_def = djls_conf::IntermediateTagDef {
            name: "elif".to_string(),
            args: vec![], // These are ignored in conversion
        };
        let intermediate = IntermediateTag::from(intermediate_def);
        assert_eq!(intermediate.name, "elif");

        // Test full TagSpecDef -> TagSpec conversion
        let tagspec_def = djls_conf::TagSpecDef {
            name: "custom".to_string(),
            module: "myapp.templatetags".to_string(), // Note: module is ignored in conversion
            end_tag: Some(djls_conf::EndTagDef {
                name: "endcustom".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: vec![djls_conf::IntermediateTagDef {
                name: "branch".to_string(),
                args: vec![],
            }],
            args: vec![],
        };
        let tagspec = TagSpec::from(tagspec_def);
        assert_eq!(tagspec.name, Some("custom".to_string()));
        assert!(tagspec.end_tag.is_some());
        assert_eq!(tagspec.end_tag.as_ref().unwrap().name, "endcustom");
        assert!(tagspec.intermediate_tags.is_some());
        assert_eq!(tagspec.intermediate_tags.as_ref().unwrap().len(), 1);
        assert_eq!(
            tagspec.intermediate_tags.as_ref().unwrap()[0].name,
            "branch"
        );
    }

    #[test]
    fn test_conversion_from_settings() {
        use std::fs;

        // Test case 1: Empty settings gives built-in specs
        let dir = tempfile::TempDir::new().unwrap();
        let settings = djls_conf::Settings::new(dir.path()).unwrap();
        let specs = TagSpecs::from(&settings);

        // Should have built-in specs
        assert!(specs.get("if").is_some());
        assert!(specs.get("for").is_some());
        assert!(specs.get("block").is_some());

        // Test case 2: Settings with user-defined tagspecs
        let dir = tempfile::TempDir::new().unwrap();
        let config_content = r#"
[[tagspecs]]
name = "mytag"
module = "myapp.templatetags.custom"
end_tag = { name = "endmytag", optional = false }
intermediate_tags = [{ name = "mybranch" }]
args = [
    { name = "arg1", type = "variable", required = true },
    { name = "arg2", type = { choice = ["on", "off"] }, required = false }
]

[[tagspecs]]
name = "if"
module = "myapp.overrides"
end_tag = { name = "endif", optional = true }
"#;
        fs::write(dir.path().join("djls.toml"), config_content).unwrap();

        let settings = djls_conf::Settings::new(dir.path()).unwrap();
        let specs = TagSpecs::from(&settings);

        // Should have built-in specs
        assert!(specs.get("for").is_some()); // Unaffected built-in
        assert!(specs.get("block").is_some()); // Unaffected built-in

        // Should have user-defined custom tag
        let mytag = specs.get("mytag").expect("mytag should be present");
        assert_eq!(mytag.name, Some("mytag".to_string()));
        assert_eq!(mytag.end_tag.as_ref().unwrap().name, "endmytag");
        assert!(!mytag.end_tag.as_ref().unwrap().optional);
        assert_eq!(mytag.intermediate_tags.as_ref().unwrap().len(), 1);
        assert_eq!(
            mytag.intermediate_tags.as_ref().unwrap()[0].name,
            "mybranch"
        );
        assert_eq!(mytag.args.len(), 2);
        assert_eq!(mytag.args[0].name, "arg1");
        assert!(mytag.args[0].required);
        assert_eq!(mytag.args[1].name, "arg2");
        assert!(!mytag.args[1].required);

        // Should have overridden built-in "if" tag
        let if_tag = specs.get("if").expect("if tag should be present");
        assert!(if_tag.end_tag.as_ref().unwrap().optional); // Changed to optional
                                                            // Note: The built-in if tag has intermediate tags, but the override doesn't specify them
                                                            // The override completely replaces the built-in
        assert!(
            if_tag.intermediate_tags.is_none()
                || if_tag.intermediate_tags.as_ref().unwrap().is_empty()
        );
    }
}
