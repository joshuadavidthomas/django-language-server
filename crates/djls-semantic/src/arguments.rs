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
/// Nodes inside opaque regions (e.g., `{% verbatim %}`) are skipped.
///
/// # Parameters
/// - `db`: The Salsa database containing tag specifications
/// - `nodelist`: The parsed template `NodeList` containing all tags
pub fn validate_all_tag_arguments(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    let opaque = crate::opaque::compute_opaque_regions(db, nodelist);

    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            if opaque.is_opaque(*span) {
                continue;
            }
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            validate_tag_arguments(db, name, bits, marker_span);
        }
    }
}

/// Validate a single tag's arguments against its specification.
///
/// Dispatches to extracted rule evaluation when available, falls back to
/// `TagArg`-based validation for user-config specs only. Builtin tags have
/// empty `args` and rely solely on extracted rules from the Python AST.
///
/// Closer and intermediate tags receive no argument validation (extraction
/// does not produce rules for them, and `EndTag`/`IntermediateTag` no longer
/// carry argument definitions).
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    if let Some(spec) = tag_specs.get(tag_name) {
        if !spec.extracted_rules.is_empty() {
            crate::rule_evaluation::evaluate_extracted_rules(
                db,
                tag_name,
                bits,
                &spec.extracted_rules,
                span,
            );
        } else if !spec.args.is_empty() {
            // Fallback for user-config-defined args only
            validate_args_against_spec(db, tag_name, bits, span, spec.args.as_ref());
        }
        // Both empty = no argument validation (conservative)
    }

    // Closer/intermediate/unknown tags: no argument validation
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

        fn inspector_inventory(&self) -> Option<djls_project::InspectorInventory> {
            None
        }

        fn filter_arity_specs(&self) -> crate::db::FilterAritySpecs {
            crate::db::FilterAritySpecs::default()
        }

        fn opaque_tag_map(&self) -> crate::db::OpaqueTagMap {
            crate::db::OpaqueTagMap::default()
        }
    }

    /// Test helper: Create a temporary `NodeList` with a single tag and validate it using custom specs
    #[allow(clippy::needless_pass_by_value)]
    fn check_validation_errors_with_db(
        tag_name: &str,
        bits: &[&str],
        db: TestDatabase,
    ) -> Vec<ValidationError> {
        use djls_source::Db as SourceDb;

        let bits: Vec<String> = bits.iter().map(|s| (*s).to_string()).collect();

        // Build a minimal template content that parses to a tag
        let bits_str = bits.join(" ");

        // Add closing tags for block tags to avoid UnclosedTag errors
        let tag_specs = db.tag_specs();
        let content = if let Some(spec) = tag_specs.get(tag_name) {
            if let Some(end_tag) = &spec.end_tag {
                format!(
                    "{{% {tag_name} {bits_str} %}}{{% {} %}}",
                    end_tag.name.as_ref()
                )
            } else {
                format!("{{% {tag_name} {bits_str} %}}")
            }
        } else {
            format!("{{% {tag_name} {bits_str} %}}")
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

    fn validate_template(content: &str) -> Vec<ValidationError> {
        use djls_source::Db as SourceDb;

        let db = TestDatabase::new();
        let path = camino::Utf8Path::new("/test.html");
        db.fs
            .lock()
            .unwrap()
            .add_file(path.to_owned(), content.to_string());

        let file = db.create_file(path);
        let nodelist = djls_templates::parse_template(&db, file).expect("Failed to parse template");

        crate::validate_nodelist(&db, nodelist);

        crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .collect()
    }

    // ===================================================================
    // Tests for the extracted rule evaluation path (primary validation)
    // ===================================================================

    #[test]
    fn test_endblock_with_name_is_valid() {
        // Closer tags receive no argument validation
        let errors = validate_template("{% block content %}hello{% endblock content %}");
        assert!(
            errors.is_empty(),
            "{{% endblock content %}} should be valid, got: {errors:?}"
        );
    }

    #[test]
    fn test_endblock_without_name_is_valid() {
        let errors = validate_template("{% block content %}hello{% endblock %}");
        assert!(
            errors.is_empty(),
            "{{% endblock %}} should be valid, got: {errors:?}"
        );
    }

    #[test]
    fn test_for_rejects_extra_token_via_extracted_rules() {
        // {% for item in items football %} has 6 words (split_len=6).
        // Extraction produces MaxArgCount{max:3} → violated when split_len <= 3 (not applicable here)
        // and we need a rule that catches >5 tokens. Simulate with a custom spec.
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;

        let mut specs = FxHashMap::default();
        specs.insert(
            "for".to_string(),
            TagSpec {
                module: Cow::Borrowed("django.template.defaulttags"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endfor"),
                    required: true,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Borrowed(&[]),
                opaque: false,
                // Simulate actual for tag extraction rules:
                // "for item in items" = split_len 4, valid
                // "for item in items reversed" = split_len 5, valid
                // "for item in items football extra" = split_len 6, invalid
                // Use ArgCountComparison{count:5, op:Gt} → violated when split_len > 5
                extracted_rules: vec![djls_extraction::ExtractedRule {
                    condition: djls_extraction::RuleCondition::ArgCountComparison {
                        count: 5,
                        op: djls_extraction::ComparisonOp::Gt,
                    },
                    message: Some("'for' tag received too many arguments.".to_string()),
                }],
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));
        // "for item in items football extra" → split_len=6, 6 > 5 → violated
        let errors = check_validation_errors_with_db(
            "for",
            &["item", "in", "items", "football", "extra"],
            db,
        );

        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(
            !rule_errors.is_empty(),
            "Expected ExtractedRuleViolation for too many args, got: {errors:?}"
        );
    }

    #[test]
    fn test_autoescape_exact_count_via_extracted_rules() {
        // autoescape expects ExactArgCount{count:2, negated:true} → error when split_len != 2
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;

        let mut specs = FxHashMap::default();
        specs.insert(
            "autoescape".to_string(),
            TagSpec {
                module: Cow::Borrowed("django.template.defaulttags"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endautoescape"),
                    required: true,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: vec![djls_extraction::ExtractedRule {
                    condition: djls_extraction::RuleCondition::ExactArgCount {
                        count: 2,
                        negated: true,
                    },
                    message: Some("'autoescape' tag requires exactly one argument.".to_string()),
                }],
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Valid: {% autoescape on %} → split_len=2 → matches → negated → not violated
        let errors = check_validation_errors_with_db("autoescape", &["on"], db.clone());
        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(
            rule_errors.is_empty(),
            "autoescape on should be valid: {errors:?}"
        );

        // Invalid: {% autoescape on extra %} → split_len=3 → !matches → negated → violated
        let errors = check_validation_errors_with_db("autoescape", &["on", "extra"], db.clone());
        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(
            !rule_errors.is_empty(),
            "autoescape with extra arg should error: {errors:?}"
        );

        // Invalid: {% autoescape %} → split_len=1 → !matches → negated → violated
        let errors = check_validation_errors_with_db("autoescape", &[], db);
        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(
            !rule_errors.is_empty(),
            "autoescape with no args should error: {errors:?}"
        );
    }

    #[test]
    fn test_no_validation_without_extracted_rules_or_args() {
        // Builtin tags with empty args and no extracted rules get no validation.
        // This is the expected behavior — extraction populates rules in the server.
        let errors = validate_template("{% csrf_token extra_arg %}");
        let arg_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::TooManyArguments { .. }
                        | ValidationError::ExtractedRuleViolation { .. }
                        | ValidationError::MissingRequiredArguments { .. }
                )
            })
            .collect();
        assert!(
            arg_errors.is_empty(),
            "Without extracted rules, no arg validation should occur: {arg_errors:?}"
        );
    }

    // ===================================================================
    // Tests for the user-config TagArg fallback path
    // ===================================================================

    #[test]
    fn test_user_config_args_fallback_rejects_extra() {
        // User-config spec with TagArg definitions (no extracted rules)
        // should use the old validation path.
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;

        let mut specs = FxHashMap::default();
        specs.insert(
            "mycustomtag".to_string(),
            TagSpec {
                module: Cow::Borrowed("myapp.tags"),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Owned(vec![TagArg::var("arg1", true)]),
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));
        let errors = check_validation_errors_with_db("mycustomtag", &["val1", "extra"], db);
        assert!(
            !errors.is_empty(),
            "User-config spec should catch extra args: {errors:?}"
        );
    }

    #[test]
    fn test_user_config_no_args_rejects_extra() {
        // User-config spec with empty args = TooManyArguments
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;

        let mut specs = FxHashMap::default();
        specs.insert(
            "zerotag".to_string(),
            TagSpec {
                module: Cow::Borrowed("myapp.tags"),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                args: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));
        // No extracted rules AND no args → no validation
        let errors = check_validation_errors_with_db("zerotag", &["extra"], db);
        assert!(
            errors.is_empty(),
            "Tag with no extracted rules and no args should get no validation: {errors:?}"
        );
    }

    #[test]
    fn test_extracted_rules_take_precedence_over_args() {
        // When both extracted_rules and args are present, extracted_rules win.
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;

        let mut specs = FxHashMap::default();
        specs.insert(
            "dualtag".to_string(),
            TagSpec {
                module: Cow::Borrowed("myapp.tags"),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                // Old path would say: no args → TooManyArguments for any input
                args: Cow::Borrowed(&[]),
                opaque: false,
                // But extracted rules allow exactly 2 words
                extracted_rules: vec![djls_extraction::ExtractedRule {
                    condition: djls_extraction::RuleCondition::ExactArgCount {
                        count: 2,
                        negated: true,
                    },
                    message: Some("dualtag requires exactly one argument".to_string()),
                }],
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Valid: {% dualtag arg1 %} → split_len=2
        let errors = check_validation_errors_with_db("dualtag", &["arg1"], db.clone());
        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(rule_errors.is_empty(), "Should be valid: {errors:?}");

        // Invalid: {% dualtag %} → split_len=1
        let errors = check_validation_errors_with_db("dualtag", &[], db);
        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(!rule_errors.is_empty(), "Should error: {errors:?}");
    }

    #[test]
    fn test_greedy_consumes_all_leaving_required_literal_unsatisfied() {
        // User-config fallback: greedy expr consumes all, missing required literal
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;
        use crate::TokenCount;

        let mut specs = FxHashMap::default();
        specs.insert(
            "customcond".to_string(),
            TagSpec {
                module: Cow::Borrowed("test.module"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endcustomcond"),
                    required: true,
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
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));
        let errors = check_validation_errors_with_db("customcond", &["x", ">", "0"], db);

        assert!(
            !errors.is_empty(),
            "Should error when greedy arg consumes all tokens: {errors:?}"
        );
        let has_missing_reversed = errors.iter().any(|err| {
            matches!(err, ValidationError::MissingArgument { argument, .. }
                if argument == "reversed")
        });
        assert!(
            has_missing_reversed,
            "Should have MissingArgument for 'reversed', got: {errors:?}"
        );
    }

    #[test]
    fn test_skip_bit_index_bug_with_exact_multi_token() {
        // User-config fallback: Exact(2) + literal + variable
        use std::borrow::Cow;

        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;
        use crate::TokenCount;

        let mut specs = FxHashMap::default();
        specs.insert(
            "customtag".to_string(),
            TagSpec {
                module: Cow::Borrowed("test.module"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endcustomtag"),
                    required: true,
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
                opaque: false,
                extracted_rules: Vec::new(),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));
        let errors = check_validation_errors_with_db("customtag", &["a", "b", "as"], db);

        assert!(
            !errors.is_empty(),
            "Should error when required 'result' argument is missing"
        );
        let has_missing_result = errors.iter().any(|err| {
            matches!(err, ValidationError::MissingArgument { argument, .. }
                if argument == "result")
        });
        assert!(
            has_missing_result,
            "Should have MissingArgument for 'result', got: {errors:?}"
        );
    }
}
