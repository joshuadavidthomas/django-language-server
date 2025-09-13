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

/// Type alias for argument specification
type ArgSpec = (&'static str, bool, ArgType);

/// Builder for creating tag specifications with a fluent API
struct TagBuilder {
    name: &'static str,
    end_tag: Option<(&'static str, Vec<ArgSpec>)>,
    intermediate_tags: Vec<&'static str>,
    args: Vec<ArgSpec>,
}

impl TagBuilder {
    fn new(name: &'static str) -> Self {
        TagBuilder {
            name,
            end_tag: None,
            intermediate_tags: Vec::new(),
            args: Vec::new(),
        }
    }

    fn with_end(mut self, end_name: &'static str) -> Self {
        self.end_tag = Some((end_name, Vec::new()));
        self
    }

    fn with_end_args(
        mut self,
        end_name: &'static str,
        args: Vec<(&'static str, bool, ArgType)>,
    ) -> Self {
        self.end_tag = Some((end_name, args));
        self
    }

    fn with_intermediate(mut self, tags: Vec<&'static str>) -> Self {
        self.intermediate_tags = tags;
        self
    }

    fn with_args(mut self, args: Vec<ArgSpec>) -> Self {
        self.args = args;
        self
    }

    fn build(self) -> TagSpec {
        TagSpec {
            name: Some(self.name.to_string()),
            end_tag: self.end_tag.map(|(name, args)| EndTag {
                name: name.to_string(),
                optional: false,
                args: args
                    .into_iter()
                    .map(|(name, required, arg_type)| Arg {
                        name: name.to_string(),
                        required,
                        arg_type,
                    })
                    .collect(),
            }),
            intermediate_tags: if self.intermediate_tags.is_empty() {
                None
            } else {
                Some(
                    self.intermediate_tags
                        .into_iter()
                        .map(|name| IntermediateTag {
                            name: name.to_string(),
                        })
                        .collect(),
                )
            },
            args: self
                .args
                .into_iter()
                .map(|(name, required, arg_type)| Arg {
                    name: name.to_string(),
                    required,
                    arg_type,
                })
                .collect(),
        }
    }
}

// Helper functions for common argument types
fn var(name: &'static str) -> ArgSpec {
    (name, true, ArgType::Simple(SimpleArgType::Variable))
}

fn opt_var(name: &'static str) -> ArgSpec {
    (name, false, ArgType::Simple(SimpleArgType::Variable))
}

fn literal(name: &'static str) -> ArgSpec {
    (name, true, ArgType::Simple(SimpleArgType::Literal))
}

fn opt_literal(name: &'static str) -> ArgSpec {
    (name, false, ArgType::Simple(SimpleArgType::Literal))
}

fn string(name: &'static str) -> ArgSpec {
    (name, true, ArgType::Simple(SimpleArgType::String))
}

fn opt_string(name: &'static str) -> ArgSpec {
    (name, false, ArgType::Simple(SimpleArgType::String))
}

fn expr(name: &'static str) -> ArgSpec {
    (name, true, ArgType::Simple(SimpleArgType::Expression))
}

fn varargs(name: &'static str) -> ArgSpec {
    (name, true, ArgType::Simple(SimpleArgType::VarArgs))
}

fn opt_varargs(name: &'static str) -> ArgSpec {
    (name, false, ArgType::Simple(SimpleArgType::VarArgs))
}

fn choice(name: &'static str, choices: Vec<String>) -> ArgSpec {
    (name, true, ArgType::Choice { choice: choices })
}

fn opt_choice(name: &'static str, choices: Vec<String>) -> ArgSpec {
    (name, false, ArgType::Choice { choice: choices })
}

// Static storage for built-in specs - built only once on first access
static BUILTIN_SPECS: LazyLock<TagSpecs> = LazyLock::new(|| {
    let mut specs = HashMap::new();

    // Define all Django built-in tags
    let tags = vec![
        // Control flow tags
        TagBuilder::new("autoescape")
            .with_end("endautoescape")
            .with_args(vec![choice(
                "mode",
                vec!["on".to_string(), "off".to_string()],
            )])
            .build(),
        TagBuilder::new("if")
            .with_end("endif")
            .with_intermediate(vec!["elif", "else"])
            .with_args(vec![expr("condition")])
            .build(),
        TagBuilder::new("for")
            .with_end("endfor")
            .with_intermediate(vec!["empty"])
            .with_args(vec![
                var("item"),
                literal("in"),
                var("items"),
                opt_literal("reversed"),
            ])
            .build(),
        TagBuilder::new("ifchanged")
            .with_end("endifchanged")
            .with_intermediate(vec!["else"])
            .with_args(vec![opt_varargs("variables")])
            .build(),
        TagBuilder::new("with")
            .with_end("endwith")
            .with_args(vec![varargs("assignments")])
            .build(),
        // Block tags
        TagBuilder::new("block")
            .with_end_args("endblock", vec![opt_var("name")])
            .with_args(vec![var("name")])
            .build(),
        TagBuilder::new("extends")
            .with_args(vec![string("template")])
            .build(),
        TagBuilder::new("include")
            .with_args(vec![
                string("template"),
                opt_literal("with"),
                opt_varargs("context"),
                opt_literal("only"),
            ])
            .build(),
        // Comments and literals
        TagBuilder::new("comment").with_end("endcomment").build(),
        TagBuilder::new("verbatim")
            .with_end("endverbatim")
            .with_args(vec![opt_string("name")])
            .build(),
        TagBuilder::new("spaceless")
            .with_end("endspaceless")
            .build(),
        // Template loading
        TagBuilder::new("load")
            .with_args(vec![varargs("libraries")])
            .build(),
        // CSRF token
        TagBuilder::new("csrf_token").build(),
        // Filters
        TagBuilder::new("filter")
            .with_end("endfilter")
            .with_args(vec![expr("filter_expr")])
            .build(),
        // Variables and display
        TagBuilder::new("cycle")
            .with_args(vec![
                varargs("values"),
                opt_literal("as"),
                opt_var("varname"),
            ])
            .build(),
        TagBuilder::new("firstof")
            .with_args(vec![varargs("variables")])
            .build(),
        TagBuilder::new("regroup")
            .with_args(vec![
                var("list"),
                literal("by"),
                var("attribute"),
                literal("as"),
                var("grouped"),
            ])
            .build(),
        // Date and time
        TagBuilder::new("now")
            .with_args(vec![
                string("format_string"),
                opt_literal("as"),
                opt_var("varname"),
            ])
            .build(),
        // URLs and static files
        TagBuilder::new("url")
            .with_args(vec![
                string("view_name"),
                opt_varargs("args"),
                opt_literal("as"),
                opt_var("varname"),
            ])
            .build(),
        TagBuilder::new("static")
            .with_args(vec![string("path")])
            .build(),
        // Template tags
        TagBuilder::new("templatetag")
            .with_args(vec![choice(
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
            )])
            .build(),
        // Utilities
        TagBuilder::new("widthratio")
            .with_args(vec![
                var("this_value"),
                var("max_value"),
                var("max_width"),
                opt_literal("as"),
                opt_var("varname"),
            ])
            .build(),
        TagBuilder::new("lorem")
            .with_args(vec![
                opt_var("count"),
                opt_choice(
                    "method",
                    vec!["w".to_string(), "p".to_string(), "b".to_string()],
                ),
                opt_literal("random"),
            ])
            .build(),
        TagBuilder::new("debug").build(),
        // Cache tags
        TagBuilder::new("cache")
            .with_end("endcache")
            .with_args(vec![
                var("timeout"),
                var("cache_key"),
                opt_varargs("variables"),
            ])
            .build(),
        // Internationalization
        TagBuilder::new("localize")
            .with_end("endlocalize")
            .with_args(vec![opt_choice(
                "mode",
                vec!["on".to_string(), "off".to_string()],
            )])
            .build(),
        TagBuilder::new("blocktranslate")
            .with_end("endblocktranslate")
            .with_intermediate(vec!["plural"])
            .with_args(vec![
                opt_string("context"),
                opt_literal("with"),
                opt_varargs("assignments"),
                opt_literal("asvar"),
                opt_var("varname"),
            ])
            .build(),
        TagBuilder::new("trans")
            .with_args(vec![
                string("message"),
                opt_string("context"),
                opt_literal("as"),
                opt_var("varname"),
                opt_literal("noop"),
            ])
            .build(),
        // Timezone tags
        TagBuilder::new("localtime")
            .with_end("endlocaltime")
            .with_args(vec![opt_choice(
                "mode",
                vec!["on".to_string(), "off".to_string()],
            )])
            .build(),
        TagBuilder::new("timezone")
            .with_end("endtimezone")
            .with_args(vec![var("timezone")])
            .build(),
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
