use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use salsa::Accumulator;

use crate::rule_evaluation::evaluate_tag_rules;
use crate::Db;
use crate::ValidationErrorAccumulator;

/// Validate arguments for all tags in the template.
///
/// Performs a single pass over the flat `NodeList`, validating each tag's arguments
/// against extraction-derived rules. Tags without extracted rules are skipped.
pub fn validate_all_tag_arguments(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    opaque_regions: &crate::OpaqueRegions,
) {
    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            if opaque_regions.is_opaque(span.start()) {
                continue;
            }
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            validate_tag_arguments(db, name, bits, marker_span);
        }
    }
}

/// Validate a single tag's arguments against extraction-derived rules.
///
/// Looks up the tag in `TagSpecs` and dispatches to the rule evaluator
/// when `extracted_rules` is present. Tags without rules are skipped.
fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: djls_source::Span) {
    let tag_specs = db.tag_specs();

    // Only opener specs can have extracted rules
    if let Some(spec) = tag_specs.get(tag_name) {
        if let Some(rules) = &spec.extracted_rules {
            for error in evaluate_tag_rules(tag_name, bits, rules, span) {
                ValidationErrorAccumulator(error).accumulate(db);
            }
        }
    }

    // Closers and intermediates have no extracted rules — nothing to validate
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
    use crate::ValidationError;

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

        fn inspector_inventory(&self) -> Option<djls_project::TemplateTags> {
            None
        }

        fn filter_arity_specs(&self) -> crate::filter_arity::FilterAritySpecs {
            crate::filter_arity::FilterAritySpecs::new()
        }
    }

    /// Test helper: Create a temporary `NodeList` with a single tag and validate it using custom specs
    #[allow(clippy::needless_pass_by_value)]
    fn check_validation_errors_with_db(
        tag_name: &str,
        bits: &[String],
        db: TestDatabase,
    ) -> Vec<ValidationError> {
        use djls_source::Db as SourceDb;

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

        let path = camino::Utf8Path::new("/test.html");
        db.fs.lock().unwrap().add_file(path.to_owned(), content);

        let file = db.create_file(path);
        let nodelist = djls_templates::parse_template(&db, file).expect("Failed to parse template");

        crate::validate_nodelist(&db, nodelist);

        crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(|err| {
                !matches!(
                    err,
                    ValidationError::UnclosedTag { .. }
                        | ValidationError::ExpressionSyntaxError { .. }
                        | ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                )
            })
            .collect()
    }

    #[test]
    fn test_builtins_without_extraction_skip_arg_validation() {
        // Builtins with no extracted_rules should not produce argument validation errors
        let bits = vec!["extra_arg".to_string()];
        let errors = check_validation_errors_with_db("csrf_token", &bits, TestDatabase::new());
        assert!(
            errors.is_empty(),
            "Builtins without extracted_rules should skip arg validation: {errors:?}"
        );
    }

    #[test]
    fn test_extracted_rules_exact_constraint_rejects() {
        use std::borrow::Cow;

        use djls_extraction::ArgumentCountConstraint;
        use djls_extraction::TagRule;
        use rustc_hash::FxHashMap;

        use crate::templatetags::TagSpec;
        use crate::templatetags::TagSpecs;

        let mut specs = FxHashMap::default();
        specs.insert(
            "csrf_token".to_string(),
            TagSpec {
                module: Cow::Borrowed("django.template.defaulttags"),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: Some(TagRule {
                    arg_constraints: vec![ArgumentCountConstraint::Exact(1)],
                    required_keywords: vec![],
                    known_options: None,
                    extracted_args: vec![],
                }),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Valid: no args → split_len=1 → Exact(1) passes
        let bits: Vec<String> = vec![];
        let errors = check_validation_errors_with_db("csrf_token", &bits, db.clone());
        assert!(
            errors.is_empty(),
            "No args should pass Exact(1): {errors:?}"
        );

        // Invalid: 1 arg → split_len=2 → Exact(1) fails
        let db2 = TestDatabase::with_custom_specs(db.tag_specs());
        let bits = vec!["extra".to_string()];
        let errors = check_validation_errors_with_db("csrf_token", &bits, db2);
        assert!(
            !errors.is_empty(),
            "Extra arg should fail Exact(1) constraint"
        );
        assert!(
            matches!(&errors[0], ValidationError::ExtractedRuleViolation { .. }),
            "Expected ExtractedRuleViolation, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn test_extracted_rules_min_constraint_rejects() {
        use std::borrow::Cow;

        use djls_extraction::ArgumentCountConstraint;
        use djls_extraction::RequiredKeyword;
        use djls_extraction::TagRule;
        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::IntermediateTag;
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
                intermediate_tags: Cow::Owned(vec![IntermediateTag {
                    name: Cow::Borrowed("empty"),
                }]),
                opaque: false,
                extracted_rules: Some(TagRule {
                    arg_constraints: vec![ArgumentCountConstraint::Min(4)],
                    required_keywords: vec![RequiredKeyword {
                        position: 2,
                        value: "in".to_string(),
                    }],
                    known_options: None,
                    extracted_args: vec![],
                }),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Invalid: {% for item %} → split_len=2 < 4
        let bits = vec!["item".to_string()];
        let errors = check_validation_errors_with_db("for", &bits, db.clone());
        assert!(
            !errors.is_empty(),
            "Too few args should fail Min(4): {errors:?}"
        );

        // Valid: {% for item in items %} → split_len=4 >= 4
        let db2 = TestDatabase::with_custom_specs(db.tag_specs());
        let bits = vec!["item".to_string(), "in".to_string(), "items".to_string()];
        let errors = check_validation_errors_with_db("for", &bits, db2);
        assert!(
            errors.is_empty(),
            "3 args should pass Min(4) (split_len=4): {errors:?}"
        );
    }

    #[test]
    fn test_extracted_rules_required_keyword_rejects() {
        use std::borrow::Cow;

        use djls_extraction::ArgumentCountConstraint;
        use djls_extraction::RequiredKeyword;
        use djls_extraction::TagRule;
        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::IntermediateTag;
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
                intermediate_tags: Cow::Owned(vec![IntermediateTag {
                    name: Cow::Borrowed("empty"),
                }]),
                opaque: false,
                extracted_rules: Some(TagRule {
                    arg_constraints: vec![ArgumentCountConstraint::Min(4)],
                    required_keywords: vec![RequiredKeyword {
                        position: 2,
                        value: "in".to_string(),
                    }],
                    known_options: None,
                    extracted_args: vec![],
                }),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Invalid: {% for item at items %} → "in" expected at position 2
        let bits = vec!["item".to_string(), "at".to_string(), "items".to_string()];
        let errors = check_validation_errors_with_db("for", &bits, db);
        assert!(
            !errors.is_empty(),
            "Wrong keyword should fail RequiredKeyword: {errors:?}"
        );
    }

    #[test]
    fn test_for_extra_arg_with_extracted_rules() {
        use std::borrow::Cow;

        use djls_extraction::ArgumentCountConstraint;
        use djls_extraction::RequiredKeyword;
        use djls_extraction::TagRule;
        use rustc_hash::FxHashMap;

        use crate::templatetags::EndTag;
        use crate::templatetags::IntermediateTag;
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
                intermediate_tags: Cow::Owned(vec![IntermediateTag {
                    name: Cow::Borrowed("empty"),
                }]),
                opaque: false,
                extracted_rules: Some(TagRule {
                    arg_constraints: vec![ArgumentCountConstraint::OneOf(vec![4, 5])],
                    required_keywords: vec![RequiredKeyword {
                        position: 2,
                        value: "in".to_string(),
                    }],
                    known_options: None,
                    extracted_args: vec![],
                }),
            },
        );

        let db = TestDatabase::with_custom_specs(TagSpecs::new(specs));

        // Valid: {% for item in items %} → split_len=4
        let bits = vec!["item".to_string(), "in".to_string(), "items".to_string()];
        let errors = check_validation_errors_with_db("for", &bits, db.clone());
        assert!(errors.is_empty(), "Valid for tag should pass: {errors:?}");

        // Valid: {% for item in items reversed %} → split_len=5
        let db2 = TestDatabase::with_custom_specs(db.tag_specs());
        let bits = vec![
            "item".to_string(),
            "in".to_string(),
            "items".to_string(),
            "reversed".to_string(),
        ];
        let errors = check_validation_errors_with_db("for", &bits, db2);
        assert!(
            errors.is_empty(),
            "Valid for tag with reversed should pass: {errors:?}"
        );

        // Invalid: 6 args → split_len=6, not in {4,5}
        let db3 = TestDatabase::with_custom_specs(db.tag_specs());
        let bits = vec![
            "item".to_string(),
            "in".to_string(),
            "items".to_string(),
            "football".to_string(),
            "extra".to_string(),
        ];
        let errors = check_validation_errors_with_db("for", &bits, db3);
        assert!(
            !errors.is_empty(),
            "Too many args should fail OneOf constraint: {errors:?}"
        );
    }
}
