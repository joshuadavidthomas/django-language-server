//! Built-in Django template tag specifications.
//!
//! This module defines all the standard Django template tags as compile-time
//! constants, avoiding the need for runtime TOML parsing.

use std::borrow::Cow::Borrowed as B;
use std::sync::LazyLock;

use rustc_hash::FxHashMap;

use super::specs::EndTag;
use super::specs::IntermediateTag;
use super::specs::TagArg;
use super::specs::TagSpec;
use super::specs::TagSpecs;

// Helper macro to create const TagArg values
macro_rules! arg {
    (expr $name:expr, $required:expr) => {
        TagArg::Expr {
            name: B($name),
            required: $required,
        }
    };
    (literal $lit:expr, $required:expr) => {
        TagArg::Literal {
            lit: B($lit),
            required: $required,
        }
    };
    (string $name:expr, $required:expr) => {
        TagArg::String {
            name: B($name),
            required: $required,
        }
    };
    (var $name:expr, $required:expr) => {
        TagArg::Var {
            name: B($name),
            required: $required,
        }
    };
    (varargs $name:expr, $required:expr) => {
        TagArg::VarArgs {
            name: B($name),
            required: $required,
        }
    };
    (choice $name:expr, $required:expr, [$($choice:expr),+ $(,)?]) => {
        TagArg::Choice {
            name: B($name),
            required: $required,
            choices: B(&[$(B($choice)),+]),
        }
    };
}

// ============================================================================
// Control Flow Tags
// ============================================================================

const AUTOESCAPE_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endautoescape"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(choice "mode", true, ["on", "off"])]),
};

const IF_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endif"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[
        IntermediateTag {
            name: B("elif"),
            args: B(&[arg!(expr "condition", true)]),
        },
        IntermediateTag {
            name: B("else"),
            args: B(&[]),
        },
    ]),
    args: B(&[arg!(expr "condition", true)]),
};

const FOR_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endfor"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[IntermediateTag {
        name: B("empty"),
        args: B(&[]),
    }]),
    args: B(&[
        arg!(var "item", true),
        arg!(literal "in", true),
        arg!(var "items", true),
        arg!(literal "reversed", false),
    ]),
};

const IFCHANGED_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endifchanged"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[IntermediateTag {
        name: B("else"),
        args: B(&[]),
    }]),
    args: B(&[arg!(varargs "variables", false)]),
};

const WITH_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endwith"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(varargs "assignments", true)]),
};

// ============================================================================
// Block Tags
// ============================================================================

const BLOCK_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endblock"),
        optional: false,
        args: B(&[arg!(var "name", false)]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(var "name", true)]),
};

const EXTENDS_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[arg!(string "template", true)]),
};

const INCLUDE_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(string "template", true),
        arg!(literal "with", false),
        arg!(varargs "context", false),
        arg!(literal "only", false),
    ]),
};

const LOAD_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[arg!(varargs "libraries", true)]),
};

// ============================================================================
// Content Manipulation Tags
// ============================================================================

const COMMENT_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endcomment"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(string "note", false)]),
};

const FILTER_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endfilter"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(varargs "filters", true)]),
};

const SPACELESS_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endspaceless"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[]),
};

const VERBATIM_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endverbatim"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(string "name", false)]),
};

// ============================================================================
// Variables and Expressions
// ============================================================================

const CYCLE_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(varargs "values", true),
        arg!(literal "as", false),
        arg!(var "varname", false),
        arg!(literal "silent", false),
    ]),
};

const FIRSTOF_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(varargs "variables", true),
        arg!(string "fallback", false),
        arg!(literal "as", false),
        arg!(var "varname", false),
    ]),
};

const REGROUP_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(var "target", true),
        arg!(literal "by", true),
        arg!(var "attribute", true),
        arg!(literal "as", true),
        arg!(var "grouped", true),
    ]),
};

// ============================================================================
// Date and Time
// ============================================================================

const NOW_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(string "format_string", true),
        arg!(literal "as", false),
        arg!(var "varname", false),
    ]),
};

// ============================================================================
// URLs and Static Files
// ============================================================================

const URL_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(string "view_name", true),
        arg!(varargs "args", false),
        arg!(literal "as", false),
        arg!(var "varname", false),
    ]),
};

const STATIC_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[arg!(string "path", true)]),
};

// ============================================================================
// Template Tags
// ============================================================================

const TEMPLATETAG_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[arg!(choice "tagbit", true, [
        "openblock",
        "closeblock",
        "openvariable",
        "closevariable",
        "openbrace",
        "closebrace",
        "opencomment",
        "closecomment",
    ])]),
};

// ============================================================================
// Security
// ============================================================================

const CSRF_TOKEN_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[]),
};

// ============================================================================
// Utilities
// ============================================================================

const WIDTHRATIO_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(var "this_value", true),
        arg!(var "max_value", true),
        arg!(var "max_width", true),
        arg!(literal "as", false),
        arg!(var "varname", false),
    ]),
};

const LOREM_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(var "count", false),
        arg!(choice "method", false, ["w", "p", "b"]),
        arg!(literal "random", false),
    ]),
};

const DEBUG_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[]),
};

// ============================================================================
// Cache Tags
// ============================================================================

const CACHE_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endcache"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(var "timeout", true),
        arg!(var "cache_key", true),
        arg!(varargs "variables", false),
    ]),
};

// ============================================================================
// Internationalization
// ============================================================================

const LOCALIZE_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endlocalize"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(choice "mode", false, ["on", "off"])]),
};

const BLOCKTRANSLATE_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endblocktranslate"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[IntermediateTag {
        name: B("plural"),
        args: B(&[arg!(var "count", false)]),
    }]),
    args: B(&[
        arg!(string "context", false),
        arg!(literal "with", false),
        arg!(varargs "assignments", false),
        arg!(literal "asvar", false),
        arg!(var "varname", false),
    ]),
};

const TRANS_SPEC: TagSpec = TagSpec {
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        arg!(string "message", true),
        arg!(string "context", false),
        arg!(literal "as", false),
        arg!(var "varname", false),
        arg!(literal "noop", false),
    ]),
};

// ============================================================================
// Timezone Tags
// ============================================================================

const LOCALTIME_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endlocaltime"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(choice "mode", false, ["on", "off"])]),
};

const TIMEZONE_SPEC: TagSpec = TagSpec {
    end_tag: Some(EndTag {
        name: B("endtimezone"),
        optional: false,
        args: B(&[]),
    }),
    intermediate_tags: B(&[]),
    args: B(&[arg!(var "timezone", true)]),
};

// ============================================================================
// Static builtin map
// ============================================================================

static BUILTIN_PAIRS: &[(&str, &TagSpec)] = &[
    ("autoescape", &AUTOESCAPE_SPEC),
    ("if", &IF_SPEC),
    ("for", &FOR_SPEC),
    ("ifchanged", &IFCHANGED_SPEC),
    ("with", &WITH_SPEC),
    ("block", &BLOCK_SPEC),
    ("extends", &EXTENDS_SPEC),
    ("include", &INCLUDE_SPEC),
    ("load", &LOAD_SPEC),
    ("comment", &COMMENT_SPEC),
    ("filter", &FILTER_SPEC),
    ("spaceless", &SPACELESS_SPEC),
    ("verbatim", &VERBATIM_SPEC),
    ("cycle", &CYCLE_SPEC),
    ("firstof", &FIRSTOF_SPEC),
    ("regroup", &REGROUP_SPEC),
    ("now", &NOW_SPEC),
    ("url", &URL_SPEC),
    ("static", &STATIC_SPEC),
    ("templatetag", &TEMPLATETAG_SPEC),
    ("csrf_token", &CSRF_TOKEN_SPEC),
    ("widthratio", &WIDTHRATIO_SPEC),
    ("lorem", &LOREM_SPEC),
    ("debug", &DEBUG_SPEC),
    ("cache", &CACHE_SPEC),
    ("localize", &LOCALIZE_SPEC),
    ("blocktranslate", &BLOCKTRANSLATE_SPEC),
    ("trans", &TRANS_SPEC),
    ("localtime", &LOCALTIME_SPEC),
    ("timezone", &TIMEZONE_SPEC),
];

static BUILTIN_SPECS: LazyLock<TagSpecs> = LazyLock::new(|| {
    let mut specs = FxHashMap::default();
    for (name, spec) in BUILTIN_PAIRS {
        specs.insert((*name).to_string(), (*spec).clone());
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

        assert!(if_tag.end_tag.is_some());
        assert_eq!(if_tag.end_tag.as_ref().unwrap().name.as_ref(), "endif");

        let intermediates = &if_tag.intermediate_tags;
        assert_eq!(intermediates.len(), 2);
        assert_eq!(intermediates[0].name.as_ref(), "elif");
        assert_eq!(intermediates[1].name.as_ref(), "else");
    }

    #[test]
    fn test_for_tag_structure() {
        let specs = django_builtin_specs();
        let for_tag = specs.get("for").expect("for tag should exist");

        assert!(for_tag.end_tag.is_some());
        assert_eq!(for_tag.end_tag.as_ref().unwrap().name.as_ref(), "endfor");

        let intermediates = &for_tag.intermediate_tags;
        assert_eq!(intermediates.len(), 1);
        assert_eq!(intermediates[0].name.as_ref(), "empty");

        // Check args structure
        assert!(!for_tag.args.is_empty(), "for tag should have arguments");
    }

    #[test]
    fn test_block_tag_with_end_args() {
        let specs = django_builtin_specs();
        let block_tag = specs.get("block").expect("block tag should exist");

        let end_tag = block_tag.end_tag.as_ref().unwrap();
        assert_eq!(end_tag.name.as_ref(), "endblock");
        assert_eq!(end_tag.args.len(), 1);
        assert_eq!(end_tag.args[0].name().as_ref(), "name");
        assert!(!end_tag.args[0].is_required());
    }

    #[test]
    fn test_single_tag_structure() {
        let specs = django_builtin_specs();

        // Test a single tag has no end tag or intermediates
        let csrf_tag = specs
            .get("csrf_token")
            .expect("csrf_token tag should exist");
        assert!(csrf_tag.end_tag.is_none());
        assert!(csrf_tag.intermediate_tags.is_empty());

        // Test extends tag with args
        let extends_tag = specs.get("extends").expect("extends tag should exist");
        assert!(extends_tag.end_tag.is_none());
        assert!(
            !extends_tag.args.is_empty(),
            "extends tag should have arguments"
        );
    }
}

