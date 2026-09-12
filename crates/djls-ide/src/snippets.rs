use djls_project::TagArgumentPattern;
use djls_project::TagArgumentPatternKind;
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

fn generate_snippet_from_pattern(arguments: &[&TagArgumentPattern]) -> String {
    let mut parts = Vec::new();
    let mut placeholder_index = 1;
    for argument in arguments {
        let part = match &argument.kind {
            TagArgumentPatternKind::Literal(value) => value.clone(),
            TagArgumentPatternKind::Choice(values) => {
                let result = format!("${{{}|{}|}}", placeholder_index, values.join(","));
                placeholder_index += 1;
                result
            }
            TagArgumentPatternKind::Variable
            | TagArgumentPatternKind::VariableWidth { .. }
            | TagArgumentPatternKind::VariableExcept(_) => {
                let result = format!("${{{}:{}}}", placeholder_index, argument.name);
                placeholder_index += 1;
                result
            }
        };
        parts.push(part);
    }
    parts.join(" ")
}

fn expanded_pattern(
    form: &djls_project::TagArgumentForm,
    length: usize,
) -> Option<Vec<&TagArgumentPattern>> {
    let variable = form
        .pattern()
        .iter()
        .position(|argument| matches!(argument.kind, TagArgumentPatternKind::VariableWidth { .. }));
    match variable {
        None => (length == form.pattern().len()).then(|| form.pattern().iter().collect()),
        Some(variable) => {
            if length < form.minimum_len() {
                return None;
            }
            let repeated = length - (form.pattern().len() - 1);
            let mut expanded = Vec::with_capacity(length);
            expanded.extend(form.pattern()[..variable].iter());
            expanded.extend(std::iter::repeat_n(&form.pattern()[variable], repeated));
            expanded.extend(form.pattern()[variable + 1..].iter());
            Some(expanded)
        }
    }
}

fn minimum_form_completion<'a>(
    form: &'a djls_project::TagArgumentForm,
    completed: &[&str],
) -> Option<Vec<&'a TagArgumentPattern>> {
    let maximum = completed.len() + form.pattern().len() + form.minimum_len() + 1;
    (form.minimum_len().max(completed.len())..=maximum).find_map(|length| {
        let expanded = expanded_pattern(form, length)?;
        expanded
            .iter()
            .zip(completed)
            .all(|(argument, bit)| argument.kind.matches(bit))
            .then(|| expanded.into_iter().skip(completed.len()).collect())
    })
}

/// Generate a complete LSP snippet for a tag including the tag name
#[must_use]
fn generate_snippet_for_tag(tag_name: &str, spec: &TagSpec) -> String {
    let args_snippet = match spec.argument_syntax() {
        TagArgumentSyntax::Signature { parameters, .. }
        | TagArgumentSyntax::Parameters(parameters) => generate_snippet_from_args(parameters),
        TagArgumentSyntax::Forms {
            forms,
            coverage: ArgumentFormCoverage::Complete,
            ..
        } => forms
            .iter()
            .min_by_key(|form| {
                (
                    form.minimum_len(),
                    form.pattern()
                        .iter()
                        .filter(|argument| {
                            matches!(argument.kind, TagArgumentPatternKind::Literal(_))
                        })
                        .count(),
                )
            })
            .and_then(|form| expanded_pattern(form, form.minimum_len()))
            .map_or_else(String::new, |arguments| {
                generate_snippet_from_pattern(&arguments)
            }),
        TagArgumentSyntax::Unknown
        | TagArgumentSyntax::Assignments { .. }
        | TagArgumentSyntax::Forms {
            coverage: ArgumentFormCoverage::Partial,
            ..
        } => String::new(),
    };

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
            ..
        } => forms.iter().any(|form| form.minimum_len() > 0),
        TagArgumentSyntax::Unknown
        | TagArgumentSyntax::Assignments { .. }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompatibleArgumentKind<'a> {
    Literal(&'a str),
    Choice(&'a [String]),
    Variable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompatibleArgument<'a> {
    pub name: &'a str,
    pub kind: CompatibleArgumentKind<'a>,
}

/// Return next parameters from syntax compatible with the arguments already typed.
#[must_use]
pub(crate) fn compatible_arguments_at<'a>(
    spec: &'a TagSpec,
    completed_arguments: &[&str],
    position: usize,
) -> Vec<CompatibleArgument<'a>> {
    let mut arguments = Vec::new();
    match spec.argument_syntax() {
        TagArgumentSyntax::Unknown | TagArgumentSyntax::Assignments { .. } => {}
        TagArgumentSyntax::Signature { parameters, .. }
        | TagArgumentSyntax::Parameters(parameters) => {
            if parameters.len() > position
                && arguments_match_prefix(parameters, completed_arguments)
            {
                let argument = &parameters[position];
                let kind = match &argument.kind {
                    TagArgumentKind::Literal(value) => CompatibleArgumentKind::Literal(value),
                    TagArgumentKind::Choice(values) => CompatibleArgumentKind::Choice(values),
                    TagArgumentKind::Variable
                    | TagArgumentKind::Keyword
                    | TagArgumentKind::VarArgs => CompatibleArgumentKind::Variable,
                };
                arguments.push(CompatibleArgument {
                    name: &argument.name,
                    kind,
                });
            }
        }
        TagArgumentSyntax::Forms { forms, .. } => {
            for form in forms {
                for continuation in form.prefix_continuations(completed_arguments) {
                    let argument = continuation.argument;
                    let kind = match &argument.kind {
                        TagArgumentPatternKind::Literal(value) => {
                            CompatibleArgumentKind::Literal(value)
                        }
                        TagArgumentPatternKind::Choice(values) => {
                            CompatibleArgumentKind::Choice(values)
                        }
                        TagArgumentPatternKind::Variable
                        | TagArgumentPatternKind::VariableWidth { .. }
                        | TagArgumentPatternKind::VariableExcept(_) => {
                            CompatibleArgumentKind::Variable
                        }
                    };
                    let candidate = CompatibleArgument {
                        name: &argument.name,
                        kind,
                    };
                    if !arguments.contains(&candidate) {
                        arguments.push(candidate);
                    }
                }
            }
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
    match spec.argument_syntax() {
        TagArgumentSyntax::Unknown | TagArgumentSyntax::Assignments { .. } => String::new(),
        TagArgumentSyntax::Signature { parameters, .. }
        | TagArgumentSyntax::Parameters(parameters) => parameters
            .get(starting_from_position..)
            .filter(|_| arguments_match_prefix(parameters, completed_arguments))
            .map_or_else(String::new, generate_snippet_from_args),
        TagArgumentSyntax::Forms { forms, .. } => forms
            .iter()
            .filter_map(|form| minimum_form_completion(form, completed_arguments))
            .filter(|remaining| !remaining.is_empty())
            .min_by_key(Vec::len)
            .map_or_else(String::new, |remaining| {
                generate_snippet_from_pattern(&remaining)
            }),
    }
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

    fn make_form(arguments: Vec<TagArgument>) -> TagArgumentForm {
        TagArgumentForm::new(
            arguments
                .into_iter()
                .map(|argument| TagArgumentPattern {
                    name: argument.name,
                    kind: match argument.kind {
                        TagArgumentKind::Variable
                        | TagArgumentKind::Keyword
                        | TagArgumentKind::VarArgs => TagArgumentPatternKind::Variable,
                        TagArgumentKind::Literal(value) => TagArgumentPatternKind::Literal(value),
                        TagArgumentKind::Choice(values) => TagArgumentPatternKind::Choice(values),
                    },
                    mismatch_message: None,
                })
                .collect(),
        )
        .expect("fixed-width test form is valid")
    }

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
                make_form(vec![
                    make_var("this_value_expr", true),
                    make_var("max_value_expr", true),
                    make_var("max_width", true),
                ]),
                make_form(vec![
                    make_var("this_value_expr", true),
                    make_var("max_value_expr", true),
                    make_var("max_width", true),
                    make_literal("as", true),
                    make_var("asvar", true),
                ]),
            ],
            coverage: ArgumentFormCoverage::Complete,
            length_mismatch_message: None,
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
                make_form(vec![
                    make_literal("first", true),
                    make_literal("left", true),
                ]),
                make_form(vec![
                    make_literal("second", true),
                    make_literal("right", true),
                ]),
            ],
            coverage: ArgumentFormCoverage::Complete,
            length_mismatch_message: None,
        });

        let arguments = compatible_arguments_at(&spec, &["first"], 1);
        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0].kind, CompatibleArgumentKind::Literal("left"));
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
