//! Built-in Django template tag specifications.
//!
//! This module defines all the standard Django template tags as compile-time
//! constants, avoiding the need for runtime TOML parsing.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::specs::Arg;
use super::specs::EndTag;
use super::specs::IntermediateTag;
use super::specs::TagSpec;
use super::ArgType;
use super::SimpleArgType;
use super::TagSpecs;

// Helper functions for creating Arg structs
fn var(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: true,
        arg_type: ArgType::Simple(SimpleArgType::Variable),
    }
}

fn opt_var(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: false,
        arg_type: ArgType::Simple(SimpleArgType::Variable),
    }
}

fn literal(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: true,
        arg_type: ArgType::Simple(SimpleArgType::Literal),
    }
}

fn opt_literal(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: false,
        arg_type: ArgType::Simple(SimpleArgType::Literal),
    }
}

fn string(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: true,
        arg_type: ArgType::Simple(SimpleArgType::String),
    }
}

fn opt_string(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: false,
        arg_type: ArgType::Simple(SimpleArgType::String),
    }
}

fn expr(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: true,
        arg_type: ArgType::Simple(SimpleArgType::Expression),
    }
}

fn varargs(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: true,
        arg_type: ArgType::Simple(SimpleArgType::VarArgs),
    }
}

fn opt_varargs(name: &'static str) -> Arg {
    Arg {
        name: name.to_string(),
        required: false,
        arg_type: ArgType::Simple(SimpleArgType::VarArgs),
    }
}

fn choice(name: &'static str, choices: Vec<String>) -> Arg {
    Arg {
        name: name.to_string(),
        required: true,
        arg_type: ArgType::Choice { choice: choices },
    }
}

fn opt_choice(name: &'static str, choices: Vec<String>) -> Arg {
    Arg {
        name: name.to_string(),
        required: false,
        arg_type: ArgType::Choice { choice: choices },
    }
}

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
            args: vec![choice("mode", vec!["on".to_string(), "off".to_string()])],
        },
        TagSpec {
            name: Some("if".to_string()),
            end_tag: Some(EndTag {
                name: "endif".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: Some(vec![
                IntermediateTag { name: "elif".to_string() },
                IntermediateTag { name: "else".to_string() },
            ]),
            args: vec![expr("condition")],
        },
        TagSpec {
            name: Some("for".to_string()),
            end_tag: Some(EndTag {
                name: "endfor".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: Some(vec![
                IntermediateTag { name: "empty".to_string() },
            ]),
            args: vec![
                var("item"),
                literal("in"),
                var("items"),
                opt_literal("reversed"),
            ],
        },
        TagSpec {
            name: Some("ifchanged".to_string()),
            end_tag: Some(EndTag {
                name: "endifchanged".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: Some(vec![
                IntermediateTag { name: "else".to_string() },
            ]),
            args: vec![opt_varargs("variables")],
        },
        TagSpec {
            name: Some("with".to_string()),
            end_tag: Some(EndTag {
                name: "endwith".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![varargs("assignments")],
        },
        
        // Block tags
        TagSpec {
            name: Some("block".to_string()),
            end_tag: Some(EndTag {
                name: "endblock".to_string(),
                optional: false,
                args: vec![opt_var("name")],
            }),
            intermediate_tags: None,
            args: vec![var("name")],
        },
        TagSpec {
            name: Some("extends".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![string("template")],
        },
        TagSpec {
            name: Some("include".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                string("template"),
                opt_literal("with"),
                opt_varargs("context"),
                opt_literal("only"),
            ],
        },
        TagSpec {
            name: Some("load".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![varargs("libraries")],
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
            args: vec![opt_string("note")],
        },
        TagSpec {
            name: Some("filter".to_string()),
            end_tag: Some(EndTag {
                name: "endfilter".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![varargs("filters")],
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
            args: vec![opt_string("name")],
        },
        
        // Variables and expressions
        TagSpec {
            name: Some("cycle".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                varargs("values"),
                opt_literal("as"),
                opt_var("varname"),
                opt_literal("silent"),
            ],
        },
        TagSpec {
            name: Some("firstof".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                varargs("variables"),
                opt_string("fallback"),
                opt_literal("as"),
                opt_var("varname"),
            ],
        },
        TagSpec {
            name: Some("regroup".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                var("target"),
                literal("by"),
                var("attribute"),
                literal("as"),
                var("grouped"),
            ],
        },
        
        // Date and time
        TagSpec {
            name: Some("now".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                string("format_string"),
                opt_literal("as"),
                opt_var("varname"),
            ],
        },
        
        // URLs and static files
        TagSpec {
            name: Some("url".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                string("view_name"),
                opt_varargs("args"),
                opt_literal("as"),
                opt_var("varname"),
            ],
        },
        TagSpec {
            name: Some("static".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![string("path")],
        },
        
        // Template tags
        TagSpec {
            name: Some("templatetag".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![choice(
                "tagbit",
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
                var("this_value"),
                var("max_value"),
                var("max_width"),
                opt_literal("as"),
                opt_var("varname"),
            ],
        },
        TagSpec {
            name: Some("lorem".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                opt_var("count"),
                opt_choice("method", vec!["w".to_string(), "p".to_string(), "b".to_string()]),
                opt_literal("random"),
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
                var("timeout"),
                var("cache_key"),
                opt_varargs("variables"),
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
            args: vec![opt_choice("mode", vec!["on".to_string(), "off".to_string()])],
        },
        TagSpec {
            name: Some("blocktranslate".to_string()),
            end_tag: Some(EndTag {
                name: "endblocktranslate".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: Some(vec![
                IntermediateTag { name: "plural".to_string() },
            ]),
            args: vec![
                opt_string("context"),
                opt_literal("with"),
                opt_varargs("assignments"),
                opt_literal("asvar"),
                opt_var("varname"),
            ],
        },
        TagSpec {
            name: Some("trans".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: vec![
                string("message"),
                opt_string("context"),
                opt_literal("as"),
                opt_var("varname"),
                opt_literal("noop"),
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
            args: vec![opt_choice("mode", vec!["on".to_string(), "off".to_string()])],
        },
        TagSpec {
            name: Some("timezone".to_string()),
            end_tag: Some(EndTag {
                name: "endtimezone".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![var("timezone")],
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
            let spec = specs.get(tag).unwrap_or_else(|| panic!("{tag} tag should be present"));
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
        let csrf_tag = specs.get("csrf_token").expect("csrf_token tag should exist");
        assert!(csrf_tag.end_tag.is_none());
        assert!(csrf_tag.intermediate_tags.is_none());

        // Test extends tag with args
        let extends_tag = specs.get("extends").expect("extends tag should exist");
        assert!(extends_tag.end_tag.is_none());
        assert!(!extends_tag.args.is_empty(), "extends tag should have arguments");
    }
}