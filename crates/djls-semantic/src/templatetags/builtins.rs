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

const DEFAULTTAGS_MOD: &str = "django.template.defaulttags";
static DEFAULTTAGS_PAIRS: &[(&str, &TagSpec)] = &[
    (
        "autoescape",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endautoescape"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::Choice {
                name: B("mode"),
                required: true,
                choices: B(&[B("on"), B("off")]),
            }]),
        },
    ),
    (
        "comment",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endcomment"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::String {
                name: B("note"),
                required: false,
            }]),
        },
    ),
    (
        "csrf_token",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
        },
    ),
    (
        "cycle",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[
                TagArg::VarArgs {
                    name: B("values"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("as"),
                    required: false,
                },
                TagArg::Var {
                    name: B("varname"),
                    required: false,
                },
                TagArg::Literal {
                    lit: B("silent"),
                    required: false,
                },
            ]),
        },
    ),
    (
        "debug",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[]),
        },
    ),
    (
        "filter",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endfilter"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::VarArgs {
                name: B("filters"),
                required: true,
            }]),
        },
    ),
    (
        "firstof",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[
                TagArg::VarArgs {
                    name: B("variables"),
                    required: true,
                },
                TagArg::String {
                    name: B("fallback"),
                    required: false,
                },
                TagArg::Literal {
                    lit: B("as"),
                    required: false,
                },
                TagArg::Var {
                    name: B("varname"),
                    required: false,
                },
            ]),
        },
    ),
    (
        "for",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
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
                TagArg::Var {
                    name: B("item"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("in"),
                    required: true,
                },
                TagArg::Var {
                    name: B("items"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("reversed"),
                    required: false,
                },
            ]),
        },
    ),
    (
        "if",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endif"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[
                IntermediateTag {
                    name: B("elif"),
                    args: B(&[TagArg::Expr {
                        name: B("condition"),
                        required: true,
                    }]),
                },
                IntermediateTag {
                    name: B("else"),
                    args: B(&[]),
                },
            ]),
            args: B(&[TagArg::Expr {
                name: B("condition"),
                required: true,
            }]),
        },
    ),
    (
        "ifchanged",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endifchanged"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[IntermediateTag {
                name: B("else"),
                args: B(&[]),
            }]),
            args: B(&[TagArg::VarArgs {
                name: B("variables"),
                required: false,
            }]),
        },
    ),
    (
        "load",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[TagArg::VarArgs {
                name: B("libraries"),
                required: true,
            }]),
        },
    ),
    (
        "lorem",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[
                TagArg::Var {
                    name: B("count"),
                    required: false,
                },
                TagArg::Choice {
                    name: B("method"),
                    required: false,
                    choices: B(&[B("w"), B("p"), B("b")]),
                },
                TagArg::Literal {
                    lit: B("random"),
                    required: false,
                },
            ]),
        },
    ),
    (
        "now",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[
                TagArg::String {
                    name: B("format_string"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("as"),
                    required: false,
                },
                TagArg::Var {
                    name: B("varname"),
                    required: false,
                },
            ]),
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
            args: B(&[
                TagArg::Var {
                    name: B("target"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("by"),
                    required: true,
                },
                TagArg::Var {
                    name: B("attribute"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("as"),
                    required: true,
                },
                TagArg::Var {
                    name: B("grouped"),
                    required: true,
                },
            ]),
        },
    ),
    // TODO: RESETCYCLE_SPEC?
    (
        "spaceless",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endspaceless"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[]),
        },
    ),
    (
        "templatetag",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[TagArg::Choice {
                name: B("tagbit"),
                required: true,
                choices: B(&[
                    B("openblock"),
                    B("closeblock"),
                    B("openvariable"),
                    B("closevariable"),
                    B("openbrace"),
                    B("closebrace"),
                    B("opencomment"),
                    B("closecomment"),
                ]),
            }]),
        },
    ),
    (
        "url",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[
                TagArg::String {
                    name: B("view_name"),
                    required: true,
                },
                TagArg::VarArgs {
                    name: B("args"),
                    required: false,
                },
                TagArg::Literal {
                    lit: B("as"),
                    required: false,
                },
                TagArg::Var {
                    name: B("varname"),
                    required: false,
                },
            ]),
        },
    ),
    (
        "verbatim",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endverbatim"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::String {
                name: B("name"),
                required: false,
            }]),
        },
    ),
    (
        "widthratio",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[
                TagArg::Var {
                    name: B("this_value"),
                    required: true,
                },
                TagArg::Var {
                    name: B("max_value"),
                    required: true,
                },
                TagArg::Var {
                    name: B("max_width"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("as"),
                    required: false,
                },
                TagArg::Var {
                    name: B("varname"),
                    required: false,
                },
            ]),
        },
    ),
    (
        "with",
        &TagSpec {
            module: B(DEFAULTTAGS_MOD),
            end_tag: Some(EndTag {
                name: B("endwith"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::VarArgs {
                name: B("assignments"),
                required: true,
            }]),
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
                optional: false,
                args: B(&[TagArg::Var {
                    name: B("name"),
                    required: false,
                }]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::Var {
                name: B("name"),
                required: true,
            }]),
        },
    ),
    (
        "extends",
        &TagSpec {
            module: B(MOD_LOADER_TAGS),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[TagArg::String {
                name: B("template"),
                required: true,
            }]),
        },
    ),
    (
        "include",
        &TagSpec {
            module: B(MOD_LOADER_TAGS),
            end_tag: None,
            intermediate_tags: B(&[]),
            args: B(&[
                TagArg::String {
                    name: B("template"),
                    required: true,
                },
                TagArg::Literal {
                    lit: B("with"),
                    required: false,
                },
                TagArg::VarArgs {
                    name: B("context"),
                    required: false,
                },
                TagArg::Literal {
                    lit: B("only"),
                    required: false,
                },
            ]),
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
            optional: false,
            args: B(&[]),
        }),
        intermediate_tags: B(&[]),
        args: B(&[
            TagArg::Var {
                name: B("timeout"),
                required: true,
            },
            TagArg::Var {
                name: B("cache_key"),
                required: true,
            },
            TagArg::VarArgs {
                name: B("variables"),
                required: false,
            },
        ]),
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
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(BLOCKTRANS_INTERMEDIATE_TAGS),
            args: B(BLOCKTRANS_ARGS),
        },
    ),
    (
        "blocktranslate",
        &TagSpec {
            module: B(I18N_MOD),
            end_tag: Some(EndTag {
                name: B("endblocktranslate"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(BLOCKTRANS_INTERMEDIATE_TAGS),
            args: B(BLOCKTRANS_ARGS),
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
const BLOCKTRANS_INTERMEDIATE_TAGS: &[IntermediateTag] = &[IntermediateTag {
    name: B("plural"),
    args: B(&[TagArg::Var {
        name: B("count"),
        required: false,
    }]),
}];
const BLOCKTRANS_ARGS: &[TagArg] = &[
    TagArg::String {
        name: B("context"),
        required: false,
    },
    TagArg::Literal {
        lit: B("with"),
        required: false,
    },
    TagArg::VarArgs {
        name: B("assignments"),
        required: false,
    },
    TagArg::Literal {
        lit: B("asvar"),
        required: false,
    },
    TagArg::Var {
        name: B("varname"),
        required: false,
    },
];
const TRANS_SPEC: TagSpec = TagSpec {
    module: B(I18N_MOD),
    end_tag: None,
    intermediate_tags: B(&[]),
    args: B(&[
        TagArg::String {
            name: B("message"),
            required: true,
        },
        TagArg::String {
            name: B("context"),
            required: false,
        },
        TagArg::Literal {
            lit: B("as"),
            required: false,
        },
        TagArg::Var {
            name: B("varname"),
            required: false,
        },
        TagArg::Literal {
            lit: B("noop"),
            required: false,
        },
    ]),
};

const L10N_MOD: &str = "django.templatetags.l10n";
static L10N_PAIRS: &[(&str, &TagSpec)] = &[(
    "localize",
    &TagSpec {
        module: B(L10N_MOD),
        end_tag: Some(EndTag {
            name: B("endlocalize"),
            optional: false,
            args: B(&[]),
        }),
        intermediate_tags: B(&[]),
        args: B(&[TagArg::Choice {
            name: B("mode"),
            required: false,
            choices: B(&[B("on"), B("off")]),
        }]),
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
            args: B(&[TagArg::String {
                name: B("path"),
                required: true,
            }]),
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
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::Choice {
                name: B("mode"),
                required: false,
                choices: B(&[B("on"), B("off")]),
            }]),
        },
    ),
    (
        "timezone",
        &TagSpec {
            module: B(TZ_MOD),
            end_tag: Some(EndTag {
                name: B("endtimezone"),
                optional: false,
                args: B(&[]),
            }),
            intermediate_tags: B(&[]),
            args: B(&[TagArg::Var {
                name: B("timezone"),
                required: true,
            }]),
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
