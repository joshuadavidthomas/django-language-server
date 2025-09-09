use super::specs::Arg;
use super::specs::ArgType;
use super::specs::SimpleArgType;
use super::specs::TagSpec;

/// Generate an LSP snippet pattern from an array of arguments
#[must_use]
pub fn generate_snippet_from_args(args: &[Arg]) -> String {
    let mut parts = Vec::new();
    let mut placeholder_index = 1;

    for arg in args {
        // Skip optional literals entirely - they're usually flags like "reversed" or "only"
        // that the user can add manually if needed
        if !arg.required && matches!(&arg.arg_type, ArgType::Simple(SimpleArgType::Literal)) {
            continue;
        }

        // Skip other optional args if we haven't seen any required args yet
        // This prevents generating snippets like: "{% for %}" when everything is optional
        if !arg.required && parts.is_empty() {
            continue;
        }

        let snippet_part = match &arg.arg_type {
            ArgType::Simple(simple_type) => match simple_type {
                SimpleArgType::Literal => {
                    // At this point, we know it's required (optional literals were skipped above)
                    arg.name.clone()
                }
                SimpleArgType::Variable | SimpleArgType::Expression => {
                    // Variables and expressions become placeholders
                    let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
                SimpleArgType::String => {
                    // Strings get quotes around them
                    let result = format!("\"${{{}:{}}}\"", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
                SimpleArgType::Assignment => {
                    // Assignments use the name as-is (e.g., "var=value")
                    let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
                SimpleArgType::VarArgs => {
                    // Variable arguments, just use the name
                    let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
            },
            ArgType::Choice { choice } => {
                // Choice placeholders with options
                let result = format!("${{{}|{}|}}", placeholder_index, choice.join(","));
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
    use super::*;
    use crate::templatetags::specs::ArgType;
    use crate::templatetags::specs::SimpleArgType;

    #[test]
    fn test_snippet_for_for_tag() {
        let args = vec![
            Arg {
                name: "item".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Variable),
            },
            Arg {
                name: "in".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Literal),
            },
            Arg {
                name: "items".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Variable),
            },
            Arg {
                name: "reversed".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::Literal),
            },
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:item} in ${2:items}");
    }

    #[test]
    fn test_snippet_for_if_tag() {
        let args = vec![Arg {
            name: "condition".to_string(),
            required: true,
            arg_type: ArgType::Simple(SimpleArgType::Expression),
        }];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:condition}");
    }

    #[test]
    fn test_snippet_for_autoescape_tag() {
        let args = vec![Arg {
            name: "mode".to_string(),
            required: true,
            arg_type: ArgType::Choice {
                choice: vec!["on".to_string(), "off".to_string()],
            },
        }];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1|on,off|}");
    }

    #[test]
    fn test_snippet_for_extends_tag() {
        let args = vec![Arg {
            name: "template".to_string(),
            required: true,
            arg_type: ArgType::Simple(SimpleArgType::String),
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
        use crate::templatetags::specs::EndTag;
        use crate::templatetags::specs::TagSpec;

        let spec = TagSpec {
            name: None,
            end_tag: Some(EndTag {
                name: "endblock".to_string(),
                optional: false,
                args: vec![Arg {
                    name: "name".to_string(),
                    required: false,
                    arg_type: ArgType::Simple(SimpleArgType::Variable),
                }],
            }),
            intermediate_tags: None,
            args: vec![Arg {
                name: "name".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Variable),
            }],
        };

        let snippet = generate_snippet_for_tag_with_end("block", &spec);
        assert_eq!(snippet, "block ${1:name} %}\n$0\n{% endblock ${1} %}");
    }

    #[test]
    fn test_snippet_with_end_tag() {
        use crate::templatetags::specs::EndTag;
        use crate::templatetags::specs::TagSpec;

        let spec = TagSpec {
            name: None,
            end_tag: Some(EndTag {
                name: "endautoescape".to_string(),
                optional: false,
                args: vec![],
            }),
            intermediate_tags: None,
            args: vec![Arg {
                name: "mode".to_string(),
                required: true,
                arg_type: ArgType::Choice {
                    choice: vec!["on".to_string(), "off".to_string()],
                },
            }],
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
            Arg {
                name: "view_name".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::String),
            },
            Arg {
                name: "args".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::VarArgs),
            },
            Arg {
                name: "as".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::Literal),
            },
            Arg {
                name: "varname".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::Variable),
            },
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "\"${1:view_name}\" ${2:args} ${3:varname}");
    }
}
