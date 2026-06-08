use djls_semantic::TagArgument;
use djls_semantic::TagArgumentKind;
use djls_semantic::TagSpec;

/// Generate an LSP snippet pattern from an array of tag arguments.
#[must_use]
pub(crate) fn generate_snippet_from_args(args: &[TagArgument]) -> String {
    let mut parts = Vec::new();
    let mut placeholder_index = 1;

    for arg in args {
        // Skip optional literals entirely - they're usually flags like "reversed" or "only"
        // that the user can add manually if needed
        if !arg.required && matches!(arg.kind, TagArgumentKind::Literal(_)) {
            continue;
        }

        // Skip other optional args if we haven't seen any required args yet
        if !arg.required && parts.is_empty() {
            continue;
        }

        let snippet_part = match &arg.kind {
            TagArgumentKind::Literal(value) => {
                // At this point, we know it's required (optional literals were skipped above)
                value.clone()
            }
            TagArgumentKind::Variable | TagArgumentKind::Keyword | TagArgumentKind::VarArgs => {
                let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                placeholder_index += 1;
                result
            }
            TagArgumentKind::Choice(choices) => {
                let options: Vec<&str> = choices.iter().map(String::as_str).collect();
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
pub(crate) fn generate_snippet_for_tag(tag_name: &str, spec: &TagSpec) -> String {
    let args = spec.arguments();

    let args_snippet = generate_snippet_from_args(&args);

    if args_snippet.is_empty() {
        tag_name.to_string()
    } else {
        format!("{tag_name} {args_snippet}")
    }
}

/// Generate a complete LSP snippet for a tag including the tag name and closing tag if needed
#[must_use]
pub(crate) fn generate_snippet_for_tag_with_end(tag_name: &str, spec: &TagSpec) -> String {
    // Special handling for block tag to mirror the name in endblock
    if tag_name == "block" {
        let snippet = String::from("block ${1:name} %}\n$0\n{% endblock ${1} %}");
        return snippet;
    }

    let mut snippet = generate_snippet_for_tag(tag_name, spec);

    // If this tag has a required end tag, include it in the snippet
    if let Some(end_tag) = &spec.end_tag
        && end_tag.required
    {
        snippet.push_str(" %}\n$0\n{% ");
        snippet.push_str(&end_tag.name);
        snippet.push_str(" %}");
    }

    snippet
}

/// Generate a partial snippet starting from a specific argument position
#[must_use]
pub(crate) fn generate_partial_snippet(spec: &TagSpec, starting_from_position: usize) -> String {
    let args = spec.arguments();

    if starting_from_position >= args.len() {
        return String::new();
    }

    let remaining_args = &args[starting_from_position..];
    generate_snippet_from_args(remaining_args)
}

#[cfg(test)]
mod tests {
    use djls_semantic::EndTag;
    use djls_semantic::TagArgument;
    use djls_semantic::TagArgumentKind;

    use super::*;

    fn make_var(name: &str, required: bool, pos: usize) -> TagArgument {
        TagArgument {
            name: name.to_string(),
            required,
            kind: TagArgumentKind::Variable,
            position: pos,
        }
    }

    fn make_literal(value: &str, required: bool, pos: usize) -> TagArgument {
        TagArgument {
            name: value.to_string(),
            required,
            kind: TagArgumentKind::Literal(value.to_string()),
            position: pos,
        }
    }

    fn make_choice(name: &str, required: bool, choices: Vec<&str>, pos: usize) -> TagArgument {
        TagArgument {
            name: name.to_string(),
            required,
            kind: TagArgumentKind::Choice(choices.into_iter().map(String::from).collect()),
            position: pos,
        }
    }

    fn make_varargs(name: &str, required: bool, pos: usize) -> TagArgument {
        TagArgument {
            name: name.to_string(),
            required,
            kind: TagArgumentKind::VarArgs,
            position: pos,
        }
    }

    #[test]
    fn test_snippet_for_for_tag() {
        let args = vec![
            make_var("item", true, 0),
            make_literal("in", true, 1),
            make_var("items", true, 2),
            make_literal("reversed", false, 3),
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:item} in ${2:items}");
    }

    #[test]
    fn test_snippet_for_if_tag() {
        let args = vec![make_var("condition", true, 0)];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:condition}");
    }

    #[test]
    fn test_snippet_for_autoescape_tag() {
        let args = vec![make_choice("mode", true, vec!["on", "off"], 0)];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1|on,off|}");
    }

    #[test]
    fn test_snippet_for_csrf_token_tag() {
        let args: Vec<TagArgument> = vec![];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "");
    }

    #[test]
    fn test_snippet_for_block_tag() {
        use std::borrow::Cow;

        let spec = TagSpec::new(
            "django.template.loader_tags".into(),
            Some(EndTag {
                name: "endblock".into(),
                required: true,
            }),
            Cow::Borrowed(&[]),
            false,
        )
        .with_arguments(vec![make_var("name", true, 0)]);

        let snippet = generate_snippet_for_tag_with_end("block", &spec);
        assert_eq!(snippet, "block ${1:name} %}\n$0\n{% endblock ${1} %}");
    }

    #[test]
    fn test_snippet_with_end_tag() {
        use std::borrow::Cow;

        let spec = TagSpec::new(
            "django.template.defaulttags".into(),
            Some(EndTag {
                name: "endautoescape".into(),
                required: true,
            }),
            Cow::Borrowed(&[]),
            false,
        )
        .with_arguments(vec![make_choice("mode", true, vec!["on", "off"], 0)]);

        let snippet = generate_snippet_for_tag_with_end("autoescape", &spec);
        assert_eq!(
            snippet,
            "autoescape ${1|on,off|} %}\n$0\n{% endautoescape %}"
        );
    }

    #[test]
    fn test_snippet_for_url_tag() {
        let args = vec![
            make_var("view_name", true, 0),
            make_varargs("args", false, 1),
            make_literal("as", false, 2),
            make_var("varname", false, 3),
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:view_name} ${2:args} ${3:varname}");
    }
}
