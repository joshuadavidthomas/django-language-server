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

#[derive(Clone, Debug, Default)]
pub struct TagSpecs(FxHashMap<String, TagSpec>);

impl PartialEq for TagSpecs {
    fn eq(&self, other: &Self) -> bool {
        if self.0.len() != other.0.len() {
            return false;
        }
        self.0.iter().all(|(k, v)| other.0.get(k) == Some(v))
    }
}

impl TagSpecs {
    #[must_use]
    pub fn new(specs: FxHashMap<String, TagSpec>) -> Self {
        TagSpecs(specs)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TagSpec> {
        self.0.get(name)
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// Merge another `TagSpecs` into this one.
    ///
    /// For each tag in `other`:
    /// - If the tag doesn't exist in `self`, it's inserted as-is.
    /// - If the tag exists in `self`, the specs are merged with `TagSpec::merge`.
    pub fn merge(&mut self, other: TagSpecs) {
        for (name, other_spec) in other.0 {
            if let Some(existing) = self.0.remove(&name) {
                let merged = existing.merge(&other_spec);
                self.0.insert(name, merged);
            } else {
                self.0.insert(name, other_spec);
            }
        }
    }

    /// Returns true if the tag is an "opener" (has an [`end_tag`]).
    #[must_use]
    pub fn is_opener(&self, name: &str) -> bool {
        self.get(name).is_some_and(|spec| spec.end_tag.is_some())
    }

    /// Returns true if the tag is a "closer" (its name matches some opener's [`end_tag`]).
    #[must_use]
    pub fn is_closer(&self, name: &str) -> bool {
        self.0.values().any(|spec| {
            spec.end_tag
                .as_ref()
                .is_some_and(|end| end.name.as_ref() == name)
        })
    }

    /// Returns true if the tag is an "intermediate" tag.
    #[must_use]
    pub fn is_intermediate(&self, name: &str) -> bool {
        self.0.values().any(|spec| {
            spec.intermediate_tags
                .iter()
                .any(|it| it.name.as_ref() == name)
        })
    }

    /// Get all parent tags for an intermediate tag.
    ///
    /// For example, `elif` returns `["if"]`, `else` returns `["if", "for"]`.
    #[must_use]
    pub fn get_parent_tags_for_intermediate(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .filter_map(|(parent_name, spec)| {
                if spec
                    .intermediate_tags
                    .iter()
                    .any(|it| it.name.as_ref() == name)
                {
                    Some(parent_name.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns the `EndTag` spec for a given closer tag name.
    #[must_use]
    pub fn get_end_spec_for_closer(&self, name: &str) -> Option<&EndTag> {
        self.0.values().find_map(|spec| {
            spec.end_tag.as_ref().filter(|end| end.name.as_ref() == name)
        })
    }

    /// Returns true if the tag creates an opaque block (like `verbatim` or `comment`).
    #[must_use]
    pub fn is_opaque(&self, name: &str) -> bool {
        self.get(name).is_some_and(|spec| spec.opaque)
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

impl IntoIterator for TagSpecs {
    type Item = (String, TagSpec);
    type IntoIter = IntoIter<String, TagSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a TagSpecs {
    type Item = (&'a String, &'a TagSpec);
    type IntoIter = Iter<'a, String, TagSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagSpec {
    pub module: S,
    pub end_tag: Option<EndTag>,
    pub intermediate_tags: L<IntermediateTag>,
    /// Whether this tag creates an opaque block (like verbatim/comment)
    pub opaque: bool,
    /// Validation rules from Python AST extraction.
    /// Evaluated by [`crate::rule_evaluation::evaluate_extracted_rules`].
    pub extracted_rules: Vec<djls_extraction::ExtractedRule>,
}

impl TagSpec {
    /// Merge another spec into this one.
    ///
    /// Used when multiple sources define the same tag (e.g., builtins + user config).
    /// The other spec takes precedence for most fields.
    #[must_use]
    pub fn merge(&self, other: &TagSpec) -> TagSpec {
        TagSpec {
            module: other.module.clone(),
            end_tag: other.end_tag.clone().or_else(|| self.end_tag.clone()),
            intermediate_tags: if other.intermediate_tags.is_empty() {
                self.intermediate_tags.clone()
            } else {
                other.intermediate_tags.clone()
            },
            opaque: other.opaque || self.opaque,
            extracted_rules: if other.extracted_rules.is_empty() {
                self.extracted_rules.clone()
            } else {
                other.extracted_rules.clone()
            },
        }
    }

    /// Merge extracted rules into this spec.
    ///
    /// Stores rules for evaluation by [`evaluate_extracted_rules`].
    /// Rules are appended to any existing rules.
    pub fn merge_extracted_rules(
        &mut self,
        rules: &[djls_extraction::ExtractedRule],
    ) {
        self.extracted_rules.extend_from_slice(rules);
    }

    /// Merge extracted block spec into this spec.
    ///
    /// Persists end tag + intermediate tags + opaque flag.
    pub fn merge_block_spec(&mut self, block: &djls_extraction::BlockTagSpec) {
        // Persist opaque flag
        self.opaque = block.opaque;

        // Update end tag if provided
        if let Some(ref end) = block.end_tag {
            self.end_tag = Some(EndTag {
                name: end.clone().into(),
                required: true,
            });
        }

        // Update intermediate tags if provided
        if !block.intermediate_tags.is_empty() {
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

    /// Create a new [`TagSpec`] from extraction results.
    #[must_use]
    pub fn from_extraction(
        module_path: &str,
        tag: &djls_extraction::ExtractedTag,
    ) -> Self {
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

        // Add a for tag that shares "else" with if
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
                    },
                ]),
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        TagSpecs::new(specs)
    }

    #[test]
    fn test_is_opener() {
        let specs = create_test_specs();

        // Test valid openers
        assert!(specs.is_opener("if"));
        assert!(specs.is_opener("for"));

        // Test non-openers
        assert!(!specs.is_opener("csrf_token"));
        assert!(!specs.is_opener("endif"));
        assert!(!specs.is_opener("elif"));

        // Test non-existent tag
        assert!(!specs.is_opener("nonexistent"));
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
        // Note: endblock is not tested because create_test_specs doesn't include block tag

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
                    required: true,
                }),
                intermediate_tags: Cow::Owned(vec![IntermediateTag {
                    name: "elseif".into(),
                }]),
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        let specs2 = TagSpecs::new(specs2_map);

        // Merge specs2 into specs1
        specs1.merge(specs2);

        // Verify new tag was added
        assert!(specs1.contains("custom"));

        // Verify if was merged (should now have elseif instead of elif/else)
        let if_spec = specs1.get("if").unwrap();
        assert_eq!(if_spec.intermediate_tags.len(), 1);
        assert_eq!(if_spec.intermediate_tags[0].name, "elseif");

        // Verify original tags are still there
        assert!(specs1.contains("for"));
        assert!(specs1.contains("csrf_token"));
    }
}
