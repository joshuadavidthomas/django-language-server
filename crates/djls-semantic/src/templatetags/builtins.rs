//! Built-in Django template tag specifications.
//!
//! This module defines all the standard Django template tags as compile-time
//! constants, avoiding the need for runtime TOML parsing.
//!
//! Note: `args` fields are empty — argument validation is handled by extracted
//! rules from the Python AST (see `rule_evaluation.rs`). The `args` field on
//! `TagSpec` is populated from extraction for completions/snippets.

use std::borrow::Cow::Borrowed as B;
use std::sync::LazyLock;

use rustc_hash::FxHashMap;

use super::specs::EndTag;
use super::specs::IntermediateTag;
use super::specs::TagSpec;
use super::specs::TagSpecs;

const DEFAULTTAGS_MOD: &str = "django.template.defaulttags";
static DEFAULTTAGS_PAIRS: &[(&str, &TagSpec)] = &[
    (
        "autoescape",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endautoescape"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "comment",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endcomment"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "csrf_token",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "cycle",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "debug",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "filter",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endfilter"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "firstof",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "for",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endfor"),
                required: true,
            }),
            intermediate_tags: B(&[IntermediateTag { name: B("empty") }]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "if",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endif"),
                required: true,
            }),
            intermediate_tags: B(&[
                IntermediateTag { name: B("elif") },
                IntermediateTag { name: B("else") },
            ]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "ifchanged",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endifchanged"),
                required: true,
            }),
            intermediate_tags: B(&[IntermediateTag { name: B("else") }]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "load",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "lorem",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "now",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    // TODO: PARTIALDEF_SPEC, 6.0+
    // TODO: PARTIAL_SPEC, 6.0+
    // TODO: QUERYSTRING_SPEC, 5.1+
    (
        "regroup",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    // TODO: RESETCYCLE_SPEC?
    (
        "spaceless",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endspaceless"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "templatetag",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "url",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "verbatim",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endverbatim"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "widthratio",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "with",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endwith"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
];

const MOD_LOADER_TAGS: &str = "django.template.loader_tags";
static LOADER_TAGS_PAIRS: &[(&str, &TagSpec)] = &[
    (
        "block",
        &TagSpec {
            module: B(MOD_LOADER_TAGS),
            end_tag: Some(EndTag {
                name: B("endblock"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "extends",
        &TagSpec {
            module: B(MOD_LOADER_TAGS),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "include",
        &TagSpec {
            module: B(MOD_LOADER_TAGS),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
];

const CACHE_MOD: &str = "django.templatetags.cache";
static CACHE_PAIRS: &[(&str, &TagSpec)] = &[(
    "cache",
    &TagSpec {
        module: B(CACHE_MOD),
        end_tag: Some(EndTag {
            name: B("endcache"),
            required: true,
        }),
        intermediate_tags: B(&[]),
        args: B(&[]),
        opaque: false,
        extracted_rules: Vec::new(),
    },
)];

const I18N_MOD: &str = "django.templatetags.i18n";
static I18N_PAIRS: &[(&str, &TagSpec)] = &[
    (
        "blocktrans",
        &TagSpec {
            module: B(I18N_MOD),
            end_tag: Some(EndTag {
                name: B("endblocktrans"),
                required: true,
            }),
            intermediate_tags: B(BLOCKTRANS_INTERMEDIATE_TAGS),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "blocktranslate",
        &TagSpec {
            module: B(I18N_MOD),
            end_tag: Some(EndTag {
                name: B("endblocktranslate"),
                required: true,
            }),
            intermediate_tags: B(BLOCKTRANS_INTERMEDIATE_TAGS),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    // TODO: GET_AVAILABLE_LANGAUGES_SPEC
    // TODO: GET_CURRENT_LANGUAGE_SPEC
    // TODO: GET_CURRENT_LANGUAGE_BIDI_SPEC
    // TODO: GET_LANGUAGE_INFO_SPEC
    // TODO: GET_LANGUAGE_INFO_LIST_SPEC
    // TODO: LANGUAGE_SPEC
    ("trans", &TRANS_SPEC),
    ("translate", &TRANS_SPEC),
];
const BLOCKTRANS_INTERMEDIATE_TAGS: &[IntermediateTag] = &[IntermediateTag { name: B("plural") }];
const TRANS_SPEC: TagSpec = TagSpec {
    module: B(I18N_MOD),
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[]),
    opaque: false,
    extracted_rules: Vec::new(),
};

const L10N_MOD: &str = "django.templatetags.l10n";
static L10N_PAIRS: &[(&str, &TagSpec)] = &[(
    "localize",
    &TagSpec {
        module: B(L10N_MOD),
        end_tag: Some(EndTag {
            name: B("endlocalize"),
            required: true,
        }),
        intermediate_tags: B(&[]),
        args: B(&[]),
        opaque: false,
        extracted_rules: Vec::new(),
    },
)];

const STATIC_MOD: &str = "django.templatetags.static";
static STATIC_PAIRS: &[(&str, &TagSpec)] = &[
    // TODO: GET_MEDIA_PREFIX_SPEC
    // TODO: GET_STATIC_PREFIX_SPEC
    (
        "static",
        &TagSpec {
            module: B(STATIC_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
];

const TZ_MOD: &str = "django.templatetags.tz";
static TZ_PAIRS: &[(&str, &TagSpec)] = &[
    // TODO: GET_CURRENT_TIMEZONE_SPEC
    (
        "localtime",
        &TagSpec {
            module: B(TZ_MOD),
            end_tag: Some(EndTag {
                name: B("endlocaltime"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
    (
        "timezone",
        &TagSpec {
            module: B(TZ_MOD),
            end_tag: Some(EndTag {
                name: B("endtimezone"),
                required: true,
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        },
    ),
];

static BUILTIN_SPECS: LazyLock<TagSpecs> = LazyLock::new(|| {
    let mut specs = FxHashMap::default();

    let all_pairs = DEFAULTTAGS_PAIRS
        .iter()
        .chain(LOADER_TAGS_PAIRS.iter())
        .chain(STATIC_PAIRS.iter())
        .chain(CACHE_PAIRS.iter())
        .chain(I18N_PAIRS.iter())
        .chain(L10N_PAIRS.iter())
        .chain(TZ_PAIRS.iter());

    for (name, spec) in all_pairs {
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
        assert!(!specs.is_empty(), "Should have loaded at least one spec");

        // Check a key tag is present as a smoke test
        assert!(specs.get("if").is_some(), "'if' tag should be present");

        // Verify all tag names are non-empty
        for (name, _) in specs {
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
    }

    #[test]
    fn test_block_tag_structure() {
        let specs = django_builtin_specs();
        let block_tag = specs.get("block").expect("block tag should exist");

        let end_tag = block_tag.end_tag.as_ref().unwrap();
        assert_eq!(end_tag.name.as_ref(), "endblock");
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

        // Test extends tag
        let extends_tag = specs.get("extends").expect("extends tag should exist");
        assert!(extends_tag.end_tag.is_none());
    }
}
