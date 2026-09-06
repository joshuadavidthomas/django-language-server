use djls_semantic::ArgumentFormCoverage;
use djls_semantic::TagArgument;
use djls_semantic::TagArgumentKind;
use djls_semantic::TagArgumentSyntax;
use djls_semantic::TagSpec;

/// Generate an LSP snippet pattern from an array of tag arguments.
#[must_use]
fn generate_snippet_from_args(args: &[TagArgument]) -> String {
    let mut parts = Vec::new();
    let mut placeholder_index = 1;

    for arg in args {
        // Skip optional literals entirely - they're usually flags like "reversed" or "only"
        // that the user can add manually if needed
        if !arg.requirement.is_required() && matches!(arg.kind, TagArgumentKind::Literal(_)) {
            continue;
        }

        // Skip other optional args if we haven't seen any required args yet
        if !arg.requirement.is_required() && parts.is_empty() {
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
fn generate_snippet_for_tag(tag_name: &str, spec: &TagSpec) -> String {
    let arguments = match spec.argument_syntax() {
        TagArgumentSyntax::Signature { parameters, .. }
        | TagArgumentSyntax::Parameters(parameters) => Some(parameters.as_slice()),
        TagArgumentSyntax::Forms {
            forms,
            coverage: ArgumentFormCoverage::Complete,
        } => forms
            .iter()
            .min_by_key(|form| form.arguments.len())
            .map(|form| form.arguments.as_slice()),
        TagArgumentSyntax::Unknown
        | TagArgumentSyntax::Forms {
            coverage: ArgumentFormCoverage::Partial,
            ..
        } => None,
    };
    let args_snippet = arguments.map_or_else(String::new, generate_snippet_from_args);

    if args_snippet.is_empty() {
        tag_name.to_string()
    } else {
        format!("{tag_name} {args_snippet}")
    }
}

#[must_use]
pub(crate) fn has_full_argument_snippet(spec: &TagSpec) -> bool {
    match spec.argument_syntax() {
        TagArgumentSyntax::Signature { parameters, .. }
        | TagArgumentSyntax::Parameters(parameters) => !parameters.is_empty(),
        TagArgumentSyntax::Forms {
            forms,
            coverage: ArgumentFormCoverage::Complete,
        } => forms.iter().any(|form| !form.arguments.is_empty()),
        TagArgumentSyntax::Unknown
        | TagArgumentSyntax::Forms {
            coverage: ArgumentFormCoverage::Partial,
            ..
        } => false,
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

/// Return next parameters from forms compatible with the arguments already typed.
#[must_use]
pub(crate) fn compatible_arguments_at<'a>(
    spec: &'a TagSpec,
    completed_arguments: &[&str],
    position: usize,
) -> Vec<&'a TagArgument> {
    let sequences: Vec<&[TagArgument]> = match spec.argument_syntax() {
        TagArgumentSyntax::Unknown => Vec::new(),
        TagArgumentSyntax::Signature { parameters, .. }
        | TagArgumentSyntax::Parameters(parameters) => vec![parameters],
        TagArgumentSyntax::Forms { forms, .. } => {
            forms.iter().map(|form| form.arguments.as_slice()).collect()
        }
    };

    let mut arguments = Vec::new();
    for sequence in sequences {
        if sequence.len() <= position || !arguments_match_prefix(sequence, completed_arguments) {
            continue;
        }
        let argument = &sequence[position];
        if !arguments.contains(&argument) {
            arguments.push(argument);
        }
    }
    arguments
}

/// Generate a partial snippet from the shortest compatible syntax sequence.
#[must_use]
pub(crate) fn generate_partial_snippet(
    spec: &TagSpec,
    completed_arguments: &[&str],
    starting_from_position: usize,
) -> String {
    let sequence = match spec.argument_syntax() {
        TagArgumentSyntax::Unknown => None,
        TagArgumentSyntax::Signature { parameters, .. }
        | TagArgumentSyntax::Parameters(parameters) => Some(parameters.as_slice()),
        TagArgumentSyntax::Forms { forms, .. } => forms
            .iter()
            .filter(|form| {
                form.arguments.len() > starting_from_position
                    && arguments_match_prefix(&form.arguments, completed_arguments)
            })
            .min_by_key(|form| form.arguments.len())
            .map(|form| form.arguments.as_slice()),
    };
    let Some(sequence) = sequence else {
        return String::new();
    };
    let Some(remaining) = sequence.get(starting_from_position..) else {
        return String::new();
    };
    generate_snippet_from_args(remaining)
}

fn arguments_match_prefix(arguments: &[TagArgument], completed: &[&str]) -> bool {
    completed.len() <= arguments.len()
        && arguments
            .iter()
            .zip(completed)
            .all(|(argument, bit)| match &argument.kind {
                TagArgumentKind::Literal(value) => value == bit,
                TagArgumentKind::Choice(values) => values.iter().any(|value| value == bit),
                TagArgumentKind::Variable | TagArgumentKind::VarArgs | TagArgumentKind::Keyword => {
                    true
                }
            })
}

#[cfg(test)]
mod tests {
    use djls_semantic::EndTag;
    use djls_semantic::ParameterRequirement;
    use djls_semantic::TagArgument;
    use djls_semantic::TagArgumentForm;
    use djls_semantic::TagArgumentKind;

    use super::*;

    fn requirement(required: bool) -> ParameterRequirement {
        if required {
            ParameterRequirement::Required
        } else {
            ParameterRequirement::Optional
        }
    }

    fn make_var(name: &str, required: bool) -> TagArgument {
        TagArgument {
            name: name.to_string(),
            requirement: requirement(required),
            kind: TagArgumentKind::Variable,
        }
    }

    fn make_literal(value: &str, required: bool) -> TagArgument {
        TagArgument {
            name: value.to_string(),
            requirement: requirement(required),
            kind: TagArgumentKind::Literal(value.to_string()),
        }
    }

    fn make_choice(name: &str, required: bool, choices: Vec<&str>) -> TagArgument {
        TagArgument {
            name: name.to_string(),
            requirement: requirement(required),
            kind: TagArgumentKind::Choice(choices.into_iter().map(String::from).collect()),
        }
    }

    fn make_varargs(name: &str, required: bool) -> TagArgument {
        TagArgument {
            name: name.to_string(),
            requirement: requirement(required),
            kind: TagArgumentKind::VarArgs,
        }
    }

    #[test]
    fn test_snippet_for_for_tag() {
        let args = vec![
            make_var("item", true),
            make_literal("in", true),
            make_var("items", true),
            make_literal("reversed", false),
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:item} in ${2:items}");
    }

    #[test]
    fn test_snippet_for_if_tag() {
        let args = vec![make_var("condition", true)];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:condition}");
    }

    #[test]
    fn test_snippet_for_autoescape_tag() {
        let args = vec![make_choice("mode", true, vec!["on", "off"])];

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
            djls_semantic::BodyAnalysis::Analyze,
        )
        .with_argument_syntax(TagArgumentSyntax::Parameters(vec![make_var("name", true)]));

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
            djls_semantic::BodyAnalysis::Analyze,
        )
        .with_argument_syntax(TagArgumentSyntax::Parameters(vec![make_choice(
            "mode",
            true,
            vec!["on", "off"],
        )]));

        let snippet = generate_snippet_for_tag_with_end("autoescape", &spec);
        assert_eq!(
            snippet,
            "autoescape ${1|on,off|} %}\n$0\n{% endautoescape %}"
        );
    }

    #[test]
    fn correlated_forms_use_shortest_full_snippet_and_whole_suffix() {
        let spec = TagSpec::new(
            "django.template.defaulttags".into(),
            None,
            std::borrow::Cow::Borrowed(&[]),
            djls_semantic::BodyAnalysis::Analyze,
        )
        .with_argument_syntax(TagArgumentSyntax::Forms {
            forms: vec![
                TagArgumentForm {
                    arguments: vec![
                        make_var("this_value_expr", true),
                        make_var("max_value_expr", true),
                        make_var("max_width", true),
                    ],
                },
                TagArgumentForm {
                    arguments: vec![
                        make_var("this_value_expr", true),
                        make_var("max_value_expr", true),
                        make_var("max_width", true),
                        make_literal("as", true),
                        make_var("asvar", true),
                    ],
                },
            ],
            coverage: ArgumentFormCoverage::Complete,
        });

        assert_eq!(
            generate_snippet_for_tag_with_end("widthratio", &spec),
            "widthratio ${1:this_value_expr} ${2:max_value_expr} ${3:max_width}"
        );
        assert_eq!(
            generate_partial_snippet(&spec, &["this", "max", "width"], 3),
            "as ${1:asvar}"
        );
        assert_eq!(
            generate_partial_snippet(&spec, &["this", "max", "width", "as"], 4),
            "${1:asvar}"
        );
    }

    #[test]
    fn compatible_form_arguments_follow_earlier_literals() {
        let spec = TagSpec::new(
            "test.tags".into(),
            None,
            std::borrow::Cow::Borrowed(&[]),
            djls_semantic::BodyAnalysis::Analyze,
        )
        .with_argument_syntax(TagArgumentSyntax::Forms {
            forms: vec![
                TagArgumentForm {
                    arguments: vec![make_literal("first", true), make_literal("left", true)],
                },
                TagArgumentForm {
                    arguments: vec![make_literal("second", true), make_literal("right", true)],
                },
            ],
            coverage: ArgumentFormCoverage::Complete,
        });

        let arguments = compatible_arguments_at(&spec, &["first"], 1);
        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0].kind, TagArgumentKind::Literal("left".into()));
    }

    #[test]
    fn test_snippet_for_url_tag() {
        let args = vec![
            make_var("view_name", true),
            make_varargs("args", false),
            make_literal("as", false),
            make_var("varname", false),
        ];

        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:view_name} ${2:args} ${3:varname}");
    }
}
