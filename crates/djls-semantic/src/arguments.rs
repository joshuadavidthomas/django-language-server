use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use salsa::Accumulator;

use crate::templatetags::TagArg;
use crate::templatetags::TagArgSliceExt;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Validate arguments for all tags in the template.
///
/// Performs a single pass over the flat `NodeList`, validating each tag's arguments
/// against its `TagSpec` definition. This is independent of block structure - we validate
/// opening tags, closing tags, intermediate tags, and standalone tags all the same way.
///
/// # Parameters
/// - `db`: The Salsa database containing tag specifications
/// - `nodelist`: The parsed template `NodeList` containing all tags
pub fn validate_all_tag_arguments(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            validate_tag_arguments(db, name, bits, marker_span);
        }
    }
}

/// Validate a single tag's arguments against its `TagSpec` definition.
///
/// This is the main entry point for tag argument validation. It looks up the tag
/// in the `TagSpecs` (checking opening tags, closing tags, and intermediate tags)
/// and delegates to the appropriate validation logic.
///
/// # Parameters
/// - `db`: The Salsa database containing tag specifications
/// - `tag_name`: The name of the tag (e.g., "if", "for", "endfor", "elif")
/// - `bits`: The tokenized arguments from the tag
/// - `span`: The span of the tag for error reporting
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    // Try to find spec for: opening tag, closing tag, or intermediate tag
    if let Some(spec) = tag_specs.get(tag_name) {
        validate_args_against_spec(db, tag_name, bits, span, spec.args.as_ref());
        return;
    }

    if let Some(end_spec) = tag_specs.get_end_spec_for_closer(tag_name) {
        validate_args_against_spec(db, tag_name, bits, span, end_spec.args.as_ref());
        return;
    }

    if let Some(intermediate) = tag_specs.get_intermediate_spec(tag_name) {
        validate_args_against_spec(db, tag_name, bits, span, intermediate.args.as_ref());
    }

    // Unknown tag - no validation (could be custom tag from unloaded library)
}

/// Validate tag arguments against an argument specification.
///
/// This function performs high-level validation (argument count) and delegates
/// to more detailed validation for argument order, choices, and literal values.
///
/// # Parameters
/// - `db`: The Salsa database
/// - `tag_name`: The name of the tag being validated
/// - `bits`: The tokenized arguments
/// - `span`: The span for error reporting
/// - `args`: The argument specification from the `TagSpec`
fn validate_args_against_spec(
    db: &dyn Db,
    tag_name: &str,
    bits: &[String],
    span: Span,
    args: &[TagArg],
) {
    // Special case: tag expects no arguments
    if args.is_empty() {
        if !bits.is_empty() {
            ValidationErrorAccumulator(ValidationError::TooManyArguments {
                tag: tag_name.to_string(),
                max: 0,
                span,
            })
            .accumulate(db);
        }
        return;
    }

    // Check minimum required arguments
    let required_count = args.iter().filter(|arg| arg.is_required()).count();
    if bits.len() < required_count {
        ValidationErrorAccumulator(ValidationError::MissingRequiredArguments {
            tag: tag_name.to_string(),
            min: required_count,
            span,
        })
        .accumulate(db);
    }

    // For varargs tags, we can't validate order/excess (they consume everything)
    // For all other tags, perform detailed positional validation
    let has_varargs = args.iter().any(|arg| matches!(arg, TagArg::VarArgs { .. }));
    if !has_varargs {
        validate_argument_order(db, tag_name, bits, span, args);
    }
}

/// Walk through arguments sequentially, consuming tokens and validating as we go.
///
/// This is the core validation logic that performs detailed argument validation by:
/// - Walking through the argument specification in order
/// - Consuming tokens from the bits array as each argument is satisfied
/// - Validating that literals appear in the correct positions
/// - Validating that choice arguments have valid values
/// - Handling multi-token expressions that consume greedily until the next literal
/// - Detecting when there are too many arguments (leftover tokens)
/// - Detecting when required arguments are missing
///
/// We can't use a simple `bits.len() > args.len()` check because Expression arguments
/// can consume multiple tokens (e.g., "x > 0" is 3 tokens, 1 arg) and this would cause
/// false positives for expressions with operators
///
/// Instead, we walk through arguments and track how many tokens each consumes,
/// then check if there are leftovers.
#[allow(clippy::too_many_lines)]
fn validate_argument_order(
    db: &dyn Db,
    tag_name: &str,
    bits: &[String],
    span: Span,
    args: &[TagArg],
) {
    let mut bit_index = 0usize;

    // Walk through argument spec, consuming tokens as we match each argument
    for (arg_index, arg) in args.iter().enumerate() {
        if bit_index >= bits.len() {
            break; // Ran out of tokens to consume
        }

        match arg {
            TagArg::Literal { lit, required, .. } => {
                // kind field is ignored for validation - it's only for semantic hints
                let matches_literal = bits[bit_index] == lit.as_ref();
                if *required {
                    if matches_literal {
                        bit_index += 1;
                    } else {
                        ValidationErrorAccumulator(ValidationError::InvalidLiteralArgument {
                            tag: tag_name.to_string(),
                            expected: lit.to_string(),
                            span,
                        })
                        .accumulate(db);
                        break; // Can't continue if required literal is wrong
                    }
                } else if matches_literal {
                    bit_index += 1; // Optional literal matched, consume it
                }
                // Optional literal didn't match - don't consume, continue
            }

            TagArg::Choice {
                name,
                required,
                choices,
            } => {
                let value = &bits[bit_index];
                if choices.iter().any(|choice| choice.as_ref() == value) {
                    bit_index += 1; // Valid choice, consume it
                } else if *required {
                    ValidationErrorAccumulator(ValidationError::InvalidArgumentChoice {
                        tag: tag_name.to_string(),
                        argument: name.to_string(),
                        choices: choices
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect(),
                        value: value.clone(),
                        span,
                    })
                    .accumulate(db);
                    break; // Can't continue if required choice is invalid
                }
                // Optional choice didn't match - don't consume, continue
            }

            TagArg::Var { .. } | TagArg::String { .. } => {
                // Simple arguments consume exactly one token
                bit_index += 1;
            }

            TagArg::Expr { .. } => {
                // Expression arguments consume tokens greedily until:
                // - We hit the next literal keyword (if any)
                // - We run out of tokens
                // Must consume at least one token

                let start_index = bit_index;
                let next_literal = args[arg_index + 1..].find_next_literal();

                // Consume tokens greedily until we hit a known literal
                while bit_index < bits.len() {
                    if let Some(ref lit) = next_literal {
                        if bits[bit_index] == *lit {
                            break; // Stop before the literal
                        }
                    }
                    bit_index += 1;
                }

                // Ensure we consumed at least one token for the expression
                if bit_index == start_index {
                    bit_index += 1;
                }
            }

            TagArg::Assignment { .. } => {
                // Assignment arguments can appear as:
                // 1. Single token: var=value
                // 2. Multi-token: expr as varname
                // Consume until we find = or "as", or hit next literal

                let next_literal = args[arg_index + 1..].find_next_literal();

                while bit_index < bits.len() {
                    let token = &bits[bit_index];
                    bit_index += 1;

                    // If token contains =, we've found the assignment
                    if token.contains('=') {
                        break;
                    }
                    crate::templatetags::TokenCount::Greedy => {
                        // Assignment arguments can appear as:
                        // 1. Single token: var=value
                        // 2. Multi-token: expr as varname
                        // Consume until we find = or "as", or hit next literal

                    // If we hit "as", consume the variable name after it
                    if token == "as" && bit_index < bits.len() {
                        bit_index += 1; // Consume the variable name
                        break;
                    }

                    // Stop if we hit the next literal argument
                    if let Some(ref lit) = next_literal {
                        if token == lit {
                            break;
                        }
                    }
                }
            }

            TagArg::VarArgs { .. } => {
                // VarArgs consumes all remaining tokens
                bit_index = bits.len();
            }
        }
    }

    // Check for unconsumed tokens (too many arguments)
    if bit_index < bits.len() {
        // We have unconsumed tokens - this is a too-many-arguments error
        // Note: VarArgs sets bit_index = bits.len(), so we never reach here for VarArgs tags
        ValidationErrorAccumulator(ValidationError::TooManyArguments {
            tag: tag_name.to_string(),
            max: args.len(), // Approximate - imperfect for Expr args but close enough
            span,
        })
        .accumulate(db);
        return;
    }

    // Check for missing required arguments that weren't satisfied
    // (Only matters if we consumed all tokens but didn't satisfy all required args)
    for arg in args.iter().skip(bit_index) {
        if arg.is_required() {
            ValidationErrorAccumulator(ValidationError::MissingArgument {
                tag: tag_name.to_string(),
                argument: arg.name().to_string(),
                span,
            })
            .accumulate(db);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use super::*;
    use crate::templatetags::django_builtin_specs;
    use crate::TagIndex;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_source::Db for TestDatabase {
        fn create_file(&self, path: &Utf8Path) -> File {
            File::new(self, path.to_owned(), 0)
        }

        fn get_file(&self, _path: &Utf8Path) -> Option<File> {
            None
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            self.fs.lock().unwrap().read_to_string(path)
        }
    }

    #[salsa::db]
    impl djls_templates::Db for TestDatabase {}

    #[salsa::db]
    impl crate::Db for TestDatabase {
        fn tag_specs(&self) -> crate::templatetags::TagSpecs {
            django_builtin_specs()
        }

        fn tag_index(&self) -> TagIndex<'_> {
            TagIndex::from_specs(self)
        }

        fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
            None
        }

        fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
            djls_conf::DiagnosticsConfig::default()
        }
    }

    /// Test helper: Create a temporary `NodeList` with a single tag and validate it
    fn check_validation_errors(
        tag_name: &str,
        bits: &[String],
        _args: &[TagArg],
    ) -> Vec<ValidationError> {
        use djls_source::Db as SourceDb;

        let db = TestDatabase::new();

        // Build a minimal template content that parses to a tag
        let bits_str = bits.join(" ");

        // Add closing tags for block tags to avoid UnclosedTag errors
        let content = match tag_name {
            "if" => format!("{{% {tag_name} {bits_str} %}}{{% endif %}}"),
            "for" => format!("{{% {tag_name} {bits_str} %}}{{% endfor %}}"),
            "with" => format!("{{% {tag_name} {bits_str} %}}{{% endwith %}}"),
            "block" => format!("{{% {tag_name} {bits_str} %}}{{% endblock %}}"),
            "autoescape" => format!("{{% {tag_name} {bits_str} %}}{{% endautoescape %}}"),
            "filter" => format!("{{% {tag_name} {bits_str} %}}{{% endfilter %}}"),
            "spaceless" => format!("{{% {tag_name} {bits_str} %}}{{% endspaceless %}}"),
            "verbatim" => format!("{{% {tag_name} {bits_str} %}}{{% endverbatim %}}"),
            _ => format!("{{% {tag_name} {bits_str} %}}"),
        };

        // Create a file and parse it
        let path = camino::Utf8Path::new("/test.html");
        db.fs.lock().unwrap().add_file(path.to_owned(), content);

        let file = db.create_file(path);
        let nodelist = djls_templates::parse_template(&db, file).expect("Failed to parse template");

        // Validate through the normal path
        crate::validate_nodelist(&db, nodelist);

        // Collect accumulated errors, filtering out UnclosedTag errors (test setup issue)
        crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(|err| !matches!(err, ValidationError::UnclosedTag { .. }))
            .collect()
    }

    #[test]
    fn test_if_tag_with_comparison_operator() {
        // Issue #1: {% if message.input_tokens > 0 %}
        // Parser tokenizes as: ["message.input_tokens", ">", "0"]
        // Spec expects: [Any{name="condition", count=Greedy}]
        let bits = vec![
            "message.input_tokens".to_string(),
            ">".to_string(),
            "0".to_string(),
        ];
        let args = vec![TagArg::expr("condition", true)];

        let errors = check_validation_errors("if", &bits, &args);
        assert!(
            errors.is_empty(),
            "Should not error on expression with multiple tokens: {errors:?}"
        );
    }

    #[test]
    fn test_translate_with_quoted_string() {
        // Issue #2: {% translate "Contact the owner of the site" %}
        // Parser tokenizes as: ['"Contact the owner of the site"'] (single token now!)
        let bits = vec![r#""Contact the owner of the site""#.to_string()];
        let args = vec![TagArg::String {
            name: "message".into(),
            required: true,
        }];

        let errors = check_validation_errors("translate", &bits, &args);
        assert!(
            errors.is_empty(),
            "Should not error on quoted string: {errors:?}"
        );
    }

    #[test]
    fn test_for_tag_with_reversed() {
        // {% for item in items reversed %}
        let bits = vec![
            "item".to_string(),
            "in".to_string(),
            "items".to_string(),
            "reversed".to_string(),
        ];
        let args = vec![
            TagArg::var("item", true),
            TagArg::syntax("in", true),
            TagArg::var("items", true),
            TagArg::modifier("reversed", false),
        ];

        let errors = check_validation_errors("for", &bits, &args);
        assert!(
            errors.is_empty(),
            "Should handle optional literal 'reversed': {errors:?}"
        );
    }

    #[test]
    fn test_if_complex_expression() {
        // {% if user.is_authenticated and user.is_staff %}
        let bits = vec![
            "user.is_authenticated".to_string(),
            "and".to_string(),
            "user.is_staff".to_string(),
        ];
        let args = vec![TagArg::expr("condition", true)];

        let errors = check_validation_errors("if", &bits, &args);
        assert!(
            errors.is_empty(),
            "Should handle complex boolean expression: {errors:?}"
        );
    }

    #[test]
    fn test_url_with_multiple_args() {
        // {% url 'view_name' arg1 arg2 arg3 %}
        let bits = vec![
            "'view_name'".to_string(),
            "arg1".to_string(),
            "arg2".to_string(),
            "arg3".to_string(),
        ];
        let args = vec![
            TagArg::String {
                name: "view_name".into(),
                required: true,
            },
            TagArg::VarArgs {
                name: "args".into(),
                required: false,
            },
        ];

        let errors = check_validation_errors("url", &bits, &args);
        assert!(errors.is_empty(), "Should handle varargs: {errors:?}");
    }

    #[test]
    fn test_with_assignment() {
        // {% with total=items|length %}
        let bits = vec!["total=items|length".to_string()];
        let args = vec![TagArg::assignment("bindings", true)];

        let errors = check_validation_errors("with", &bits, &args);
        assert!(
            errors.is_empty(),
            "Should handle assignment with filter: {errors:?}"
        );
    }

    #[test]
    fn test_include_with_quoted_path() {
        // {% include "partials/header.html" %}
        let bits = vec![r#""partials/header.html""#.to_string()];
        let args = vec![TagArg::String {
            name: "template".into(),
            required: true,
        }];

        let errors = check_validation_errors("include", &bits, &args);
        assert!(errors.is_empty(), "Should handle quoted path: {errors:?}");
    }

    #[test]
    fn test_expr_stops_at_literal() {
        // {% if condition reversed %} - "reversed" should not be consumed by expr
        let bits = vec![
            "x".to_string(),
            ">".to_string(),
            "0".to_string(),
            "reversed".to_string(),
        ];
        let args = vec![
            TagArg::expr("condition", true),
            TagArg::modifier("reversed", false),
        ];

        let errors = check_validation_errors("if", &bits, &args);
        assert!(
            errors.is_empty(),
            "Expr should stop before literal keyword: {errors:?}"
        );
    }

    #[test]
    fn test_tag_with_no_args_rejects_extra() {
        // {% csrf_token extra_arg %}
        // csrf_token expects no arguments
        let bits = vec!["extra_arg".to_string()];
        let args = vec![]; // No arguments expected

        let errors = check_validation_errors("csrf_token", &bits, &args);
        assert_eq!(errors.len(), 1, "Should error on unexpected argument");
        assert!(
            matches!(errors[0], ValidationError::TooManyArguments { max: 0, .. }),
            "Expected TooManyArguments error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn test_tag_with_no_args_rejects_multiple_extra() {
        // {% debug arg1 arg2 arg3 %}
        // debug expects no arguments
        let bits = vec!["arg1".to_string(), "arg2".to_string(), "arg3".to_string()];
        let args = vec![]; // No arguments expected

        let errors = check_validation_errors("debug", &bits, &args);
        assert_eq!(errors.len(), 1, "Should error once for extra arguments");
        assert!(
            matches!(errors[0], ValidationError::TooManyArguments { max: 0, .. }),
            "Expected TooManyArguments error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn test_autoescape_rejects_extra_args() {
        // {% autoescape on extra_arg %}
        // autoescape expects exactly 1 choice argument (on/off)
        let bits = vec!["on".to_string(), "extra_arg".to_string()];
        let args = vec![TagArg::Choice {
            name: "mode".into(),
            required: true,
            choices: vec!["on".into(), "off".into()].into(),
        }];

        let errors = check_validation_errors("autoescape", &bits, &args);
        // This is the key test - does the real implementation catch too many args?
        assert!(
            !errors.is_empty(),
            "Should error on extra argument after choice"
        );
    }

    #[test]
    fn test_csrf_token_rejects_extra_args() {
        // {% csrf_token "extra" %}
        // csrf_token expects no arguments
        let bits = vec![r#""extra""#.to_string()];
        let args = vec![];

        let errors = check_validation_errors("csrf_token", &bits, &args);
        assert!(
            !errors.is_empty(),
            "Should error on extra argument for zero-arg tag"
        );
        assert!(
            matches!(errors[0], ValidationError::TooManyArguments { .. }),
            "Expected TooManyArguments, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn test_now_rejects_extra_args() {
        // {% now "Y-m-d" as varname extra_arg %}
        // now expects: format, optional "as" + varname, then nothing more
        let bits = vec![
            r#""Y-m-d""#.to_string(),
            "as".to_string(),
            "varname".to_string(),
            "extra_arg".to_string(),
        ];
        let args = vec![];

        let errors = check_validation_errors("now", &bits, &args);
        // Should have too many arguments after consuming all valid args
        assert!(
            !errors.is_empty(),
            "Should error when extra arg appears after complete argument list"
        );
    }

    #[test]
    fn test_load_accepts_varargs() {
        // {% load tag1 tag2 tag3 from library %}
        // load has varargs, so should accept many arguments
        let bits = vec![
            "tag1".to_string(),
            "tag2".to_string(),
            "tag3".to_string(),
            "from".to_string(),
            "library".to_string(),
        ];
        let args = vec![
            TagArg::varargs("tags", false),
            TagArg::syntax("from", false),
            TagArg::var("library", false),
        ];

        let errors = check_validation_errors("load", &bits, &args);
        assert!(
            errors.is_empty(),
            "VarArgs should accept many arguments: {errors:?}"
        );
    }

    #[test]
    fn test_regroup_rejects_extra_args() {
        // {% regroup list by attr as varname extra_arg %}
        // regroup expects: list, "by", attr, "as", varname - no more
        let bits = vec![
            "list".to_string(),
            "by".to_string(),
            "attr".to_string(),
            "as".to_string(),
            "varname".to_string(),
            "extra_arg".to_string(),
        ];
        let args = vec![];

        let errors = check_validation_errors("regroup", &bits, &args);
        assert!(
            !errors.is_empty(),
            "Should error on extra argument after complete regroup args"
        );
    }
}
