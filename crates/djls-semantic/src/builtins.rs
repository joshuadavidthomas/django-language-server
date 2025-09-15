//! Built-in Django template tag specifications.
//!
//! This module defines all the standard Django template tags as compile-time
//! constants, avoiding the need for runtime TOML parsing.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::specs::EndTag;
use super::specs::IntermediateTag;
use super::specs::TagArg;
use super::specs::TagSpec;
use super::specs::TagSpecs;

// Static storage for built-in specs - built only once on first access
static BUILTIN_SPECS: LazyLock<TagSpecs> = LazyLock::new(|| {
    let mut specs = HashMap::new();

    // Define all Django built-in tags using direct struct construction
    let tags = vec![
        // Control flow tags
        TagSpec {
            name: Some("autoescape".to_string()),
            end_tag: Some(EndTag {
                name: "endautoescape".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::choice(
                "mode",
                true,
                vec!["on".to_string(), "off".to_string()],
            )],
        },
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
                    args: vec![TagArg::expr("condition", true)],
                },
                IntermediateTag {
                    name: "else".to_string(),
                    args: vec![],
                },
            ]),
            args: vec![TagArg::expr("condition", true)],
        },
        TagSpec {
            name: Some("for".to_string()),
            end_tag: Some(EndTag {
                name: "endfor".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: Some(vec![IntermediateTag {
                name: "empty".to_string(),
                args: vec![],
            }]),
            args: vec![
                TagArg::var("item", true),
                TagArg::literal("in", true),
                TagArg::var("items", true),
                TagArg::literal("reversed", false),
            ],
        },
        TagSpec {
            name: Some("ifchanged".to_string()),
            end_tag: Some(EndTag {
                name: "endifchanged".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: Some(vec![IntermediateTag {
                name: "else".to_string(),
                args: vec![],
            }]),
            args: vec![TagArg::varargs("variables", false)],
        },
        TagSpec {
            name: Some("with".to_string()),
            end_tag: Some(EndTag {
                name: "endwith".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::varargs("assignments", true)],
        },
        // Block tags
        TagSpec {
            name: Some("block".to_string()),
            end_tag: Some(EndTag {
                name: "endblock".to_string(),
                optional: false,
                args: vec![TagArg::var("name", false)],
            }),
            intermediate_tags: None,
            args: vec![TagArg::var("name", true)],
        },
        TagSpec {
            name: Some("extends".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![TagArg::string("template", true)],
        },
        TagSpec {
            name: Some("include".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::string("template", true),
                TagArg::literal("with", false),
                TagArg::varargs("context", false),
                TagArg::literal("only", false),
            ],
        },
        TagSpec {
            name: Some("load".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![TagArg::varargs("libraries", true)],
        },
        // Content manipulation tags
        TagSpec {
            name: Some("comment".to_string()),
            end_tag: Some(EndTag {
                name: "endcomment".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::string("note", false)],
        },
        TagSpec {
            name: Some("filter".to_string()),
            end_tag: Some(EndTag {
                name: "endfilter".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::varargs("filters", true)],
        },
        TagSpec {
            name: Some("spaceless".to_string()),
            end_tag: Some(EndTag {
                name: "endspaceless".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![],
        },
        TagSpec {
            name: Some("verbatim".to_string()),
            end_tag: Some(EndTag {
                name: "endverbatim".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::string("name", false)],
        },
        // Variables and expressions
        TagSpec {
            name: Some("cycle".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::varargs("values", true),
                TagArg::literal("as", false),
                TagArg::var("varname", false),
                TagArg::literal("silent", false),
            ],
        },
        TagSpec {
            name: Some("firstof".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::varargs("variables", true),
                TagArg::string("fallback", false),
                TagArg::literal("as", false),
                TagArg::var("varname", false),
            ],
        },
        TagSpec {
            name: Some("regroup".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::var("target", true),
                TagArg::literal("by", true),
                TagArg::var("attribute", true),
                TagArg::literal("as", true),
                TagArg::var("grouped", true),
            ],
        },
        // Date and time
        TagSpec {
            name: Some("now".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::string("format_string", true),
                TagArg::literal("as", false),
                TagArg::var("varname", false),
            ],
        },
        // URLs and static files
        TagSpec {
            name: Some("url".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::string("view_name", true),
                TagArg::varargs("args", false),
                TagArg::literal("as", false),
                TagArg::var("varname", false),
            ],
        },
        TagSpec {
            name: Some("static".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![TagArg::string("path", true)],
        },
        // Template tags
        TagSpec {
            name: Some("templatetag".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![TagArg::choice(
                "tagbit",
                true,
                vec![
                    "openblock".to_string(),
                    "closeblock".to_string(),
                    "openvariable".to_string(),
                    "closevariable".to_string(),
                    "openbrace".to_string(),
                    "closebrace".to_string(),
                    "opencomment".to_string(),
                    "closecomment".to_string(),
                ],
            )],
        },
        // Security
        TagSpec {
            name: Some("csrf_token".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![],
        },
        // Utilities
        TagSpec {
            name: Some("widthratio".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::var("this_value", true),
                TagArg::var("max_value", true),
                TagArg::var("max_width", true),
                TagArg::literal("as", false),
                TagArg::var("varname", false),
            ],
        },
        TagSpec {
            name: Some("lorem".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::var("count", false),
                TagArg::choice(
                    "method",
                    false,
                    vec!["w".to_string(), "p".to_string(), "b".to_string()],
                ),
                TagArg::literal("random", false),
            ],
        },
        TagSpec {
            name: Some("debug".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![],
        },
        // Cache tags
        TagSpec {
            name: Some("cache".to_string()),
            end_tag: Some(EndTag {
                name: "endcache".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![
                TagArg::var("timeout", true),
                TagArg::var("cache_key", true),
                TagArg::varargs("variables", false),
            ],
        },
        // Internationalization
        TagSpec {
            name: Some("localize".to_string()),
            end_tag: Some(EndTag {
                name: "endlocalize".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::choice(
                "mode",
                false,
                vec!["on".to_string(), "off".to_string()],
            )],
        },
        TagSpec {
            name: Some("blocktranslate".to_string()),
            end_tag: Some(EndTag {
                name: "endblocktranslate".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: Some(vec![IntermediateTag {
                name: "plural".to_string(),
                args: vec![TagArg::var("count", false)],
            }]),
            args: vec![
                TagArg::string("context", false),
                TagArg::literal("with", false),
                TagArg::varargs("assignments", false),
                TagArg::literal("asvar", false),
                TagArg::var("varname", false),
            ],
        },
        TagSpec {
            name: Some("trans".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                TagArg::string("message", true),
                TagArg::string("context", false),
                TagArg::literal("as", false),
                TagArg::var("varname", false),
                TagArg::literal("noop", false),
            ],
        },
        // Timezone tags
        TagSpec {
            name: Some("localtime".to_string()),
            end_tag: Some(EndTag {
                name: "endlocaltime".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::choice(
                "mode",
                false,
                vec!["on".to_string(), "off".to_string()],
            )],
        },
        TagSpec {
            name: Some("timezone".to_string()),
            end_tag: Some(EndTag {
                name: "endtimezone".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![TagArg::var("timezone", true)],
        },
    ];

    // Insert all tags into the HashMap
    for tag in tags {
        if let Some(ref name) = tag.name {
            specs.insert(name.clone(), tag);
        }
    }

    TagSpecs::new(specs)
});

/// Returns all built-in Django template tag specifications
///
/// This function returns a clone of the statically initialized built-in specs.
/// The actual specs are only built once on first access and then cached.
#[must_use]
pub fn django_builtin_specs() -> TagSpecs {
    BUILTIN_SPECS.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_specs_non_empty() {
        let specs = django_builtin_specs();

        // Verify we have specs loaded
        assert!(
            specs.iter().count() > 0,
            "Should have loaded at least one spec"
        );

        // Check a key tag is present as a smoke test
        assert!(specs.get("if").is_some(), "'if' tag should be present");

        // Verify all tag names are non-empty
        for (name, _) in specs.iter() {
            assert!(!name.is_empty(), "Tag name should not be empty");
        }
    }

    #[test]
    fn test_all_expected_tags_present() {
        let specs = django_builtin_specs();

        // Block tags that should be present
        let expected_block_tags = [
            "autoescape",
            "block",
            "comment",
            "filter",
            "for",
            "if",
            "ifchanged",
            "spaceless",
            "verbatim",
            "with",
            "cache",
            "localize",
            "blocktranslate",
            "localtime",
            "timezone",
        ];

        // Single tags that should be present
        let expected_single_tags = [
            "csrf_token",
            "cycle",
            "extends",
            "include",
            "load",
            "now",
            "templatetag",
            "url",
            "debug",
            "firstof",
            "lorem",
            "regroup",
            "widthratio",
            "trans",
            "static",
        ];

        for tag in expected_block_tags {
            let spec = specs
                .get(tag)
                .unwrap_or_else(|| panic!("{tag} tag should be present"));
            assert!(spec.end_tag.is_some(), "{tag} should have an end tag");
        }

        for tag in expected_single_tags {
            assert!(specs.get(tag).is_some(), "{tag} tag should be present");
        }

        // Tags that should NOT be present yet (future Django versions)
        let missing_tags = [
            "querystring", // Django 5.1+
            "resetcycle",
        ];

        for tag in missing_tags {
            assert!(
                specs.get(tag).is_none(),
                "{tag} tag should not be present yet"
            );
        }
    }

    #[test]
    fn test_if_tag_structure() {
        let specs = django_builtin_specs();
        let if_tag = specs.get("if").expect("if tag should exist");

        assert_eq!(if_tag.name, Some("if".to_string()));
        assert!(if_tag.end_tag.is_some());
        assert_eq!(if_tag.end_tag.as_ref().unwrap().name, "endif");

        let intermediates = if_tag.intermediate_tags.as_ref().unwrap();
        assert_eq!(intermediates.len(), 2);
        assert_eq!(intermediates[0].name, "elif");
        assert_eq!(intermediates[1].name, "else");
    }

    #[test]
    fn test_for_tag_structure() {
        let specs = django_builtin_specs();
        let for_tag = specs.get("for").expect("for tag should exist");

        assert_eq!(for_tag.name, Some("for".to_string()));
        assert!(for_tag.end_tag.is_some());
        assert_eq!(for_tag.end_tag.as_ref().unwrap().name, "endfor");

        let intermediates = for_tag.intermediate_tags.as_ref().unwrap();
        assert_eq!(intermediates.len(), 1);
        assert_eq!(intermediates[0].name, "empty");

        // Check args structure
        assert!(!for_tag.args.is_empty(), "for tag should have arguments");
    }

    #[test]
    fn test_block_tag_with_end_args() {
        let specs = django_builtin_specs();
        let block_tag = specs.get("block").expect("block tag should exist");

        let end_tag = block_tag.end_tag.as_ref().unwrap();
        assert_eq!(end_tag.name, "endblock");
        assert_eq!(end_tag.args.len(), 1);
        assert_eq!(end_tag.args[0].name, "name");
        assert!(!end_tag.args[0].required);
    }

    #[test]
    fn test_single_tag_structure() {
        let specs = django_builtin_specs();

        // Test a single tag has no end tag or intermediates
        let csrf_tag = specs
            .get("csrf_token")
            .expect("csrf_token tag should exist");
        assert!(csrf_tag.end_tag.is_none());
        assert!(csrf_tag.intermediate_tags.is_none());

        // Test extends tag with args
        let extends_tag = specs.get("extends").expect("extends tag should exist");
        assert!(extends_tag.end_tag.is_none());
        assert!(
            !extends_tag.args.is_empty(),
            "extends tag should have arguments"
        );
    }
}
