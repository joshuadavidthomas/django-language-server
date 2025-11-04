use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use rustc_hash::FxHashSet;
use salsa::Accumulator;

use crate::semantic::forest::SegmentKind;
use crate::semantic::forest::SemanticNode;
use crate::templatetags::IntermediateTag;
use crate::templatetags::TagArg;
use crate::templatetags::TagSpecs;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

pub fn validate_block_tags(db: &dyn Db, roots: &[SemanticNode]) {
    for node in roots {
        validate_node(db, node);
    }
}

pub fn validate_non_block_tags(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    skip_spans: &[Span],
) {
    let skip: FxHashSet<_> = skip_spans.iter().copied().collect();

    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            if skip.contains(&marker_span) {
                continue;
            }
            validate_tag_arguments(db, name, bits, marker_span);
        }
    }
}

fn validate_node(db: &dyn Db, node: &SemanticNode) {
    match node {
        SemanticNode::Tag {
            name,
            marker_span,
            arguments,
            segments,
        } => {
            validate_tag_arguments(db, name, arguments, *marker_span);

            for segment in segments {
                match &segment.kind {
                    SegmentKind::Main => {
                        validate_children(db, &segment.children);
                    }
                    SegmentKind::Intermediate { tag } => {
                        validate_tag_arguments(db, tag, &segment.arguments, segment.marker_span);
                        validate_children(db, &segment.children);
                    }
                }
            }
        }
        SemanticNode::Leaf { .. } => {}
    }
}

fn validate_children(db: &dyn Db, children: &[SemanticNode]) {
    for child in children {
        validate_node(db, child);
    }
}

/// Validate a single tag invocation against its `TagSpec` definition.
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    if let Some(spec) = tag_specs.get(tag_name) {
        validate_args(db, tag_name, bits, span, spec.args.as_ref());
        return;
    }

    if let Some(end_spec) = tag_specs.get_end_spec_for_closer(tag_name) {
        validate_args(db, tag_name, bits, span, end_spec.args.as_ref());
        return;
    }

    if let Some(intermediate) = find_intermediate_spec(&tag_specs, tag_name) {
        validate_args(db, tag_name, bits, span, intermediate.args.as_ref());
    }
}

fn find_intermediate_spec<'a>(specs: &'a TagSpecs, tag_name: &str) -> Option<&'a IntermediateTag> {
    specs.iter().find_map(|(_, spec)| {
        spec.intermediate_tags
            .iter()
            .find(|it| it.name.as_ref() == tag_name)
    })
}

fn validate_args(db: &dyn Db, tag_name: &str, bits: &[String], span: Span, args: &[TagArg]) {
    if args.is_empty() {
        // If the spec expects no arguments but bits exist, report once.
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

    let has_varargs = args.iter().any(|arg| matches!(arg, TagArg::VarArgs { .. }));
    let required_count = args.iter().filter(|arg| arg.is_required()).count();

    if bits.len() < required_count {
        ValidationErrorAccumulator(ValidationError::MissingRequiredArguments {
            tag: tag_name.to_string(),
            min: required_count,
            span,
        })
        .accumulate(db);
    }

    validate_literals(db, tag_name, bits, span, args);

    if !has_varargs {
        validate_choices_and_order(db, tag_name, bits, span, args);
    }
}

fn validate_literals(db: &dyn Db, tag_name: &str, bits: &[String], span: Span, args: &[TagArg]) {
    for arg in args {
        if let TagArg::Literal { lit, required } = arg {
            if *required && !bits.iter().any(|bit| bit == lit.as_ref()) {
                ValidationErrorAccumulator(ValidationError::InvalidLiteralArgument {
                    tag: tag_name.to_string(),
                    expected: lit.to_string(),
                    span,
                })
                .accumulate(db);
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn validate_choices_and_order(
    db: &dyn Db,
    tag_name: &str,
    bits: &[String],
    span: Span,
    args: &[TagArg],
) {
    let mut bit_index = 0usize;
    let mut args_consumed = 0usize;

    for (arg_index, arg) in args.iter().enumerate() {
        if bit_index >= bits.len() {
            break;
        }

        args_consumed = arg_index + 1;

        match arg {
            TagArg::Literal { lit, required } => {
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
                        break;
                    }
                } else if matches_literal {
                    bit_index += 1;
                }
            }
            TagArg::Choice {
                name,
                required,
                choices,
            } => {
                let value = &bits[bit_index];
                if choices.iter().any(|choice| choice.as_ref() == value) {
                    bit_index += 1;
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
                    break;
                }
            }
            TagArg::Var { .. } | TagArg::String { .. } => {
                // Consume exactly 1 token
                bit_index += 1;
            }
            TagArg::Expr { required, .. } => {
                // Expression arguments consume tokens until:
                // - We hit the next literal keyword
                // - We hit the end of bits
                // - We've consumed at least one token

                let start_index = bit_index;
                let next_literal = find_next_literal(&args[arg_index + 1..]);

                // Consume tokens greedily until we hit a known literal
                while bit_index < bits.len() {
                    if let Some(lit) = next_literal {
                        if bits[bit_index] == lit {
                            break; // Stop before the literal
                        }
                    }
                    bit_index += 1;
                }

                // Optional expressions shouldn't steal the next literal
                if bit_index == start_index
                    && bit_index < bits.len()
                    && (*required || next_literal.is_none_or(|lit| bits[bit_index] != lit))
                {
                    bit_index += 1;
                }
            }
            TagArg::Assignment { .. } => {
                // Assignment can be:
                // 1. Single token with = (e.g., "total=value")
                // 2. Multiple tokens with "as" keyword (e.g., "url 'name' as varname")
                // For now, consume until we find pattern or reach next literal

                let next_literal = find_next_literal(&args[arg_index + 1..]);

                while bit_index < bits.len() {
                    if let Some(lit) = next_literal {
                        if bits[bit_index] == lit {
                            break;
                        }
                    }

                    let token = &bits[bit_index];
                    bit_index += 1;

                    // If token contains =, we're done with this assignment
                    if token.contains('=') {
                        break;
                    }

                    // If we hit "as", consume one more token (the variable name)
                    if token == "as" {
                        if bit_index < bits.len() {
                            bit_index += 1;
                        }
                        break;
                    }
                }
            }
            TagArg::VarArgs { .. } => {
                // Consume all remaining tokens
                bit_index = bits.len();
            }
        }
    }

    // Check for too many arguments: if we have unconsumed tokens after processing all args
    if bit_index < bits.len() {
        // We have unconsumed tokens - this is a too-many-arguments error
        // Note: VarArgs sets bit_index = bits.len(), so we never reach here for VarArgs tags
        ValidationErrorAccumulator(ValidationError::TooManyArguments {
            tag: tag_name.to_string(),
            max: args.len(),
            span,
        })
        .accumulate(db);
        return;
    }

    for arg in args.iter().skip(args_consumed) {
        if arg.is_required() {
            ValidationErrorAccumulator(ValidationError::MissingArgument {
                tag: tag_name.to_string(),
                argument: argument_name(arg),
                span,
            })
            .accumulate(db);
        }
    }
}

fn argument_name(arg: &TagArg) -> String {
    match arg {
        TagArg::Literal { lit, .. } => lit.to_string(),
        TagArg::Choice { name, .. }
        | TagArg::Var { name, .. }
        | TagArg::String { name, .. }
        | TagArg::Expr { name, .. }
        | TagArg::Assignment { name, .. }
        | TagArg::VarArgs { name, .. } => name.to_string(),
    }
}

/// Find the next literal keyword in the argument list.
/// This helps expression arguments know when to stop consuming tokens.
fn find_next_literal(remaining_args: &[TagArg]) -> Option<&str> {
    for arg in remaining_args {
        if let TagArg::Literal { lit, .. } = arg {
            return Some(lit.as_ref());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::templatetags::django_builtin_specs;
    use crate::TagIndex;
    use crate::TagSpec;
    use crate::TagSpecs;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        tag_specs: Arc<TagSpecs>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self::with_specs(django_builtin_specs())
        }

        fn with_specs(specs: TagSpecs) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                tag_specs: Arc::new(specs),
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
            (*self.tag_specs).clone()
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
        // Spec expects: [Expr{name="condition"}]
        let bits = vec![
            "message.input_tokens".to_string(),
            ">".to_string(),
            "0".to_string(),
        ];
        let args = vec![TagArg::Expr {
            name: "condition".into(),
            required: true,
        }];

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
        let args = vec![TagArg::Expr {
            name: "condition".into(),
            required: true,
        }];

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
        let args = vec![TagArg::Assignment {
            name: "bindings".into(),
            required: true,
        }];

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
            TagArg::Expr {
                name: "condition".into(),
                required: true,
            },
            TagArg::Literal {
                lit: "reversed".into(),
                required: false,
            },
        ];

        let errors = check_validation_errors("if", &bits, &args);
        assert!(
            errors.is_empty(),
            "Expr should stop before literal keyword: {errors:?}"
        );
    }

    #[test]
    fn test_optional_expr_skips_literal() {
        let mut specs_map: FxHashMap<String, TagSpec> =
            django_builtin_specs().into_iter().collect();
        specs_map.insert(
            "optional_expr".to_string(),
            TagSpec {
                module: "tests.optional".into(),
                end_tag: None,
                intermediate_tags: Vec::new().into(),
                args: vec![
                    TagArg::Expr {
                        name: "optional".into(),
                        required: false,
                    },
                    TagArg::Literal {
                        lit: "reversed".into(),
                        required: true,
                    },
                ]
                .into(),
            },
        );
        let db = TestDatabase::with_specs(TagSpecs::new(specs_map));

        let bits = ["reversed".to_string()];
        let bits_str = bits.join(" ");
        let content = format!("{{% optional_expr {bits_str} %}}");

        let path = Utf8Path::new("/optional.html");
        db.fs.lock().unwrap().add_file(path.to_owned(), content);

        let file = db.create_file(path);
        let nodelist = djls_templates::parse_template(&db, file).expect("Failed to parse template");

        crate::validate_nodelist(&db, nodelist);

        let errors: Vec<_> =
            crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
                .into_iter()
                .map(|acc| acc.0.clone())
                .collect();
        assert!(
            errors.is_empty(),
            "Optional expr should not consume following literal: {errors:?}"
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
            TagArg::VarArgs {
                name: "tags".into(),
                required: false,
            },
            TagArg::Literal {
                lit: "from".into(),
                required: false,
            },
            TagArg::Var {
                name: "library".into(),
                required: false,
            },
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
