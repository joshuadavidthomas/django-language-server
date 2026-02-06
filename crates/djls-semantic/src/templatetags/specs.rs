use std::borrow::Cow;
use std::borrow::Cow::Borrowed as B;
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

#[derive(Clone, Debug, Default, PartialEq)]
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

    /// Get the intermediate tag spec for a given intermediate tag
    #[must_use]
    pub fn get_intermediate_spec(&self, tag_name: &str) -> Option<&IntermediateTag> {
        self.0.values().find_map(|spec| {
            spec.intermediate_tags
                .iter()
                .find(|it| it.name.as_ref() == tag_name)
        })
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

#[derive(Debug, Clone, PartialEq)]
pub struct TagSpec {
    pub module: S,
    pub end_tag: Option<EndTag>,
    pub intermediate_tags: L<IntermediateTag>,
    /// Whether this is an opaque block (like verbatim/comment — no inner parsing)
    pub opaque: bool,
    /// Validation rules from Python AST extraction.
    /// Evaluated by `rule_evaluation::evaluate_extracted_rules`.
    pub extracted_rules: Vec<djls_extraction::ExtractedRule>,
}

impl TagSpec {
    /// Merge extracted validation rules into this spec.
    ///
    /// Only populates if not already set (first source wins).
    pub fn merge_extracted_rules(&mut self, rules: &[djls_extraction::ExtractedRule]) {
        if self.extracted_rules.is_empty() {
            self.extracted_rules.extend_from_slice(rules);
        }
    }

    /// Merge an extracted block spec into this spec.
    ///
    /// Persists end tag, intermediate tags, and opaque flag from extraction.
    pub fn merge_block_spec(&mut self, block: &djls_extraction::BlockTagSpec) {
        self.opaque = block.opaque;

        if self.end_tag.is_none() {
            if let Some(ref end) = block.end_tag {
                self.end_tag = Some(EndTag {
                    name: end.clone().into(),
                    required: true,
                });
            }
        }

        if self.intermediate_tags.is_empty() && !block.intermediate_tags.is_empty() {
            self.intermediate_tags = block
                .intermediate_tags
                .iter()
                .map(|it| IntermediateTag {
                    name: it.name.clone().into(),
                })
                .collect::<Vec<_>>()
                .into();
        }
    }

    /// Create a `TagSpec` from extraction results.
    #[must_use]
    pub fn from_extraction(module_path: &str, tag: &djls_extraction::ExtractedTag) -> Self {
        let mut spec = TagSpec {
            module: module_path.to_string().into(),
            end_tag: None,
            intermediate_tags: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        };

        spec.merge_extracted_rules(&tag.rules);
        if let Some(ref block_spec) = tag.block_spec {
            spec.merge_block_spec(block_spec);
        }

        spec
    }
}




#[derive(Debug, Clone, PartialEq)]
pub struct EndTag {
    pub name: S,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IntermediateTag {
    pub name: S,
}

#[cfg(test)]
mod tests {
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
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        // Add a block tag with intermediates
        specs.insert(
            "if".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endif".into(),
                    required: true,
                }),
                intermediate_tags: Cow::Owned(vec![
                    IntermediateTag {
                        name: "elif".into(),
                    },
                    IntermediateTag {
                        name: "else".into(),
                    },
                ]),
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        // Add another block tag with different intermediate
        specs.insert(
            "for".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endfor".into(),
                    required: true,
                }),
                intermediate_tags: Cow::Owned(vec![
                    IntermediateTag {
                        name: "empty".into(),
                    },
                    IntermediateTag {
                        name: "else".into(),
                    }, // Note: else is shared
                ]),
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        // Add a block tag without intermediates
        specs.insert(
            "block".to_string(),
            TagSpec {
                module: "django.template.loader_tags".into(),
                end_tag: Some(EndTag {
                    name: "endblock".into(),
                    required: true,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: Vec::new(),
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
        assert!(endif_spec.required);

        let endblock_spec = specs.get_end_spec_for_closer("endblock").unwrap();
        assert_eq!(endblock_spec.name.as_ref(), "endblock");

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
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        // Override an existing tag (if) with different structure
        specs2_map.insert(
            "if".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endif".into(),
                    required: false, // Changed to not required
                }),
                intermediate_tags: Cow::Borrowed(&[]), // Removed intermediates
                opaque: false,
                extracted_rules: Vec::new(),
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
        assert!(!if_spec.end_tag.as_ref().unwrap().required); // Should not be required now
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
    fn test_merge_block_spec_preserves_existing_end_tag() {
        // Regression test: merge_block_spec must not overwrite an existing end_tag.
        let mut spec = TagSpec {
            module: "django.template.loader_tags".into(),
            end_tag: Some(EndTag {
                name: "endblock".into(),
                required: true,
            }),
            intermediate_tags: Cow::Borrowed(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        };

        let extracted_block = djls_extraction::BlockTagSpec {
            end_tag: Some("endblock".to_string()),
            intermediate_tags: vec![],
            opaque: false,
        };

        spec.merge_block_spec(&extracted_block);

        // The end_tag should still exist
        let end = spec.end_tag.as_ref().expect("end_tag should exist");
        assert_eq!(end.name.as_ref(), "endblock");
    }

    #[test]
    fn test_merge_block_spec_preserves_existing_intermediate_tags() {
        let mut spec = TagSpec {
            module: "django.template.defaulttags".into(),
            end_tag: Some(EndTag {
                name: "endif".into(),
                required: true,
            }),
            intermediate_tags: Cow::Owned(vec![
                IntermediateTag {
                    name: "elif".into(),
                },
                IntermediateTag {
                    name: "else".into(),
                },
            ]),
            opaque: false,
            extracted_rules: Vec::new(),
        };

        let extracted_block = djls_extraction::BlockTagSpec {
            end_tag: Some("endif".to_string()),
            intermediate_tags: vec![
                djls_extraction::IntermediateTagSpec {
                    name: "elif".to_string(),
                    repeatable: true,
                },
                djls_extraction::IntermediateTagSpec {
                    name: "else".to_string(),
                    repeatable: false,
                },
            ],
            opaque: false,
        };

        spec.merge_block_spec(&extracted_block);

        // Intermediate tags should be preserved (not replaced by extraction)
        assert_eq!(spec.intermediate_tags.len(), 2);
    }

    #[test]
    fn test_merge_block_spec_populates_when_empty() {
        // When no end_tag or intermediates exist, extraction data should populate them
        let mut spec = TagSpec {
            module: "myapp.tags".into(),
            end_tag: None,
            intermediate_tags: Cow::Borrowed(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        };

        let extracted_block = djls_extraction::BlockTagSpec {
            end_tag: Some("endcustom".to_string()),
            intermediate_tags: vec![djls_extraction::IntermediateTagSpec {
                name: "otherwise".to_string(),
                repeatable: false,
            }],
            opaque: true,
        };

        spec.merge_block_spec(&extracted_block);

        assert!(spec.opaque);
        let end = spec.end_tag.as_ref().expect("end_tag should be set");
        assert_eq!(end.name.as_ref(), "endcustom");
        assert_eq!(spec.intermediate_tags.len(), 1);
        assert_eq!(spec.intermediate_tags[0].name.as_ref(), "otherwise");
    }
}
