use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;

use crate::Db;

/// Validate arguments for all tags in the template.
///
/// Performs a single pass over the flat `NodeList`, validating each tag's arguments
/// against its extracted rules. Nodes inside opaque regions (e.g., `{% verbatim %}`)
/// are skipped.
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
/// Dispatches to extracted rule evaluation when available.
/// Closer and intermediate tags receive no argument validation.
fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
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

    use crate::templatetags::django_builtin_specs;
    use crate::Db;
    use crate::TagIndex;
    use crate::ValidationError;
    use crate::ValidationErrorAccumulator;

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
    impl Db for TestDatabase {
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
    // Tests for the extracted rule evaluation path
    // ===================================================================

    #[test]
    fn test_endblock_with_name_is_valid() {
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
                opaque: false,
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

        // Valid: {% autoescape on %} → split_len=2
        let errors = check_validation_errors_with_db("autoescape", &["on"], db.clone());
        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(
            rule_errors.is_empty(),
            "autoescape on should be valid: {errors:?}"
        );

        // Invalid: {% autoescape on extra %}
        let errors = check_validation_errors_with_db("autoescape", &["on", "extra"], db.clone());
        let rule_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(
            !rule_errors.is_empty(),
            "autoescape with extra arg should error: {errors:?}"
        );

        // Invalid: {% autoescape %}
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
    fn test_no_validation_without_extracted_rules() {
        // Builtin tags with no extracted rules get no validation in test context.
        let errors = validate_template("{% csrf_token extra_arg %}");
        let arg_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtractedRuleViolation { .. }))
            .collect();
        assert!(
            arg_errors.is_empty(),
            "Without extracted rules, no arg validation should occur: {arg_errors:?}"
        );
    }

    #[test]
    fn test_extracted_rules_take_precedence() {
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
                opaque: false,
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
}
