// DEPRECATION: This entire module will be removed in v6.2.0
// Legacy v0.4.0 TagSpec format support with deprecation warnings
//
// This module provides backward compatibility for the old flat tagspec format.
// Supported in v6.0.x and v6.1.x; removed in v6.2.0 following the
// project's deprecation policy.

use std::collections::HashMap;

use serde::Deserialize;

use super::ArgKindDef;
use super::EndTagDef;
use super::IntermediateTagDef;
use super::TagArgDef;
use super::TagDef;
use super::TagLibraryDef;
use super::TagSpecDef;
use super::TagTypeDef;

/// Legacy v0.4.0 tag specification (DEPRECATED)
#[deprecated(since = "6.0.0", note = "Remove in v6.2.0")]
#[allow(deprecated)]
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LegacyTagSpecDef {
    /// Tag name (e.g., "for", "if", "cache")
    pub name: String,
    /// Module where this tag is defined (e.g., "django.template.defaulttags")
    pub module: String,
    /// Optional end tag specification
    #[serde(default)]
    pub end_tag: Option<LegacyEndTagDef>,
    /// Optional intermediate tags (e.g., "elif", "else" for "if" tag)
    #[serde(default)]
    pub intermediate_tags: Vec<LegacyIntermediateTagDef>,
    /// Tag arguments specification
    #[serde(default)]
    pub args: Vec<LegacyTagArgDef>,
}

/// Legacy end tag specification
#[deprecated(since = "6.0.0", note = "Remove in v6.2.0")]
#[allow(deprecated)]
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LegacyEndTagDef {
    /// End tag name (e.g., "endfor", "endif")
    pub name: String,
    /// Whether the end tag is optional (default: false)
    #[serde(default)]
    pub optional: bool,
    /// Optional arguments for the end tag
    #[serde(default)]
    pub args: Vec<LegacyTagArgDef>,
}

/// Legacy intermediate tag specification
#[deprecated(since = "6.0.0", note = "Remove in v6.2.0")]
#[allow(deprecated)]
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LegacyIntermediateTagDef {
    /// Intermediate tag name (e.g., "elif", "else")
    pub name: String,
    /// Optional arguments for the intermediate tag
    #[serde(default)]
    pub args: Vec<LegacyTagArgDef>,
}

/// Legacy tag argument specification
#[deprecated(since = "6.0.0", note = "Remove in v6.2.0")]
#[allow(deprecated)]
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LegacyTagArgDef {
    /// Argument name
    pub name: String,
    /// Whether the argument is required (default: true)
    #[serde(default = "default_true")]
    pub required: bool,
    /// Argument type (called "kind" in v0.6.0)
    #[serde(rename = "type")]
    pub arg_type: LegacyArgTypeDef,
}

/// Legacy argument type specification
#[deprecated(since = "6.0.0", note = "Remove in v6.2.0")]
#[allow(deprecated)]
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum LegacyArgTypeDef {
    /// Simple type like "variable", "string", etc.
    Simple(LegacySimpleArgTypeDef),
    /// Choice from a list of values
    Choice { choice: Vec<String> },
}

/// Legacy simple argument types
#[deprecated(since = "6.0.0", note = "Remove in v6.2.0")]
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LegacySimpleArgTypeDef {
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

/// Convert a vector of legacy tagspecs to the new v0.6.0 hierarchical format
///
/// Groups tags by module and creates the appropriate library structure.
#[deprecated(since = "6.0.0", note = "Remove in v6.2.0")]
#[must_use]
pub fn convert_legacy_tagspecs(legacy: Vec<LegacyTagSpecDef>) -> TagSpecDef {
    let mut modules: HashMap<String, Vec<TagDef>> = HashMap::new();

    for legacy_tag in legacy {
        let module = legacy_tag.module.clone();
        let tag = convert_legacy_tag(legacy_tag);
        modules.entry(module).or_default().push(tag);
    }

    let libraries = modules
        .into_iter()
        .map(|(module, tags)| TagLibraryDef {
            module,
            requires_engine: None,
            tags,
            extra: None,
        })
        .collect();

    TagSpecDef {
        version: "0.6.0".to_string(),
        engine: "django".to_string(),
        requires_engine: None,
        extends: vec![],
        libraries,
        extra: None,
    }
}

fn convert_legacy_tag(legacy: LegacyTagSpecDef) -> TagDef {
    let tag_type = if legacy.end_tag.is_some() {
        TagTypeDef::Block
    } else {
        TagTypeDef::Standalone
    };

    TagDef {
        name: legacy.name,
        tag_type,
        end: legacy.end_tag.map(convert_legacy_end_tag),
        intermediates: legacy
            .intermediate_tags
            .into_iter()
            .map(convert_legacy_intermediate_tag)
            .collect(),
        args: legacy.args.into_iter().map(convert_legacy_arg).collect(),
        extra: None,
    }
}

fn convert_legacy_end_tag(legacy: LegacyEndTagDef) -> EndTagDef {
    EndTagDef {
        name: legacy.name,
        required: !legacy.optional, // Invert: optional -> required
        args: legacy.args.into_iter().map(convert_legacy_arg).collect(),
        extra: None,
    }
}

fn convert_legacy_intermediate_tag(legacy: LegacyIntermediateTagDef) -> IntermediateTagDef {
    IntermediateTagDef {
        name: legacy.name,
        args: legacy.args.into_iter().map(convert_legacy_arg).collect(),
        min: None,
        max: None,
        position: super::PositionDef::Any,
        extra: None,
    }
}

fn convert_legacy_arg(legacy: LegacyTagArgDef) -> TagArgDef {
    let (kind, extra) = match legacy.arg_type {
        LegacyArgTypeDef::Simple(simple) => {
            let kind = match simple {
                LegacySimpleArgTypeDef::Literal => ArgKindDef::Literal,
                LegacySimpleArgTypeDef::Variable | LegacySimpleArgTypeDef::String => {
                    ArgKindDef::Variable
                }
                LegacySimpleArgTypeDef::Expression | LegacySimpleArgTypeDef::VarArgs => {
                    ArgKindDef::Any
                }
                LegacySimpleArgTypeDef::Assignment => ArgKindDef::Assignment,
            };
            (kind, None)
        }
        LegacyArgTypeDef::Choice { choice } => {
            // Store choices in extra metadata as required by v0.6.0 spec
            let mut extra = std::collections::HashMap::new();
            extra.insert(
                "choices".to_string(),
                serde_json::Value::Array(
                    choice.into_iter().map(serde_json::Value::String).collect(),
                ),
            );
            (ArgKindDef::Choice, Some(extra))
        }
    };

    TagArgDef {
        name: legacy.name,
        required: legacy.required,
        arg_type: super::ArgTypeDef::Both, // Default for legacy
        kind,
        count: None,
        extra,
    }
}

#[cfg(test)]
mod tests {
    #![allow(deprecated)]

    use super::*;

    #[test]
    fn test_convert_simple_tag() {
        let legacy = vec![LegacyTagSpecDef {
            name: "mytag".to_string(),
            module: "myapp.tags".to_string(),
            end_tag: None,
            intermediate_tags: vec![],
            args: vec![],
        }];

        let converted = convert_legacy_tagspecs(legacy);

        assert_eq!(converted.version, "0.6.0");
        assert_eq!(converted.libraries.len(), 1);
        assert_eq!(converted.libraries[0].module, "myapp.tags");
        assert_eq!(converted.libraries[0].tags.len(), 1);
        assert_eq!(converted.libraries[0].tags[0].name, "mytag");
        assert!(matches!(
            converted.libraries[0].tags[0].tag_type,
            TagTypeDef::Standalone
        ));
    }

    #[test]
    fn test_convert_block_tag() {
        let legacy = vec![LegacyTagSpecDef {
            name: "block".to_string(),
            module: "django.template.defaulttags".to_string(),
            end_tag: Some(LegacyEndTagDef {
                name: "endblock".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: vec![],
            args: vec![],
        }];

        let converted = convert_legacy_tagspecs(legacy);

        assert_eq!(converted.libraries[0].tags[0].name, "block");
        assert!(matches!(
            converted.libraries[0].tags[0].tag_type,
            TagTypeDef::Block
        ));
        assert!(converted.libraries[0].tags[0].end.is_some());
        assert_eq!(
            converted.libraries[0].tags[0].end.as_ref().unwrap().name,
            "endblock"
        );
        assert!(
            converted.libraries[0].tags[0]
                .end
                .as_ref()
                .unwrap()
                .required
        );
    }

    #[test]
    fn test_convert_optional_end_tag() {
        let legacy = vec![LegacyTagSpecDef {
            name: "autoescape".to_string(),
            module: "django.template.defaulttags".to_string(),
            end_tag: Some(LegacyEndTagDef {
                name: "endautoescape".to_string(),
                optional: true,
                args: vec![],
            }),
            intermediate_tags: vec![],
            args: vec![],
        }];

        let converted = convert_legacy_tagspecs(legacy);

        assert!(
            !converted.libraries[0].tags[0]
                .end
                .as_ref()
                .unwrap()
                .required
        );
    }

    #[test]
    fn test_convert_arg_types() {
        let legacy = vec![LegacyTagSpecDef {
            name: "test".to_string(),
            module: "test.module".to_string(),
            end_tag: None,
            intermediate_tags: vec![],
            args: vec![
                LegacyTagArgDef {
                    name: "lit".to_string(),
                    required: true,
                    arg_type: LegacyArgTypeDef::Simple(LegacySimpleArgTypeDef::Literal),
                },
                LegacyTagArgDef {
                    name: "var".to_string(),
                    required: true,
                    arg_type: LegacyArgTypeDef::Simple(LegacySimpleArgTypeDef::Variable),
                },
                LegacyTagArgDef {
                    name: "choice".to_string(),
                    required: true,
                    arg_type: LegacyArgTypeDef::Choice {
                        choice: vec!["on".to_string(), "off".to_string()],
                    },
                },
            ],
        }];

        let converted = convert_legacy_tagspecs(legacy);
        let args = &converted.libraries[0].tags[0].args;

        assert!(matches!(args[0].kind, ArgKindDef::Literal));
        assert!(matches!(args[1].kind, ArgKindDef::Variable));
        assert!(matches!(args[2].kind, ArgKindDef::Choice));

        // Check that choices are stored in extra
        assert!(args[2].extra.is_some());
        let choices = args[2].extra.as_ref().unwrap().get("choices").unwrap();
        assert_eq!(choices, &serde_json::json!(["on", "off"]));
    }

    #[test]
    fn test_convert_groups_by_module() {
        let legacy = vec![
            LegacyTagSpecDef {
                name: "tag1".to_string(),
                module: "module.a".to_string(),
                end_tag: None,
                intermediate_tags: vec![],
                args: vec![],
            },
            LegacyTagSpecDef {
                name: "tag2".to_string(),
                module: "module.b".to_string(),
                end_tag: None,
                intermediate_tags: vec![],
                args: vec![],
            },
            LegacyTagSpecDef {
                name: "tag3".to_string(),
                module: "module.a".to_string(),
                end_tag: None,
                intermediate_tags: vec![],
                args: vec![],
            },
        ];

        let converted = convert_legacy_tagspecs(legacy);

        assert_eq!(converted.libraries.len(), 2);

        // Find the libraries by module (order is not guaranteed due to HashMap)
        let module_a = converted
            .libraries
            .iter()
            .find(|lib| lib.module == "module.a")
            .unwrap();
        let module_b = converted
            .libraries
            .iter()
            .find(|lib| lib.module == "module.b")
            .unwrap();

        assert_eq!(module_a.tags.len(), 2);
        assert_eq!(module_b.tags.len(), 1);
    }

    #[test]
    fn test_convert_intermediate_tags() {
        let legacy = vec![LegacyTagSpecDef {
            name: "if".to_string(),
            module: "django.template.defaulttags".to_string(),
            end_tag: Some(LegacyEndTagDef {
                name: "endif".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: vec![
                LegacyIntermediateTagDef {
                    name: "elif".to_string(),
                    args: vec![],
                },
                LegacyIntermediateTagDef {
                    name: "else".to_string(),
                    args: vec![],
                },
            ],
            args: vec![],
        }];

        let converted = convert_legacy_tagspecs(legacy);
        let intermediates = &converted.libraries[0].tags[0].intermediates;

        assert_eq!(intermediates.len(), 2);
        assert_eq!(intermediates[0].name, "elif");
        assert_eq!(intermediates[1].name, "else");
    }
}
