use djls_semantic::TagArg;
use djls_semantic::TagSpec;

/// Generate an LSP snippet pattern from an array of arguments
#[must_use]
pub fn generate_snippet_from_args(args: &[TagArg]) -> String {
    let mut parts = Vec::new();
    let mut placeholder_index = 1;

    for arg in args {
        // Skip optional literals entirely - they're usually flags like "reversed" or "only"
        // that the user can add manually if needed
        if !arg.is_required() && matches!(arg, TagArg::Literal { .. }) {
            continue;
        }

        // Skip other optional args if we haven't seen any required args yet
        // This prevents generating snippets like: "{% for %}" when everything is optional
        if !arg.is_required() && parts.is_empty() {
            continue;
        }

        let snippet_part = match arg {
            TagArg::Literal { lit, .. } => {
                // At this point, we know it's required (optional literals were skipped above)
                lit.to_string()
            }
            TagArg::Var { name, .. } | TagArg::Expr { name, .. } => {
                // Variables and expressions become placeholders
                let result = format!("${{{}:{}}}", placeholder_index, name.as_ref());
                placeholder_index += 1;
                result
            }
            TagArg::String { name, .. } => {
                // Strings get quotes around them
                let result = format!("\"${{{}:{}}}\"", placeholder_index, name.as_ref());
                placeholder_index += 1;
                result
            }
            TagArg::Assignment { name, .. } => {
                // Assignments use the name as-is (e.g., "var=value")
                let result = format!("${{{}:{}}}", placeholder_index, name.as_ref());
                placeholder_index += 1;
                result
            }
            TagArg::VarArgs { name, .. } => {
                // Variable arguments, just use the name
                let result = format!("${{{}:{}}}", placeholder_index, name.as_ref());
                placeholder_index += 1;
                result
            }
            TagArg::Choice { choices, .. } => {
                // Choice placeholders with options
                let options: Vec<_> = choices.iter().map(std::convert::AsRef::as_ref).collect();
                let result = format!("${{{}|{}|}}", placeholder_index, options.join(","));
                placeholder_index += 1;
                result
            }
        };

        parts.push(snippet_part);
    }

    parts.join(" ")
}

/// Generate a complete LSP snippet for a tag including the tag name
#[must_use]
pub fn generate_snippet_for_tag(tag_name: &str, spec: &TagSpec) -> String {
    let args_snippet = generate_snippet_from_args(&spec.args);

    if args_snippet.is_empty() {
        // Tag with no arguments
        tag_name.to_string()
    } else {
        // Tag with arguments
        format!("{tag_name} {args_snippet}")
    }
}

/// Generate a complete LSP snippet for a tag including the tag name and closing tag if needed
#[must_use]
pub fn generate_snippet_for_tag_with_end(tag_name: &str, spec: &TagSpec) -> String {
    // Special handling for block tag to mirror the name in endblock
    if tag_name == "block" {
        // LSP snippets support placeholder mirroring using the same number
        // ${1:name} in opening tag will be mirrored to ${1} in closing tag
        let snippet = String::from("block ${1:name} %}\n$0\n{% endblock ${1} %}");
        return snippet;
    }

    let mut snippet = generate_snippet_for_tag(tag_name, spec);

    // If this tag has a required end tag, include it in the snippet
    if let Some(end_tag) = &spec.end_tag {
        if !end_tag.optional {
            // Add closing %} for the opening tag, newline, cursor position, newline, then end tag
            snippet.push_str(" %}\n$0\n{% ");
            snippet.push_str(&end_tag.name);
            snippet.push_str(" %}");
        }
    }

    snippet
}

/// Generate a partial snippet starting from a specific argument position
/// This is useful when the user has already typed some arguments
#[must_use]
pub fn generate_partial_snippet(spec: &TagSpec, starting_from_position: usize) -> String {
    if starting_from_position >= spec.args.len() {
        return String::new();
    }

    let remaining_args = &spec.args[starting_from_position..];
    generate_snippet_from_args(remaining_args)
}

#[cfg(test)]
mod tests {
    use djls_semantic::EndTag;

    use super::*;

    #[test]
    fn test_snippet_for_for_tag() {
        let args = vec![
            TagArg::Var {
                name: "item".into(),
                required: true,
            },
            TagArg::Literal {
                lit: "in".into(),
                required: true,
            },
            TagArg::Var {
                name: "items".into(),
                required: true,
            },
            TagArg::Literal {
                lit: "reversed".into(),
                required: false,
            },
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:item} in ${2:items}");
    }

    #[test]
    fn test_snippet_for_if_tag() {
        let args = vec![TagArg::Expr {
            name: "condition".into(),
            required: true,
        }];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:condition}");
    }

    #[test]
    fn test_snippet_for_autoescape_tag() {
        let args = vec![TagArg::Choice {
            name: "mode".into(),
            required: true,
            choices: vec!["on".into(), "off".into()].into(),
        }];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1|on,off|}");
    }

    #[test]
    fn test_snippet_for_extends_tag() {
        let args = vec![TagArg::String {
            name: "template".into(),
            required: true,
        }];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "\"${1:template}\"");
    }

    #[test]
    fn test_snippet_for_csrf_token_tag() {
        let args = vec![];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "");
    }

    #[test]
    fn test_snippet_for_block_tag() {
        use std::borrow::Cow;

        let spec = TagSpec {
            module: "django.template.loader_tags".into(),
            end_tag: Some(EndTag {
                name: "endblock".into(),
                optional: false,
                args: vec![TagArg::Var {
                    name: "name".into(),
                    required: false,
                }]
                .into(),
            }),
            intermediate_tags: Cow::Borrowed(&[]),
            args: vec![TagArg::Var {
                name: "name".into(),
                required: true,
            }]
            .into(),
        };

        let snippet = generate_snippet_for_tag_with_end("block", &spec);
        assert_eq!(snippet, "block ${1:name} %}\n$0\n{% endblock ${1} %}");
    }

    #[test]
    fn test_snippet_with_end_tag() {
        use std::borrow::Cow;

        let spec = TagSpec {
            module: "django.template.defaulttags".into(),
            end_tag: Some(EndTag {
                name: "endautoescape".into(),
                optional: false,
                args: Cow::Borrowed(&[]),
            }),
            intermediate_tags: Cow::Borrowed(&[]),
            args: vec![TagArg::Choice {
                name: "mode".into(),
                required: true,
                choices: vec!["on".into(), "off".into()].into(),
            }]
            .into(),
        };

        let snippet = generate_snippet_for_tag_with_end("autoescape", &spec);
        assert_eq!(
            snippet,
            "autoescape ${1|on,off|} %}\n$0\n{% endautoescape %}"
        );
    }

    #[test]
    fn test_snippet_for_url_tag() {
        let args = vec![
            TagArg::String {
                name: "view_name".into(),
                required: true,
            },
            TagArg::VarArgs {
                name: "args".into(),
                required: false,
            },
            TagArg::Literal {
                lit: "as".into(),
                required: false,
            },
            TagArg::Var {
                name: "varname".into(),
                required: false,
            },
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "\"${1:view_name}\" ${2:args} ${3:varname}");
    }
}
