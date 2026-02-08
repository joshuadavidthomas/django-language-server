use std::borrow::Cow;
use std::collections::hash_map::IntoIter;
use std::collections::hash_map::Iter;
use std::ops::Deref;
use std::ops::DerefMut;

use rustc_hash::FxHashMap;

pub type S<T = str> = Cow<'static, T>;
pub type L<T> = Cow<'static, [T]>;

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

    /// Merge extraction results into tag specs.
    ///
    /// Block specs from extraction override existing end-tag/intermediate info.
    /// This enriches the handcoded `builtins.rs` defaults with information
    /// extracted from actual Python source code.
    pub fn merge_extraction_results(
        &mut self,
        extraction: &djls_extraction::ExtractionResult,
    ) -> &mut Self {
        // Merge block specs (end tags, intermediates, opaque)
        for (key, block_spec) in &extraction.block_specs {
            if key.kind != djls_extraction::SymbolKind::Tag {
                continue;
            }
            if let Some(spec) = self.0.get_mut(&key.name) {
                // Override end_tag from extraction
                if let Some(end_tag_name) = &block_spec.end_tag {
                    spec.end_tag = Some(EndTag {
                        name: end_tag_name.clone().into(),
                        required: true,
                    });
                }
                // Override intermediates from extraction
                spec.intermediate_tags = if block_spec.intermediates.is_empty() {
                    std::borrow::Cow::Borrowed(&[])
                } else {
                    std::borrow::Cow::Owned(
                        block_spec
                            .intermediates
                            .iter()
                            .map(|name| IntermediateTag {
                                name: name.clone().into(),
                            })
                            .collect(),
                    )
                };
                // Propagate opaque flag from extraction
                spec.opaque = block_spec.opaque;
            } else {
                // Tag not yet in specs — create a new entry from extraction
                let end_tag = block_spec.end_tag.as_ref().map(|name| EndTag {
                    name: name.clone().into(),
                    required: true,
                });
                let intermediate_tags: Vec<IntermediateTag> = block_spec
                    .intermediates
                    .iter()
                    .map(|name| IntermediateTag {
                        name: name.clone().into(),
                    })
                    .collect();
                self.0.insert(
                    key.name.clone(),
                    TagSpec {
                        module: key.registration_module.clone().into(),
                        end_tag,
                        intermediate_tags: std::borrow::Cow::Owned(intermediate_tags),
                        opaque: block_spec.opaque,
                        extracted_rules: None,
                    },
                );
            }
        }

        // Merge tag rules (argument validation constraints from extraction)
        for (key, tag_rule) in &extraction.tag_rules {
            if key.kind != djls_extraction::SymbolKind::Tag {
                continue;
            }

            if let Some(spec) = self.0.get_mut(&key.name) {
                spec.extracted_rules = Some(tag_rule.clone());
            } else {
                // Tag not yet in specs — create a minimal entry with extracted rules
                self.0.insert(
                    key.name.clone(),
                    TagSpec {
                        module: key.registration_module.clone().into(),
                        end_tag: None,
                        intermediate_tags: std::borrow::Cow::Borrowed(&[]),
                        opaque: false,
                        extracted_rules: Some(tag_rule.clone()),
                    },
                );
            }
        }
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

/// Specification for a Django template tag's structure and validation rules.
///
/// Argument validation is handled by `extracted_rules` (derived from Python AST
/// extraction). Argument structure for completions/snippets is accessed via
/// `extracted_rules.extracted_args`.
#[derive(Debug, Clone, PartialEq)]
pub struct TagSpec {
    pub module: S,
    pub end_tag: Option<EndTag>,
    pub intermediate_tags: L<IntermediateTag>,
    pub opaque: bool,
    /// Extraction-derived validation rules from Python AST analysis.
    ///
    /// When present, provides argument validation (S117 diagnostics) and
    /// argument structure for completions/snippets via `extracted_args`.
    pub extracted_rules: Option<djls_extraction::TagRule>,
}

/// Specification for a closing tag (e.g., `{% endfor %}`, `{% endblock %}`).
#[derive(Debug, Clone, PartialEq)]
pub struct EndTag {
    pub name: S,
    pub required: bool,
}

/// Specification for an intermediate tag (e.g., `{% else %}`, `{% elif %}`).
#[derive(Debug, Clone, PartialEq)]
pub struct IntermediateTag {
    pub name: S,
}

/// Returns minimal Django tag specs for use in test databases.
///
/// Provides block structure (end tags, intermediates, opaque flags) for
/// standard Django tags. Does NOT include extracted rules — tests that
/// need argument validation should construct specs with `extracted_rules`
/// explicitly or use extraction on Python source.
#[cfg(test)]
#[allow(clippy::similar_names)]
pub(crate) fn test_tag_specs() -> TagSpecs {
    use std::borrow::Cow::Borrowed as B;

    let dt = "django.template.defaulttags";
    let lt = "django.template.loader_tags";
    let i18n = "django.templatetags.i18n";
    let cache = "django.templatetags.cache";
    let l10n = "django.templatetags.l10n";
    let tz = "django.templatetags.tz";
    let st = "django.templatetags.static";

    let mut specs = FxHashMap::default();

    let simple = |module: &'static str| TagSpec {
        module: B(module),
        end_tag: None,
        intermediate_tags: B(&[]),
        opaque: false,
        extracted_rules: None,
    };

    let block = |module: &'static str,
                 end: &'static str,
                 intermediates: Vec<IntermediateTag>,
                 opaque: bool| TagSpec {
        module: B(module),
        end_tag: Some(EndTag {
            name: B(end),
            required: true,
        }),
        intermediate_tags: Cow::Owned(intermediates),
        opaque,
        extracted_rules: None,
    };

    let im = |name: &'static str| IntermediateTag { name: B(name) };

    // defaulttags
    specs.insert(
        "autoescape".into(),
        block(dt, "endautoescape", vec![], false),
    );
    specs.insert("comment".into(), block(dt, "endcomment", vec![], true));
    specs.insert("csrf_token".into(), simple(dt));
    specs.insert("cycle".into(), simple(dt));
    specs.insert("debug".into(), simple(dt));
    specs.insert("filter".into(), block(dt, "endfilter", vec![], false));
    specs.insert("firstof".into(), simple(dt));
    specs.insert("for".into(), block(dt, "endfor", vec![im("empty")], false));
    specs.insert(
        "if".into(),
        block(dt, "endif", vec![im("elif"), im("else")], false),
    );
    specs.insert(
        "ifchanged".into(),
        block(dt, "endifchanged", vec![im("else")], false),
    );
    specs.insert("load".into(), simple(dt));
    specs.insert("lorem".into(), simple(dt));
    specs.insert("now".into(), simple(dt));
    specs.insert("regroup".into(), simple(dt));
    specs.insert("spaceless".into(), block(dt, "endspaceless", vec![], false));
    specs.insert("templatetag".into(), simple(dt));
    specs.insert("url".into(), simple(dt));
    specs.insert("verbatim".into(), block(dt, "endverbatim", vec![], true));
    specs.insert("widthratio".into(), simple(dt));
    specs.insert("with".into(), block(dt, "endwith", vec![], false));

    // loader_tags
    specs.insert("block".into(), block(lt, "endblock", vec![], false));
    specs.insert("extends".into(), simple(lt));
    specs.insert("include".into(), simple(lt));

    // i18n
    specs.insert(
        "blocktrans".into(),
        block(i18n, "endblocktrans", vec![im("plural")], false),
    );
    specs.insert(
        "blocktranslate".into(),
        block(i18n, "endblocktranslate", vec![im("plural")], false),
    );
    specs.insert("trans".into(), simple(i18n));
    specs.insert("translate".into(), simple(i18n));

    // cache
    specs.insert("cache".into(), block(cache, "endcache", vec![], false));

    // l10n
    specs.insert("localize".into(), block(l10n, "endlocalize", vec![], false));

    // static
    specs.insert("static".into(), simple(st));

    // tz
    specs.insert("localtime".into(), block(tz, "endlocaltime", vec![], false));
    specs.insert("timezone".into(), block(tz, "endtimezone", vec![], false));

    TagSpecs::new(specs)
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
                extracted_rules: None,
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
                extracted_rules: None,
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
                extracted_rules: None,
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
                extracted_rules: None,
            },
        );

        TagSpecs::new(specs)
    }

    #[test]
    fn test_get() {
        let specs = create_test_specs();

        assert!(specs.get("if").is_some());
        assert!(specs.get("for").is_some());
        assert!(specs.get("csrf_token").is_some());
        assert!(specs.get("block").is_some());
        assert!(specs.get("nonexistent").is_none());

        let if_spec = specs.get("if").unwrap();
        assert!(if_spec.end_tag.is_some());
    }

    #[test]
    fn test_iter() {
        let specs = create_test_specs();
        assert_eq!(specs.len(), 4);

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

        assert!(specs.get_end_spec_for_closer("endnonexistent").is_none());
    }

    #[test]
    fn test_is_opener() {
        let specs = create_test_specs();

        assert!(specs.is_opener("if"));
        assert!(specs.is_opener("for"));
        assert!(specs.is_opener("block"));
        assert!(!specs.is_opener("csrf_token"));
        assert!(!specs.is_opener("nonexistent"));
        assert!(!specs.is_opener("endif"));
    }

    #[test]
    fn test_is_intermediate() {
        let specs = create_test_specs();

        assert!(specs.is_intermediate("elif"));
        assert!(specs.is_intermediate("else"));
        assert!(specs.is_intermediate("empty"));
        assert!(!specs.is_intermediate("if"));
        assert!(!specs.is_intermediate("for"));
        assert!(!specs.is_intermediate("csrf_token"));
        assert!(!specs.is_intermediate("endif"));
        assert!(!specs.is_intermediate("nonexistent"));
    }

    #[test]
    fn test_is_closer() {
        let specs = create_test_specs();

        assert!(specs.is_closer("endif"));
        assert!(specs.is_closer("endfor"));
        assert!(specs.is_closer("endblock"));
        assert!(!specs.is_closer("if"));
        assert!(!specs.is_closer("for"));
        assert!(!specs.is_closer("csrf_token"));
        assert!(!specs.is_closer("elif"));
        assert!(!specs.is_closer("nonexistent"));
    }

    #[test]
    fn test_get_parent_tags_for_intermediate() {
        let specs = create_test_specs();

        let elif_parents = specs.get_parent_tags_for_intermediate("elif");
        assert_eq!(elif_parents.len(), 1);
        assert!(elif_parents.contains(&"if".to_string()));

        let mut else_parents = specs.get_parent_tags_for_intermediate("else");
        else_parents.sort();
        assert_eq!(else_parents.len(), 2);
        assert!(else_parents.contains(&"if".to_string()));
        assert!(else_parents.contains(&"for".to_string()));

        let empty_parents = specs.get_parent_tags_for_intermediate("empty");
        assert_eq!(empty_parents.len(), 1);
        assert!(empty_parents.contains(&"for".to_string()));

        assert_eq!(specs.get_parent_tags_for_intermediate("if").len(), 0);
        assert_eq!(
            specs.get_parent_tags_for_intermediate("nonexistent").len(),
            0
        );
    }

    #[test]
    fn test_merge() {
        let mut specs1 = create_test_specs();

        let mut specs2_map = FxHashMap::default();
        specs2_map.insert(
            "custom".to_string(),
            TagSpec {
                module: "custom.module".into(),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: None,
            },
        );
        specs2_map.insert(
            "if".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endif".into(),
                    required: false,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: None,
            },
        );

        let specs2 = TagSpecs::new(specs2_map);
        let result = specs1.merge(specs2);
        assert!(std::ptr::eq(result, std::ptr::from_ref(&specs1)));

        assert!(specs1.get("custom").is_some());
        let if_spec = specs1.get("if").unwrap();
        assert!(!if_spec.end_tag.as_ref().unwrap().required);
        assert!(if_spec.intermediate_tags.is_empty());
        assert!(specs1.get("for").is_some());
        assert!(specs1.get("csrf_token").is_some());
        assert!(specs1.get("block").is_some());
        assert_eq!(specs1.len(), 5);
    }

    #[test]
    fn test_merge_empty() {
        let mut specs = create_test_specs();
        let original_count = specs.len();
        specs.merge(TagSpecs::new(FxHashMap::default()));
        assert_eq!(specs.len(), original_count);
    }

    #[test]
    fn test_merge_extraction_results_overrides_existing() {
        let mut specs = create_test_specs();

        assert!(specs.get("if").unwrap().end_tag.is_some());
        assert_eq!(specs.get("if").unwrap().intermediate_tags.len(), 2);

        let mut extraction = djls_extraction::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_extraction::SymbolKey::tag("django.template.defaulttags", "if"),
            djls_extraction::BlockTagSpec {
                end_tag: Some("endif".to_string()),
                intermediates: vec!["elif".to_string(), "else".to_string(), "elseif".to_string()],
                opaque: false,
            },
        );

        specs.merge_extraction_results(&extraction);

        let if_spec = specs.get("if").unwrap();
        assert_eq!(if_spec.end_tag.as_ref().unwrap().name.as_ref(), "endif");
        assert_eq!(if_spec.intermediate_tags.len(), 3);
        assert!(if_spec
            .intermediate_tags
            .iter()
            .any(|t| t.name.as_ref() == "elseif"));
    }

    #[test]
    fn test_merge_extraction_results_adds_new_tag() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        let mut extraction = djls_extraction::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_extraction::SymbolKey::tag("myapp.templatetags.custom", "myblock"),
            djls_extraction::BlockTagSpec {
                end_tag: Some("endmyblock".to_string()),
                intermediates: vec!["mymiddle".to_string()],
                opaque: false,
            },
        );

        specs.merge_extraction_results(&extraction);

        assert_eq!(specs.len(), original_count + 1);
        let myblock = specs.get("myblock").unwrap();
        assert_eq!(
            myblock.end_tag.as_ref().unwrap().name.as_ref(),
            "endmyblock"
        );
        assert_eq!(myblock.intermediate_tags.len(), 1);
        assert_eq!(myblock.intermediate_tags[0].name.as_ref(), "mymiddle");
        assert_eq!(myblock.module.as_ref(), "myapp.templatetags.custom");
    }

    #[test]
    fn test_merge_extraction_results_skips_filters() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        let mut extraction = djls_extraction::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_extraction::SymbolKey::filter("module", "lower"),
            djls_extraction::BlockTagSpec {
                end_tag: Some("endlower".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );

        specs.merge_extraction_results(&extraction);
        assert_eq!(specs.len(), original_count);
        assert!(specs.get("lower").is_none());
    }

    #[test]
    fn test_merge_extraction_results_empty() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        let extraction = djls_extraction::ExtractionResult::default();
        specs.merge_extraction_results(&extraction);
        assert_eq!(specs.len(), original_count);
    }

    #[test]
    fn test_merge_extraction_results_stores_rules() {
        let mut specs = create_test_specs();

        let mut extraction = djls_extraction::ExtractionResult::default();
        extraction.tag_rules.insert(
            djls_extraction::SymbolKey::tag("django.template.defaulttags", "for"),
            djls_extraction::TagRule {
                arg_constraints: vec![djls_extraction::ArgumentCountConstraint::Min(4)],
                extracted_args: vec![
                    djls_extraction::ExtractedArg {
                        name: "item".to_string(),
                        required: true,
                        kind: djls_extraction::ExtractedArgKind::Variable,
                        position: 0,
                    },
                    djls_extraction::ExtractedArg {
                        name: "in".to_string(),
                        required: true,
                        kind: djls_extraction::ExtractedArgKind::Literal("in".to_string()),
                        position: 1,
                    },
                    djls_extraction::ExtractedArg {
                        name: "iterable".to_string(),
                        required: true,
                        kind: djls_extraction::ExtractedArgKind::Variable,
                        position: 2,
                    },
                ],
                ..Default::default()
            },
        );

        specs.merge_extraction_results(&extraction);

        let for_spec = specs.get("for").unwrap();
        let rules = for_spec.extracted_rules.as_ref().unwrap();
        assert_eq!(rules.extracted_args.len(), 3);
        assert_eq!(rules.extracted_args[0].name, "item");
        assert!(rules.extracted_args[0].required);
        assert_eq!(rules.extracted_args[2].name, "iterable");
    }
}
