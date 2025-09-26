use std::borrow::Cow;
use std::collections::hash_map::IntoIter;
use std::collections::hash_map::Iter;
use std::ops::Deref;
use std::ops::DerefMut;

use rustc_hash::FxHashMap;

pub type S<T = str> = Cow<'static, T>;
pub type L<T> = Cow<'static, [T]>;

#[allow(dead_code)]
pub enum TagType {
    Opener,
    Intermediate,
    Closer,
    Standalone,
}

#[allow(dead_code)]
impl TagType {
    #[must_use]
    pub fn for_name(name: &str, tag_specs: &TagSpecs) -> TagType {
        if tag_specs.is_opener(name) {
            TagType::Opener
        } else if tag_specs.is_closer(name) {
            TagType::Closer
        } else if tag_specs.is_intermediate(name) {
            TagType::Intermediate
        } else {
            TagType::Standalone
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TagSpecs(FxHashMap<String, TagSpec>);

impl TagSpecs {
    #[must_use]
    pub fn new(specs: FxHashMap<String, TagSpec>) -> Self {
        TagSpecs(specs)
    }

    /// Find the opener tag for a given closer tag
    #[must_use]
    pub fn find_opener_for_closer(&self, closer: &str) -> Option<String> {
        for (tag_name, spec) in &self.0 {
            if let Some(end_spec) = &spec.end_tag {
                if end_spec.name.as_ref() == closer {
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
                if end_spec.name.as_ref() == closer {
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
                .iter()
                .any(|tag| tag.name.as_ref() == name)
        })
    }

    #[must_use]
    pub fn is_closer(&self, name: &str) -> bool {
        self.0.values().any(|spec| {
            spec.end_tag
                .as_ref()
                .is_some_and(|end_tag| end_tag.name.as_ref() == name)
        })
    }

    /// Get the parent tags that can contain this intermediate tag
    #[must_use]
    pub fn get_parent_tags_for_intermediate(&self, intermediate: &str) -> Vec<String> {
        let mut parents = Vec::new();
        for (opener_name, spec) in &self.0 {
            if spec
                .intermediate_tags
                .iter()
                .any(|tag| tag.name.as_ref() == intermediate)
            {
                parents.push(opener_name.clone());
            }
        }
        parents
    }

    /// Merge another `TagSpecs` into this one, with the other taking precedence
    pub fn merge(&mut self, other: TagSpecs) -> &mut Self {
        self.0.extend(other.0);
        self
    }
}

impl Deref for TagSpecs {
    type Target = FxHashMap<String, TagSpec>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TagSpecs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> IntoIterator for &'a TagSpecs {
    type Item = (&'a String, &'a TagSpec);
    type IntoIter = Iter<'a, String, TagSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl IntoIterator for TagSpecs {
    type Item = (String, TagSpec);
    type IntoIter = IntoIter<String, TagSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl From<&djls_conf::Settings> for TagSpecs {
    fn from(settings: &djls_conf::Settings) -> Self {
        // Start with built-in specs
        let mut specs = crate::templatetags::django_builtin_specs();

        // Convert and merge user-defined tagspecs
        let mut user_specs = FxHashMap::default();
        for tagspec_def in settings.tagspecs() {
            // Clone because we're consuming the tagspec_def in the conversion
            let name = tagspec_def.name.clone();
            let tagspec: TagSpec = tagspec_def.clone().into();
            user_specs.insert(name, tagspec);
        }

        // Merge user specs into built-in specs (user specs override built-ins)
        if !user_specs.is_empty() {
            specs.merge(TagSpecs::new(user_specs));
        }

        specs
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagSpec {
    pub module: S,
    pub end_tag: Option<EndTag>,
    pub intermediate_tags: L<IntermediateTag>,
    pub args: L<TagArg>,
}

impl From<djls_conf::TagSpecDef> for TagSpec {
    fn from(value: djls_conf::TagSpecDef) -> Self {
        TagSpec {
            module: value.module.into(),
            end_tag: value.end_tag.map(Into::into),
            intermediate_tags: value
                .intermediate_tags
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>()
                .into(),
            args: value
                .args
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>()
                .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagArg {
    Var {
        name: S,
        required: bool,
    },
    String {
        name: S,
        required: bool,
    },
    Literal {
        lit: S,
        required: bool,
    },
    Expr {
        name: S,
        required: bool,
    },
    Assignment {
        name: S,
        required: bool,
    },
    VarArgs {
        name: S,
        required: bool,
    },
    Choice {
        name: S,
        required: bool,
        choices: L<S>,
    },
}

impl TagArg {
    #[must_use]
    pub fn name(&self) -> &S {
        match self {
            Self::Var { name, .. }
            | Self::String { name, .. }
            | Self::Expr { name, .. }
            | Self::Assignment { name, .. }
            | Self::VarArgs { name, .. }
            | Self::Choice { name, .. } => name,
            Self::Literal { lit, .. } => lit,
        }
    }

    #[must_use]
    pub fn is_required(&self) -> bool {
        match self {
            Self::Var { required, .. }
            | Self::String { required, .. }
            | Self::Literal { required, .. }
            | Self::Expr { required, .. }
            | Self::Assignment { required, .. }
            | Self::VarArgs { required, .. }
            | Self::Choice { required, .. } => *required,
        }
    }

    pub fn choice(name: impl Into<S>, required: bool, choices: impl Into<L<S>>) -> Self {
        Self::Choice {
            name: name.into(),
            required,
            choices: choices.into(),
        }
    }

    pub fn expr(name: impl Into<S>, required: bool) -> Self {
        Self::Expr {
            name: name.into(),
            required,
        }
    }

    pub fn literal(lit: impl Into<S>, required: bool) -> Self {
        Self::Literal {
            lit: lit.into(),
            required,
        }
    }

    pub fn string(name: impl Into<S>, required: bool) -> Self {
        Self::String {
            name: name.into(),
            required,
        }
    }

    pub fn var(name: impl Into<S>, required: bool) -> Self {
        Self::Var {
            name: name.into(),
            required,
        }
    }

    pub fn varargs(name: impl Into<S>, required: bool) -> Self {
        Self::VarArgs {
            name: name.into(),
            required,
        }
    }

    pub fn assignment(name: impl Into<S>, required: bool) -> Self {
        Self::Assignment {
            name: name.into(),
            required,
        }
    }
}

impl From<djls_conf::TagArgDef> for TagArg {
    fn from(value: djls_conf::TagArgDef) -> Self {
        match value.arg_type {
            djls_conf::ArgTypeDef::Simple(simple) => match simple {
                djls_conf::SimpleArgTypeDef::Literal => TagArg::Literal {
                    lit: value.name.into(),
                    required: value.required,
                },
                djls_conf::SimpleArgTypeDef::Variable => TagArg::Var {
                    name: value.name.into(),
                    required: value.required,
                },
                djls_conf::SimpleArgTypeDef::String => TagArg::String {
                    name: value.name.into(),
                    required: value.required,
                },
                djls_conf::SimpleArgTypeDef::Expression => TagArg::Expr {
                    name: value.name.into(),
                    required: value.required,
                },
                djls_conf::SimpleArgTypeDef::Assignment => TagArg::Assignment {
                    name: value.name.into(),
                    required: value.required,
                },
                djls_conf::SimpleArgTypeDef::VarArgs => TagArg::VarArgs {
                    name: value.name.into(),
                    required: value.required,
                },
            },
            djls_conf::ArgTypeDef::Choice { choice } => TagArg::Choice {
                name: value.name.into(),
                required: value.required,
                choices: choice
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<_>>()
                    .into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndTag {
    pub name: S,
    pub optional: bool,
    pub args: L<TagArg>,
}

impl From<djls_conf::EndTagDef> for EndTag {
    fn from(value: djls_conf::EndTagDef) -> Self {
        EndTag {
            name: value.name.into(),
            optional: value.optional,
            args: value
                .args
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>()
                .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IntermediateTag {
    pub name: S,
    pub args: L<TagArg>,
}

impl From<djls_conf::IntermediateTagDef> for IntermediateTag {
    fn from(value: djls_conf::IntermediateTagDef) -> Self {
        IntermediateTag {
            name: value.name.into(),
            args: value
                .args
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>()
                .into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use camino::Utf8Path;

    use super::*;

    // Helper function to create a small test TagSpecs
    fn create_test_specs() -> TagSpecs {
        let mut specs = FxHashMap::default();

        // Add a simple single tag
        specs.insert(
            "csrf_token".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Borrowed(&[]),
            },
        );

        // Add a block tag with intermediates
        specs.insert(
            "if".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endif".into(),
                    optional: false,
                    args: Cow::Borrowed(&[]),
                }),
                intermediate_tags: Cow::Owned(vec![
                    IntermediateTag {
                        name: "elif".into(),
                        args: Cow::Owned(vec![TagArg::expr("condition", true)]),
                    },
                    IntermediateTag {
                        name: "else".into(),
                        args: Cow::Borrowed(&[]),
                    },
                ]),
                args: Cow::Borrowed(&[]),
            },
        );

        // Add another block tag with different intermediate
        specs.insert(
            "for".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endfor".into(),
                    optional: false,
                    args: Cow::Borrowed(&[]),
                }),
                intermediate_tags: Cow::Owned(vec![
                    IntermediateTag {
                        name: "empty".into(),
                        args: Cow::Borrowed(&[]),
                    },
                    IntermediateTag {
                        name: "else".into(),
                        args: Cow::Borrowed(&[]),
                    }, // Note: else is shared
                ]),
                args: Cow::Borrowed(&[]),
            },
        );

        // Add a block tag without intermediates
        specs.insert(
            "block".to_string(),
            TagSpec {
                module: "django.template.loader_tags".into(),
                end_tag: Some(EndTag {
                    name: "endblock".into(),
                    optional: false,
                    args: Cow::Owned(vec![TagArg::Var {
                        name: "name".into(),
                        required: false,
                    }]),
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Borrowed(&[]),
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

        // Verify the content is correct - if tag should have an end tag
        let if_spec = specs.get("if").unwrap();
        assert!(if_spec.end_tag.is_some());
    }

    #[test]
    fn test_iter() {
        let specs = create_test_specs();

        let count = specs.len();
        assert_eq!(count, 4);

        let mut found_keys: Vec<String> = specs.keys().cloned().collect();
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
        assert_eq!(endif_spec.name.as_ref(), "endif");
        assert!(!endif_spec.optional);
        assert_eq!(endif_spec.args.len(), 0);

        let endblock_spec = specs.get_end_spec_for_closer("endblock").unwrap();
        assert_eq!(endblock_spec.name.as_ref(), "endblock");
        assert_eq!(endblock_spec.args.len(), 1);
        assert_eq!(endblock_spec.args[0].name().as_ref(), "name");

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
        let mut specs2_map = FxHashMap::default();

        // Add a new tag
        specs2_map.insert(
            "custom".to_string(),
            TagSpec {
                module: "custom.module".into(),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Borrowed(&[]),
            },
        );

        // Override an existing tag (if) with different structure
        specs2_map.insert(
            "if".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endif".into(),
                    optional: true, // Changed to optional
                    args: Cow::Borrowed(&[]),
                }),
                intermediate_tags: Cow::Borrowed(&[]), // Removed intermediates
                args: Cow::Borrowed(&[]),
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
        assert!(if_spec.intermediate_tags.is_empty()); // Should have no intermediates

        // Check that unaffected tags remain
        assert!(specs1.get("for").is_some());
        assert!(specs1.get("csrf_token").is_some());
        assert!(specs1.get("block").is_some());

        // Total count should be 5 (original 4 + 1 new)
        assert_eq!(specs1.len(), 5);
    }

    #[test]
    fn test_merge_empty() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        // Merge with empty TagSpecs
        specs.merge(TagSpecs::new(FxHashMap::default()));

        // Should remain unchanged
        assert_eq!(specs.len(), original_count);
    }

    #[test]
    fn test_conversion_from_conf_types() {
        // Test TagArgDef -> TagArg conversion for different arg types
        let string_arg_def = djls_conf::TagArgDef {
            name: "test".to_string(),
            required: true,
            arg_type: djls_conf::ArgTypeDef::Simple(djls_conf::SimpleArgTypeDef::String),
        };
        assert!(matches!(
            TagArg::from(string_arg_def),
            TagArg::String { .. }
        ));

        let choice_arg_def = djls_conf::TagArgDef {
            name: "mode".to_string(),
            required: false,
            arg_type: djls_conf::ArgTypeDef::Choice {
                choice: vec!["on".to_string(), "off".to_string()],
            },
        };
        if let TagArg::Choice { choices, .. } = TagArg::from(choice_arg_def) {
            assert_eq!(choices.len(), 2);
            assert_eq!(choices[0].as_ref(), "on");
            assert_eq!(choices[1].as_ref(), "off");
        } else {
            panic!("Expected Choice variant");
        }

        // Test TagArgDef -> TagArg conversion for Variable type
        let tag_arg_def = djls_conf::TagArgDef {
            name: "test_arg".to_string(),
            required: true,
            arg_type: djls_conf::ArgTypeDef::Simple(djls_conf::SimpleArgTypeDef::Variable),
        };
        let arg = TagArg::from(tag_arg_def);
        assert!(matches!(arg, TagArg::Var { .. }));
        if let TagArg::Var { name, required } = arg {
            assert_eq!(name.as_ref(), "test_arg");
            assert!(required);
        }

        // Test EndTagDef -> EndTag conversion
        let end_tag_def = djls_conf::EndTagDef {
            name: "endtest".to_string(),
            optional: true,
            args: vec![],
        };
        let end_tag = EndTag::from(end_tag_def);
        assert_eq!(end_tag.name.as_ref(), "endtest");
        assert!(end_tag.optional);
        assert_eq!(end_tag.args.len(), 0);

        // Test IntermediateTagDef -> IntermediateTag conversion
        let intermediate_def = djls_conf::IntermediateTagDef {
            name: "elif".to_string(),
            args: vec![djls_conf::TagArgDef {
                name: "condition".to_string(),
                required: true,
                arg_type: djls_conf::ArgTypeDef::Simple(djls_conf::SimpleArgTypeDef::Expression),
            }],
        };
        let intermediate = IntermediateTag::from(intermediate_def);
        assert_eq!(intermediate.name.as_ref(), "elif");
        assert_eq!(intermediate.args.len(), 1);
        assert_eq!(intermediate.args[0].name().as_ref(), "condition");

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
        // Name field was removed from TagSpec
        assert!(tagspec.end_tag.is_some());
        assert_eq!(tagspec.end_tag.as_ref().unwrap().name.as_ref(), "endcustom");
        assert_eq!(tagspec.intermediate_tags.len(), 1);
        assert_eq!(tagspec.intermediate_tags[0].name.as_ref(), "branch");
        assert_eq!(tagspec.intermediate_tags[0].args.len(), 0);
    }

    #[test]
    fn test_conversion_from_settings() {
        // Test case 1: Empty settings gives built-in specs
        let dir = tempfile::TempDir::new().unwrap();
        let settings = djls_conf::Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
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

        let settings = djls_conf::Settings::new(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
        let specs = TagSpecs::from(&settings);

        // Should have built-in specs
        assert!(specs.get("for").is_some()); // Unaffected built-in
        assert!(specs.get("block").is_some()); // Unaffected built-in

        // Should have user-defined custom tag
        let mytag = specs.get("mytag").expect("mytag should be present");
        // Name field was removed from TagSpec
        assert_eq!(mytag.end_tag.as_ref().unwrap().name.as_ref(), "endmytag");
        assert!(!mytag.end_tag.as_ref().unwrap().optional);
        assert_eq!(mytag.intermediate_tags.len(), 1);
        assert_eq!(mytag.intermediate_tags[0].name.as_ref(), "mybranch");
        assert_eq!(mytag.args.len(), 2);
        assert_eq!(mytag.args[0].name().as_ref(), "arg1");
        assert!(mytag.args[0].is_required());
        assert_eq!(mytag.args[1].name().as_ref(), "arg2");
        assert!(!mytag.args[1].is_required());

        // Should have overridden built-in "if" tag
        let if_tag = specs.get("if").expect("if tag should be present");
        assert!(if_tag.end_tag.as_ref().unwrap().optional); // Changed to optional
                                                            // Note: The built-in if tag has intermediate tags, but the override doesn't specify them
                                                            // The override completely replaces the built-in
        assert!(if_tag.intermediate_tags.is_empty());
    }
}
