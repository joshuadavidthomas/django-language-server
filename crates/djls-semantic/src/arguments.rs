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
    let mut args_processed = 0usize;

    // Walk through argument spec, consuming tokens as we match each argument
    for (arg_index, arg) in args.iter().enumerate() {
        if bit_index >= bits.len() {
            break; // Ran out of tokens to consume
        }

        args_processed = arg_index + 1;

        match arg {
            TagArg::Literal { lit, required, .. } => {
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

            TagArg::String { .. } => {
                // String arguments consume exactly one token
                bit_index += 1;
            }

            TagArg::Variable {
                name,
                required,
                count,
            }
            | TagArg::Any {
                name,
                required,
                count,
            } => {
                match count {
                    crate::templatetags::TokenCount::Exact(n) => {
                        let available = bits.len().saturating_sub(bit_index);
                        if available < *n {
                            if *required {
                                ValidationErrorAccumulator(ValidationError::MissingArgument {
                                    tag: tag_name.to_string(),
                                    argument: name.to_string(),
                                    span,
                                })
                                .accumulate(db);
                                return;
                            }
                            continue;
                        }

                        // Consume exactly N tokens
                        bit_index += n;
                    }
                    crate::templatetags::TokenCount::Greedy => {
                        // Greedy: consume tokens until next literal or end
                        let start_index = bit_index;
                        let next_literal = args[arg_index + 1..].find_next_literal();

                        while bit_index < bits.len() {
                            if let Some(ref lit) = next_literal {
                                if bits[bit_index] == *lit {
                                    break; // Stop before the literal
                                }
                            }
                            bit_index += 1;
                        }

                        // Ensure we consumed at least one token
                        if bit_index == start_index {
                            let is_next_literal = next_literal
                                .as_ref()
                                .is_some_and(|lit| bits.get(bit_index) == Some(lit));
                            if !is_next_literal {
                                bit_index += 1;
                            }
                        }
                    }
                }
            }

            TagArg::Assignment {
                name,
                required,
                count,
            } => {
                match count {
                    crate::templatetags::TokenCount::Exact(n) => {
                        let available = bits.len().saturating_sub(bit_index);
                        if available < *n {
                            if *required {
                                ValidationErrorAccumulator(ValidationError::MissingArgument {
                                    tag: tag_name.to_string(),
                                    argument: name.to_string(),
                                    span,
                                })
                                .accumulate(db);
                                return;
                            }
                            continue;
                        }

                        // Consume exactly N tokens
                        bit_index += n;
                    }
                    crate::templatetags::TokenCount::Greedy => {
                        let next_literal = args[arg_index + 1..].find_next_literal();

                        while bit_index < bits.len() {
                            if let Some(ref lit) = next_literal {
                                if bits[bit_index] == *lit {
                                    break;
                                }
                            }

                            let token = &bits[bit_index];

                            if token.contains('=') {
                                bit_index += 1;
                            } else {
                                break;
                            }
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
    for arg in args.iter().skip(args_processed) {
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
        custom_specs: Option<crate::templatetags::TagSpecs>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                custom_specs: None,
            }
        }

        fn with_custom_specs(specs: crate::templatetags::TagSpecs) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                custom_specs: Some(specs),
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
            self.custom_specs
                .clone()
                .unwrap_or_else(django_builtin_specs)
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
        check_validation_errors_with_db(tag_name, bits, TestDatabase::new())
    }

    /// Test helper: Create a temporary `NodeList` with a single tag and validate it using custom specs
    #[allow(clippy::needless_pass_by_value)]
    fn check_validation_errors_with_db(
        tag_name: &str,
        bits: &[String],
        db: TestDatabase,
    ) -> Vec<ValidationError> {
        use djls_source::Db as SourceDb;

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
    fn test_with_multiple_greedy_assignments() {
        // {% with a=1 b=2 c=3 %}
        let bits = vec!["a=1".to_string(), "b=2".to_string(), "c=3".to_string()];
        let args = vec![TagArg::assignment("bindings", true)];

        let errors = check_validation_errors("with", &bits, &args);
        assert!(
            errors.is_empty(),
            "Should handle multiple greedy assignments: {errors:?}"
        );
    }

    #[test]
    fn test_greedy_consumes_all_leaving_required_literal_unsatisfied() {
        // {% customcond x > 0 %} - greedy expr consumes all, missing required "reversed" literal
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;
        use crate::TokenCount;

        // Create custom spec: customcond expects [Greedy expr, required "reversed" literal]
        let mut specs = FxHashMap::default();
        specs.insert(
            "customcond".to_string(),
            TagSpec {
                module: Cow::Borrowed("test.module"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endcustomcond"),
                    required: true,
                    args: Cow::Borrowed(&[]),
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Owned(vec![
                    TagArg::Any {
                        name: Cow::Borrowed("condition"),
                        required: true,
                        count: TokenCount::Greedy,
                    },
                    TagArg::modifier("reversed", true),
                ]),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Input: 3 tokens, greedy consumes all, required literal missing
        let bits = vec!["x".to_string(), ">".to_string(), "0".to_string()];

        let errors = check_validation_errors_with_db("customcond", &bits, db);

        assert!(
            !errors.is_empty(),
            "Should error when greedy arg consumes all tokens, leaving required literal unsatisfied"
        );

        // The error should be MissingArgument for "reversed"
        let has_missing_reversed = errors.iter().any(|err| {
            matches!(err, ValidationError::MissingArgument { argument, .. }
                if argument == "reversed")
        });
        assert!(
            has_missing_reversed,
            "Should have MissingArgument error for 'reversed', got: {errors:?}"
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

    #[test]
    fn test_exact_token_count_insufficient_tokens_required() {
        // {% for item %} - missing required "in" and items arguments
        let bits = vec!["item".to_string()];
        let args = vec![
            TagArg::var("item", true),
            TagArg::syntax("in", true),
            TagArg::var("items", true),
        ];

        let errors = check_validation_errors("for", &bits, &args);
        assert!(
            !errors.is_empty(),
            "Should error when required arguments are missing"
        );
        assert!(
            matches!(errors[0], ValidationError::MissingRequiredArguments { .. }),
            "Expected MissingRequiredArguments, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn test_exact_token_count_does_not_overflow() {
        // {% for item in %} - missing required items argument
        let bits = vec!["item".to_string(), "in".to_string()];
        let args = vec![
            TagArg::var("item", true),
            TagArg::syntax("in", true),
            TagArg::var("items", true), // Missing this required arg
        ];

        let errors = check_validation_errors("for", &bits, &args);
        // Should detect missing required argument (caught by count check)
        assert!(
            !errors.is_empty(),
            "Should error when required 'items' argument is missing"
        );
        assert!(
            matches!(errors[0], ValidationError::MissingRequiredArguments { .. }),
            "Expected MissingRequiredArguments, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn test_skip_bit_index_bug_with_exact_multi_token() {
        // {% customtag a b as %} - Exact(2) consumes 2 tokens, missing required "result" argument
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;
        use crate::TokenCount;

        // Create custom spec: customtag expects [Exact(2), "as", Variable]
        let mut specs = FxHashMap::default();
        specs.insert(
            "customtag".to_string(),
            TagSpec {
                module: Cow::Borrowed("test.module"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endcustomtag"),
                    required: true,
                    args: Cow::Borrowed(&[]),
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Owned(vec![
                    TagArg::Variable {
                        name: Cow::Borrowed("pair"),
                        required: true,
                        count: TokenCount::Exact(2),
                    },
                    TagArg::syntax("as", true),
                    TagArg::var("result", true),
                ]),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Input: 3 tokens, passes early count check (3 required args = 3 tokens)
        // but doesn't satisfy all positional requirements
        let bits = vec!["a".to_string(), "b".to_string(), "as".to_string()];

        let errors = check_validation_errors_with_db("customtag", &bits, db);

        // BUG: Currently this test will FAIL because skip(bit_index) doesn't detect
        // the missing "result" argument
        assert!(
            !errors.is_empty(),
            "Should error when required 'result' argument is missing"
        );

        // The error should be MissingArgument for "result"
        let has_missing_result = errors.iter().any(|err| {
            matches!(err, ValidationError::MissingArgument { argument, .. }
                if argument == "result")
        });
        assert!(
            has_missing_result,
            "Should have MissingArgument error for 'result', got: {errors:?}"
        );
    }
}
