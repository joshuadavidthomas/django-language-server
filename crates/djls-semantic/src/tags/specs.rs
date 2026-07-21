use std::borrow::Cow;
use std::collections::hash_map::IntoIter;
use std::collections::hash_map::Iter;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;

use djls_conf::ArgKindDef;
use djls_conf::ArgTypeDef;
use djls_conf::TagLibraryDef;
use djls_conf::TagSpecDef;
use djls_conf::TagTypeDef;
use djls_project::BlockSpecs;
use djls_project::TagArgument;
use djls_project::TagArgumentKind;
use djls_project::TagRule;
use djls_project::TagRuleMap;
use djls_project::TemplateSymbolKind;
use rustc_hash::FxHashMap;

use super::TagRole;
use crate::references::TemplateReferenceKind;

pub(crate) type S<T = str> = Cow<'static, T>;
pub(crate) type L<T> = Cow<'static, [T]>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagSpecs(FxHashMap<String, TagSpec>);

impl TagSpecs {
    #[must_use]
    pub fn new(specs: FxHashMap<String, TagSpec>) -> Self {
        TagSpecs(specs)
    }

    /// Merge another `TagSpecs` into this one, with the other taking precedence
    pub fn merge(&mut self, other: TagSpecs) -> &mut Self {
        self.0.extend(other.0);
        self
    }

    /// Merge block specs into tag specs.
    ///
    /// Block specs from extraction override existing end-tag/intermediate info.
    /// This enriches the handcoded `builtins.rs` defaults with information
    /// extracted from actual Python source code.
    pub fn merge_block_specs(&mut self, block_specs: &BlockSpecs) -> &mut Self {
        for (key, block_spec) in block_specs.as_map() {
            if key.kind != TemplateSymbolKind::Tag {
                continue;
            }
            if let Some(spec) = self.0.get_mut(&key.name) {
                // An unknown closer is insufficient evidence to modify an
                // existing spec.
                let Some(end_tag_name) = &block_spec.end_tag else {
                    continue;
                };

                spec.end_tag = Some(EndTag {
                    name: end_tag_name.clone().into(),
                    required: true,
                });
                // Override intermediates from extraction
                spec.intermediate_tags = if block_spec.intermediates.is_empty() {
                    Cow::Borrowed(&[])
                } else {
                    Cow::Owned(
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
                        intermediate_tags: Cow::Owned(intermediate_tags),
                        opaque: block_spec.opaque,
                        role: None,
                        extracted_rules: None,
                    },
                );
            }
        }
        self
    }

    /// Merge tag rules into tag specs.
    pub fn merge_tag_rules(&mut self, tag_rules: &TagRuleMap) -> &mut Self {
        for (key, tag_rule) in tag_rules {
            if key.kind != TemplateSymbolKind::Tag {
                continue;
            }

            if let Some(spec) = self.0.get_mut(&key.name) {
                spec.set_extracted_rules(Arc::clone(tag_rule));
            } else {
                // Tag not yet in specs — create a minimal entry with extracted rules
                self.0.insert(
                    key.name.clone(),
                    TagSpec::new(
                        key.registration_module.clone().into(),
                        None,
                        Cow::Borrowed(&[]),
                        false,
                    )
                    .with_extracted_rules(Arc::clone(tag_rule)),
                );
            }
        }
        self
    }

    /// Merge fallback specs into this one without overriding extraction-derived data.
    ///
    /// This is used for manual `TagSpecs` configuration: extraction wins, fallback
    /// only fills missing end tags, intermediates, and argument rules.
    pub(crate) fn merge_fallback(&mut self, fallback: TagSpecs) -> &mut Self {
        let TagSpecs(fallback) = fallback;

        for (name, fallback_spec) in fallback {
            match self.0.get_mut(&name) {
                None => {
                    self.0.insert(name, fallback_spec);
                }
                Some(existing) => {
                    let mut fallback_spec = fallback_spec;

                    if existing.end_tag.is_none() {
                        existing.end_tag = fallback_spec.end_tag.take();
                    }

                    if existing.intermediate_tags.is_empty()
                        && !fallback_spec.intermediate_tags.is_empty()
                    {
                        existing.intermediate_tags = fallback_spec.intermediate_tags;
                    }

                    if existing.extracted_rules.is_none() {
                        existing.extracted_rules = fallback_spec.extracted_rules;
                    }
                }
            }
        }

        self
    }

    #[must_use]
    pub(crate) fn from_tagspec_library(library: &TagLibraryDef) -> TagSpecs {
        let mut doc = TagSpecDef::default();
        doc.libraries.push(library.clone());
        Self::from_tagspec_def(&doc)
    }

    #[must_use]
    #[allow(clippy::too_many_lines)]
    fn from_tagspec_def(doc: &TagSpecDef) -> TagSpecs {
        let mut specs = FxHashMap::default();

        for library in &doc.libraries {
            for tag_def in &library.tags {
                let end_tag = match tag_def.tag_type.clone() {
                    TagTypeDef::Block => tag_def.end.as_ref().map_or_else(
                        || {
                            Some(EndTag {
                                name: format!("end{}", tag_def.name).into(),
                                required: true,
                            })
                        },
                        |end| {
                            Some(EndTag {
                                name: end.name.clone().into(),
                                required: end.required,
                            })
                        },
                    ),
                    TagTypeDef::Loader => tag_def.end.as_ref().map(|end| EndTag {
                        name: end.name.clone().into(),
                        required: end.required,
                    }),
                    TagTypeDef::Standalone => None,
                };

                let intermediate_tags: Vec<IntermediateTag> = tag_def
                    .intermediates
                    .iter()
                    .map(|it| IntermediateTag {
                        name: it.name.clone().into(),
                    })
                    .collect();

                let extracted_rules = if tag_def.args.is_empty() {
                    None
                } else {
                    use djls_project::ArgumentCountConstraint;
                    use djls_project::ChoiceAt;
                    use djls_project::RequiredKeyword;
                    use djls_project::SplitPosition;

                    let mut rule = TagRule::default();

                    let required_count = tag_def.args.iter().filter(|arg| arg.required).count();
                    if required_count > 0 {
                        rule.arg_constraints
                            .push(ArgumentCountConstraint::Min(required_count + 1));
                    }

                    let mut extracted_args = Vec::new();

                    for (pos, arg) in tag_def.args.iter().enumerate() {
                        let mut kind = match arg.kind.clone() {
                            ArgKindDef::Syntax | ArgKindDef::Literal | ArgKindDef::Modifier => {
                                TagArgumentKind::Literal(arg.name.clone())
                            }
                            ArgKindDef::Choice => {
                                let choices: Vec<String> = arg
                                    .extra
                                    .as_ref()
                                    .and_then(|extra| extra.get("choices"))
                                    .and_then(serde_json::Value::as_array)
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(serde_json::Value::as_str)
                                            .map(String::from)
                                            .collect()
                                    })
                                    .unwrap_or_default();

                                TagArgumentKind::Choice(choices)
                            }
                            ArgKindDef::Variable | ArgKindDef::Any | ArgKindDef::Assignment => {
                                TagArgumentKind::Variable
                            }
                        };

                        if matches!(arg.arg_type, ArgTypeDef::Keyword)
                            && matches!(kind, TagArgumentKind::Variable)
                        {
                            kind = TagArgumentKind::Keyword;
                        }

                        extracted_args.push(TagArgument {
                            name: arg.name.clone(),
                            required: arg.required,
                            kind: kind.clone(),
                            position: pos,
                        });

                        if arg.required {
                            match kind {
                                TagArgumentKind::Literal(value) => {
                                    rule.required_keywords.push(RequiredKeyword {
                                        position: SplitPosition::Forward(pos + 1),
                                        value,
                                    });
                                }
                                TagArgumentKind::Choice(values) if !values.is_empty() => {
                                    rule.choice_at_constraints.push(ChoiceAt {
                                        position: SplitPosition::Forward(pos + 1),
                                        values,
                                    });
                                }
                                TagArgumentKind::Variable
                                | TagArgumentKind::Choice(_)
                                | TagArgumentKind::VarArgs
                                | TagArgumentKind::Keyword => {}
                            }
                        }
                    }

                    rule.extracted_args = extracted_args;

                    Some(rule.into())
                };

                specs.insert(
                    tag_def.name.clone(),
                    TagSpec {
                        module: library.module.clone().into(),
                        end_tag,
                        intermediate_tags: Cow::Owned(intermediate_tags),
                        opaque: false,
                        role: None,
                        extracted_rules,
                    },
                );
            }
        }

        TagSpecs::new(specs)
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

/// Specification for a Django template tag's structure, validation rules, and role.
///
/// Argument validation is handled by `extracted_rules` (derived from Python AST
/// extraction). Argument structure for completions/snippets is accessed via
/// `extracted_rules.extracted_args`. `role` records durable tag meaning
/// that downstream features can use without matching on tag names.
#[derive(Debug, Clone, PartialEq)]
pub struct TagSpec {
    module: S,
    pub end_tag: Option<EndTag>,
    pub(crate) intermediate_tags: L<IntermediateTag>,
    pub(crate) opaque: bool,
    role: Option<TagRole>,
    /// Extraction-derived validation rules from Python AST analysis.
    ///
    /// When present, provides argument validation (S117 diagnostics) and
    /// argument structure for completions/snippets via `extracted_args`.
    extracted_rules: Option<Arc<TagRule>>,
}

impl TagSpec {
    #[must_use]
    pub fn new(
        module: Cow<'static, str>,
        end_tag: Option<EndTag>,
        intermediate_tags: Cow<'static, [IntermediateTag]>,
        opaque: bool,
    ) -> Self {
        Self {
            module,
            end_tag,
            intermediate_tags,
            opaque,
            role: None,
            extracted_rules: None,
        }
    }

    #[must_use]
    pub(crate) fn module(&self) -> &str {
        self.module.as_ref()
    }

    #[must_use]
    pub fn role(&self) -> Option<TagRole> {
        self.role
    }

    #[must_use]
    pub(crate) fn extracted_rules(&self) -> Option<&TagRule> {
        self.extracted_rules.as_deref()
    }

    pub fn set_extracted_rules(&mut self, rules: Arc<TagRule>) {
        self.extracted_rules = Some(rules);
    }

    #[must_use]
    pub fn with_extracted_rules(mut self, rules: Arc<TagRule>) -> Self {
        self.set_extracted_rules(rules);
        self
    }

    #[must_use]
    pub fn with_role(mut self, role: TagRole) -> Self {
        self.role = Some(role);
        self
    }

    #[must_use]
    pub fn arguments(&self) -> Vec<TagArgument> {
        self.extracted_rules
            .as_ref()
            .map_or_else(Vec::new, |rules| rules.extracted_args.clone())
    }

    #[must_use]
    pub fn with_arguments(mut self, args: Vec<TagArgument>) -> Self {
        let mut rules = self.extracted_rules.as_deref().cloned().unwrap_or_default();
        rules.extracted_args = args;
        self.extracted_rules = Some(rules.into());
        self
    }
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
/// need argument metadata can use [`TagSpec::with_arguments`], while tests
/// that need full validation rules should use extraction on Python source.
#[must_use]
#[allow(clippy::similar_names, clippy::too_many_lines)]
pub fn builtin_tag_specs() -> TagSpecs {
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
        role: None,
        extracted_rules: None,
    };

    let simple_role = |module: &'static str, role: TagRole| TagSpec {
        module: B(module),
        end_tag: None,
        intermediate_tags: B(&[]),
        opaque: false,
        role: Some(role),
        extracted_rules: None,
    };

    let block = |module: &'static str,
                 end: &'static str,
                 intermediates: Vec<IntermediateTag>,
                 opaque: bool,
                 role: TagRole| TagSpec {
        module: B(module),
        end_tag: Some(EndTag {
            name: B(end),
            required: true,
        }),
        intermediate_tags: Cow::Owned(intermediates),
        opaque,
        role: Some(role),
        extracted_rules: None,
    };

    let im = |name: &'static str| IntermediateTag { name: B(name) };

    // defaulttags
    specs.insert(
        "autoescape".into(),
        block(dt, "endautoescape", vec![], false, TagRole::ControlTag),
    );
    specs.insert(
        "comment".into(),
        block(dt, "endcomment", vec![], true, TagRole::ControlTag),
    );
    specs.insert("csrf_token".into(), simple(dt));
    specs.insert("cycle".into(), simple(dt));
    specs.insert("debug".into(), simple(dt));
    specs.insert(
        "filter".into(),
        block(dt, "endfilter", vec![], false, TagRole::ControlTag),
    );
    specs.insert("firstof".into(), simple(dt));
    specs.insert(
        "for".into(),
        block(dt, "endfor", vec![im("empty")], false, TagRole::ControlTag),
    );
    specs.insert(
        "if".into(),
        block(
            dt,
            "endif",
            vec![im("elif"), im("else")],
            false,
            TagRole::ControlTag,
        ),
    );
    specs.insert(
        "ifchanged".into(),
        block(
            dt,
            "endifchanged",
            vec![im("else")],
            false,
            TagRole::ControlTag,
        ),
    );
    specs.insert(
        "load".into(),
        simple_role(dt, TagRole::TemplateLibraryLoader),
    );
    specs.insert("lorem".into(), simple(dt));
    specs.insert("now".into(), simple(dt));
    specs.insert("regroup".into(), simple(dt));
    specs.insert(
        "spaceless".into(),
        block(dt, "endspaceless", vec![], false, TagRole::ControlTag),
    );
    specs.insert("templatetag".into(), simple(dt));
    specs.insert("url".into(), simple_role(dt, TagRole::RouteReference));
    specs.insert(
        "verbatim".into(),
        block(dt, "endverbatim", vec![], true, TagRole::ControlTag),
    );
    specs.insert("widthratio".into(), simple(dt));
    specs.insert(
        "with".into(),
        block(dt, "endwith", vec![], false, TagRole::ControlTag),
    );

    // loader_tags
    specs.insert(
        "block".into(),
        block(lt, "endblock", vec![], false, TagRole::TemplateBlock),
    );
    specs.insert(
        "extends".into(),
        simple_role(
            lt,
            TagRole::TemplateReference(TemplateReferenceKind::Extends),
        ),
    );
    specs.insert(
        "include".into(),
        simple_role(
            lt,
            TagRole::TemplateReference(TemplateReferenceKind::Include),
        ),
    );

    // i18n
    specs.insert(
        "blocktrans".into(),
        block(
            i18n,
            "endblocktrans",
            vec![im("plural")],
            false,
            TagRole::ControlTag,
        ),
    );
    specs.insert(
        "blocktranslate".into(),
        block(
            i18n,
            "endblocktranslate",
            vec![im("plural")],
            false,
            TagRole::ControlTag,
        ),
    );
    specs.insert("trans".into(), simple(i18n));
    specs.insert("translate".into(), simple(i18n));

    // cache
    specs.insert(
        "cache".into(),
        block(cache, "endcache", vec![], false, TagRole::ControlTag),
    );

    // l10n
    specs.insert(
        "localize".into(),
        block(l10n, "endlocalize", vec![], false, TagRole::ControlTag),
    );

    // static
    specs.insert(
        "static".into(),
        simple_role(st, TagRole::StaticAssetReference),
    );

    // tz
    specs.insert(
        "localtime".into(),
        block(tz, "endlocaltime", vec![], false, TagRole::ControlTag),
    );
    specs.insert(
        "timezone".into(),
        block(tz, "endtimezone", vec![], false, TagRole::ControlTag),
    );

    TagSpecs::new(specs)
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::string::ToString;

    use djls_project::ArgumentCountConstraint;
    use djls_project::BlockSpec;
    use djls_project::SymbolKey;

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
                role: None,
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
                role: None,
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
                role: None,
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
                role: None,
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

        let if_spec = specs
            .get("if")
            .expect("test specs should contain the if tag");
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
                .map(ToString::to_string)
                .collect::<Vec<_>>()
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
                role: None,
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
                role: None,
                extracted_rules: None,
            },
        );

        let specs2 = TagSpecs::new(specs2_map);
        let result = specs1.merge(specs2);
        assert!(ptr::eq(result, ptr::from_ref(&specs1)));

        assert!(specs1.get("custom").is_some());
        let if_spec = specs1
            .get("if")
            .expect("merged test specs should contain the if tag");
        assert!(
            !if_spec
                .end_tag
                .as_ref()
                .expect("the merged if tag should retain its end tag")
                .required
        );
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
    fn test_merge_block_specs_overrides_existing() {
        let mut specs = create_test_specs();

        let original_if = specs
            .get("if")
            .expect("test specs should contain the if tag before merging block specs");
        assert!(original_if.end_tag.is_some());
        assert_eq!(original_if.intermediate_tags.len(), 2);

        let mut block_specs = BlockSpecs::default();
        block_specs.insert(
            SymbolKey::tag("django.template.defaulttags", "if"),
            BlockSpec {
                end_tag: Some("endif".to_string()),
                intermediates: vec!["elif".to_string(), "else".to_string(), "elseif".to_string()],
                opaque: false,
            },
        );

        specs.merge_block_specs(&block_specs);

        let if_spec = specs
            .get("if")
            .expect("test specs should contain the if tag after merging block specs");
        assert_eq!(
            if_spec
                .end_tag
                .as_ref()
                .expect("the merged if block spec should define an end tag")
                .name
                .as_ref(),
            "endif"
        );
        assert_eq!(if_spec.intermediate_tags.len(), 3);
        assert!(
            if_spec
                .intermediate_tags
                .iter()
                .any(|t| t.name.as_ref() == "elseif")
        );
    }

    #[test]
    fn test_merge_block_specs_unknown_end_tag_preserves_existing_spec() {
        let mut spec_map = FxHashMap::default();
        spec_map.insert(
            "guarded".to_string(),
            TagSpec {
                module: "django.template.defaulttags".into(),
                end_tag: Some(EndTag {
                    name: "endguarded".into(),
                    required: true,
                }),
                intermediate_tags: Cow::Owned(vec![IntermediateTag {
                    name: "middle".into(),
                }]),
                opaque: true,
                role: None,
                extracted_rules: None,
            },
        );
        let mut specs = TagSpecs::new(spec_map);
        let original = specs
            .get("guarded")
            .expect("test specs should contain the guarded tag")
            .clone();

        let mut block_specs = BlockSpecs::default();
        block_specs.insert(
            SymbolKey::tag("django.template.defaulttags", "guarded"),
            BlockSpec {
                end_tag: None,
                intermediates: vec![],
                opaque: false,
            },
        );

        specs.merge_block_specs(&block_specs);

        assert_eq!(
            specs
                .get("guarded")
                .expect("the guarded tag should remain after merging block specs"),
            &original
        );
    }

    #[test]
    fn test_merge_block_specs_unknown_end_tag_inserts_new_spec_without_end_tag() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        let mut block_specs = BlockSpecs::default();
        block_specs.insert(
            SymbolKey::tag("myapp.templatetags.custom", "unknownblock"),
            BlockSpec {
                end_tag: None,
                intermediates: vec![],
                opaque: false,
            },
        );

        specs.merge_block_specs(&block_specs);

        assert_eq!(specs.len(), original_count + 1);
        let unknownblock = specs
            .get("unknownblock")
            .expect("merging block specs should insert the unknownblock tag");
        assert_eq!(unknownblock.end_tag, None);
    }

    #[test]
    fn test_merge_block_specs_adds_new_tag() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        let mut block_specs = BlockSpecs::default();
        block_specs.insert(
            SymbolKey::tag("myapp.templatetags.custom", "myblock"),
            BlockSpec {
                end_tag: Some("endmyblock".to_string()),
                intermediates: vec!["mymiddle".to_string()],
                opaque: false,
            },
        );

        specs.merge_block_specs(&block_specs);

        assert_eq!(specs.len(), original_count + 1);
        let myblock = specs
            .get("myblock")
            .expect("merging block specs should insert the myblock tag");
        assert_eq!(
            myblock
                .end_tag
                .as_ref()
                .expect("the merged myblock tag should define an end tag")
                .name
                .as_ref(),
            "endmyblock"
        );
        assert_eq!(myblock.intermediate_tags.len(), 1);
        assert_eq!(myblock.intermediate_tags[0].name.as_ref(), "mymiddle");
        assert_eq!(myblock.module.as_ref(), "myapp.templatetags.custom");
        assert_eq!(myblock.role, None);
    }

    #[test]
    fn test_merge_block_specs_skips_filters() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        let mut block_specs = BlockSpecs::default();
        block_specs.insert(
            SymbolKey::filter("module", "lower"),
            BlockSpec {
                end_tag: Some("endlower".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );

        specs.merge_block_specs(&block_specs);
        assert_eq!(specs.len(), original_count);
        assert!(specs.get("lower").is_none());
    }

    #[test]
    fn test_merge_extraction_maps_empty() {
        let mut specs = create_test_specs();
        let original_count = specs.len();

        specs
            .merge_block_specs(&BlockSpecs::default())
            .merge_tag_rules(&TagRuleMap::default());

        assert_eq!(specs.len(), original_count);
    }

    #[test]
    fn test_merge_tag_rules_stores_rules() {
        let mut specs = create_test_specs();

        let mut tag_rules = TagRuleMap::default();
        tag_rules.insert(
            SymbolKey::tag("django.template.defaulttags", "for"),
            TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Min(4)],
                extracted_args: vec![
                    TagArgument {
                        name: "item".to_string(),
                        required: true,
                        kind: TagArgumentKind::Variable,
                        position: 0,
                    },
                    TagArgument {
                        name: "in".to_string(),
                        required: true,
                        kind: TagArgumentKind::Literal("in".to_string()),
                        position: 1,
                    },
                    TagArgument {
                        name: "iterable".to_string(),
                        required: true,
                        kind: TagArgumentKind::Variable,
                        position: 2,
                    },
                ],
                ..Default::default()
            }
            .into(),
        );

        specs.merge_tag_rules(&tag_rules);

        let for_spec = specs
            .get("for")
            .expect("test specs should contain the for tag after merging rules");
        let rules = for_spec
            .extracted_rules
            .as_ref()
            .expect("the merged for tag should contain extracted rules");
        assert_eq!(rules.extracted_args.len(), 3);
        assert_eq!(rules.extracted_args[0].name, "item");
        assert!(rules.extracted_args[0].required);
        assert_eq!(rules.extracted_args[2].name, "iterable");
    }
}
