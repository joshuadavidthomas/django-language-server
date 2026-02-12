pub mod db;
pub mod errors;
pub mod primitives;
pub mod resolution;
pub mod scoping;
pub mod specs;
pub mod structure;
pub mod tag_rules;
pub mod traits;
pub mod validation;

#[cfg(test)]
pub mod testing;

pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
pub use primitives::Tag;
pub use primitives::Template;
pub use primitives::TemplateName;
pub use resolution::find_references_to_template;
pub use resolution::resolve_template;
pub use resolution::ResolveResult;
pub use resolution::TemplateReference;
pub use scoping::compute_loaded_libraries;
pub use scoping::parse_load_bits;
pub use scoping::AvailableSymbols;
pub use scoping::FilterAvailability;
pub use scoping::LoadKind;
pub use scoping::LoadStatement;
pub use scoping::LoadedLibraries;
pub use scoping::TagAvailability;
pub use specs::filters::FilterAritySpecs;
pub use specs::tags::builtin_tag_specs;
pub use specs::tags::CompletionArg;
pub use specs::tags::CompletionArgKind;
pub use specs::tags::EndTag;
pub use specs::tags::TagSpec;
pub use specs::tags::TagSpecs;
pub use structure::build_block_tree;
pub use structure::build_semantic_forest;
pub use structure::compute_opaque_regions;
pub use structure::OpaqueRegions;
pub use structure::TagIndex;
pub use validation::TemplateValidator;

/// Validate a Django template node list and return validation errors.
///
/// This function builds a `BlockTree` from the parsed node list and, during
/// construction, accumulates semantic validation errors for issues such as:
/// - Unclosed block tags
/// - Mismatched tag pairs
/// - Orphaned intermediate tags
/// - Invalid argument counts
/// - Unmatched block names
#[salsa::tracked]
pub fn validate_nodelist(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    let nodes = nodelist.nodelist(db);
    if nodes.is_empty() {
        return;
    }

    // 1. Structural Analysis (Builds BlockTree and SemanticForest)
    let block_tree = build_block_tree(db, nodelist);
    let _forest = build_semantic_forest(db, block_tree, nodelist);

    // 2. Perform all other validations in a single walk
    let opaque_regions = compute_opaque_regions(db, nodelist);
    let validator = TemplateValidator::new(db, nodelist, &opaque_regions);
    validator.validate(nodes);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::fmt::Write;

    use camino::Utf8PathBuf;
    use djls_project::TemplateLibraries;
    use djls_python::FilterArity;
    use djls_python::SymbolKey;

    use crate::specs::filters::FilterAritySpecs;
    use crate::testing::builtin_filter_json;
    use crate::testing::builtin_tag_json;
    use crate::testing::collect_errors;
    use crate::testing::library_tag_json;
    use crate::testing::make_template_libraries;
    use crate::testing::TestDatabase;
    use crate::ValidationError;

    fn default_builtins_module() -> &'static str {
        "django.template.defaulttags"
    }

    fn default_filters_module() -> &'static str {
        "django.template.defaultfilters"
    }

    fn standard_inventory() -> TemplateLibraries {
        let tags = vec![
            builtin_tag_json("if", default_builtins_module()),
            builtin_tag_json("for", default_builtins_module()),
            builtin_tag_json("block", default_builtins_module()),
            builtin_tag_json("verbatim", default_builtins_module()),
            builtin_tag_json("comment", default_builtins_module()),
            builtin_tag_json("load", default_builtins_module()),
            builtin_tag_json("csrf_token", default_builtins_module()),
            builtin_tag_json("with", default_builtins_module()),
            library_tag_json("trans", "i18n", "django.templatetags.i18n"),
        ];
        let filters = vec![
            builtin_filter_json("title", default_filters_module()),
            builtin_filter_json("lower", default_filters_module()),
            builtin_filter_json("default", default_filters_module()),
            builtin_filter_json("truncatewords", default_filters_module()),
            builtin_filter_json("date", default_filters_module()),
        ];
        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        let builtins = vec![
            default_builtins_module().to_string(),
            default_filters_module().to_string(),
        ];
        make_template_libraries(&tags, &filters, &libraries, &builtins)
    }

    fn standard_arities() -> FilterAritySpecs {
        let mut specs = FilterAritySpecs::new();
        specs.insert(
            SymbolKey::filter(default_filters_module(), "title"),
            FilterArity {
                expects_arg: false,
                arg_optional: false,
            },
        );
        specs.insert(
            SymbolKey::filter(default_filters_module(), "lower"),
            FilterArity {
                expects_arg: false,
                arg_optional: false,
            },
        );
        specs.insert(
            SymbolKey::filter(default_filters_module(), "default"),
            FilterArity {
                expects_arg: true,
                arg_optional: false,
            },
        );
        specs.insert(
            SymbolKey::filter(default_filters_module(), "truncatewords"),
            FilterArity {
                expects_arg: true,
                arg_optional: false,
            },
        );
        specs.insert(
            SymbolKey::filter(default_filters_module(), "date"),
            FilterArity {
                expects_arg: true,
                arg_optional: true,
            },
        );
        specs
    }

    fn standard_db() -> TestDatabase {
        TestDatabase::new()
            .with_template_libraries(standard_inventory())
            .with_arity_specs(standard_arities())
    }

    fn collect_all_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
        collect_errors(db, "test.html", source)
    }

    // Integration: Mixed diagnostics

    #[test]
    fn mixed_expression_and_filter_arity_errors() {
        let db = standard_db();
        let source = concat!(
            "{% if and x %}bad expr{% endif %}\n",
            "{{ value|truncatewords }}\n",
            "{{ value|title:\"bad\" }}\n",
        );
        let errors = collect_all_errors(&db, source);

        let expr_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
            .collect();
        let s115_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::FilterMissingArgument { .. }))
            .collect();
        let s116_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::FilterUnexpectedArgument { .. }))
            .collect();

        assert_eq!(
            expr_errors.len(),
            1,
            "Expected 1 expression error, got: {expr_errors:?}"
        );
        assert_eq!(
            s115_errors.len(),
            1,
            "Expected 1 FilterMissingArgument, got: {s115_errors:?}"
        );
        assert_eq!(
            s116_errors.len(),
            1,
            "Expected 1 FilterUnexpectedArgument, got: {s116_errors:?}"
        );
    }

    #[test]
    fn opaque_region_suppresses_all_validation() {
        let db = standard_db();
        // Everything inside verbatim should be skipped
        let source = concat!(
            "{% verbatim %}\n",
            "{% if and x %}bad expr{% endif %}\n",
            "{{ value|truncatewords }}\n",
            "{{ value|title:\"bad\" }}\n",
            "{% endverbatim %}\n",
        );
        let errors = collect_all_errors(&db, source);

        // Filter out structural errors (UnclosedTag etc) that come from the block tree
        let validation_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExpressionSyntaxError { .. }
                        | ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                        | ValidationError::UnknownTag { .. }
                        | ValidationError::UnloadedTag { .. }
                        | ValidationError::UnknownFilter { .. }
                        | ValidationError::UnloadedFilter { .. }
                )
            })
            .collect();

        assert!(
            validation_errors.is_empty(),
            "No expression/filter/scoping errors expected inside verbatim, got: {validation_errors:?}"
        );
    }

    #[test]
    fn errors_before_and_after_opaque_region() {
        let db = standard_db();
        let source = concat!(
            "{{ value|truncatewords }}\n",
            "{% verbatim %}{% if and x %}{% endverbatim %}\n",
            "{{ value|title:\"bad\" }}\n",
        );
        let errors = collect_all_errors(&db, source);

        let s115_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::FilterMissingArgument { .. }))
            .collect();
        let s116_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::FilterUnexpectedArgument { .. }))
            .collect();
        let expr_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
            .collect();

        assert_eq!(
            s115_errors.len(),
            1,
            "Expected S115 before verbatim, got: {s115_errors:?}"
        );
        assert_eq!(
            s116_errors.len(),
            1,
            "Expected S116 after verbatim, got: {s116_errors:?}"
        );
        assert!(
            expr_errors.is_empty(),
            "No expression errors expected (bad if is inside verbatim), got: {expr_errors:?}"
        );
    }

    #[test]
    fn comment_block_also_opaque() {
        let db = standard_db();
        let source = concat!(
            "{% comment %}\n",
            "{% if and x %}{% endif %}\n",
            "{{ value|truncatewords }}\n",
            "{% endcomment %}\n",
        );
        let errors = collect_all_errors(&db, source);

        let validation_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExpressionSyntaxError { .. }
                        | ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                )
            })
            .collect();

        assert!(
            validation_errors.is_empty(),
            "No errors expected inside comment block, got: {validation_errors:?}"
        );
    }

    #[test]
    fn unloaded_tag_and_filter_with_expression_error() {
        let db = standard_db();
        // trans requires {% load i18n %}, but it's not loaded
        // Also has an expression error in an if tag
        let source = concat!("{% if or x %}bad{% endif %}\n", "{% trans \"hello\" %}\n",);
        let errors = collect_all_errors(&db, source);

        let expr_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
            .collect();
        let scoping_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::UnloadedTag { .. } | ValidationError::UnknownTag { .. }
                )
            })
            .collect();

        assert_eq!(
            expr_errors.len(),
            1,
            "Expected 1 expression error, got: {expr_errors:?}"
        );
        assert_eq!(
            scoping_errors.len(),
            1,
            "Expected 1 scoping error for trans, got: {scoping_errors:?}"
        );
        // Verify it's specifically an UnloadedTag for trans
        assert!(
            matches!(&scoping_errors[0], ValidationError::UnloadedTag { tag, library, .. }
                if tag == "trans" && library == "i18n"),
            "Expected UnloadedTag for trans/i18n, got: {:?}",
            scoping_errors[0]
        );
    }

    #[test]
    fn loaded_library_tags_valid_with_filter_errors() {
        let db = standard_db();
        let source = concat!(
            "{% load i18n %}\n",
            "{% trans \"hello\" %}\n",
            "{{ value|truncatewords }}\n",
        );
        let errors = collect_all_errors(&db, source);

        let scoping_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::UnloadedTag { .. } | ValidationError::UnknownTag { .. }
                )
            })
            .collect();
        let s115_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::FilterMissingArgument { .. }))
            .collect();

        assert!(
            scoping_errors.is_empty(),
            "No scoping errors after load, got: {scoping_errors:?}"
        );
        assert_eq!(
            s115_errors.len(),
            1,
            "Expected S115 for truncatewords, got: {s115_errors:?}"
        );
    }

    // Snapshot tests for diagnostic output

    #[test]
    fn snapshot_mixed_diagnostics() {
        let db = standard_db();
        let source = concat!(
            "{% if and x %}oops{% endif %}\n",
            "{{ name|title:\"arg\" }}\n",
            "{{ text|truncatewords }}\n",
            "{% trans \"hello\" %}\n",
        );

        let rendered = crate::testing::render_validate_snapshot(&db, "test.html", 0, source);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_clean_template_no_errors() {
        let db = standard_db();
        let source = concat!(
            "{% if user.is_authenticated %}\n",
            "  <h1>{{ user.name|title }}</h1>\n",
            "  {{ user.joined|date:\"Y-m-d\" }}\n",
            "{% endif %}\n",
        );
        let errors = collect_all_errors(&db, source);

        let validation_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExpressionSyntaxError { .. }
                        | ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                        | ValidationError::UnknownTag { .. }
                        | ValidationError::UnloadedTag { .. }
                        | ValidationError::UnknownFilter { .. }
                        | ValidationError::UnloadedFilter { .. }
                )
            })
            .collect();

        assert!(
            validation_errors.is_empty(),
            "Clean template should produce no validation errors, got: {validation_errors:?}"
        );
    }

    #[test]
    fn snapshot_complex_valid_template() {
        let db = standard_db();
        // A realistic Django admin-style template with various features
        let source = concat!(
            "{% load i18n %}\n",
            "{% if user.is_staff and not user.is_superuser %}\n",
            "  <p>{{ greeting|default:\"Hello\" }}</p>\n",
            "  {% for item in items %}\n",
            "    <li>{{ item.name|title }} - {{ item.date|date }}</li>\n",
            "  {% endfor %}\n",
            "  {% trans \"Welcome\" %}\n",
            "{% endif %}\n",
            "{% verbatim %}\n",
            "  {{ raw_template_syntax }}\n",
            "{% endverbatim %}\n",
        );
        let errors = collect_all_errors(&db, source);

        let validation_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExpressionSyntaxError { .. }
                        | ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                        | ValidationError::UnknownTag { .. }
                        | ValidationError::UnloadedTag { .. }
                        | ValidationError::UnknownFilter { .. }
                        | ValidationError::UnloadedFilter { .. }
                )
            })
            .collect();

        assert!(
            validation_errors.is_empty(),
            "Valid complex template should have no errors, got: {validation_errors:?}"
        );
    }

    #[test]
    fn snapshot_multiple_error_types() {
        let db = standard_db();
        let source = concat!(
            "{{ value|title:\"unwanted\" }}\n",
            "{% if == broken %}bad{% endif %}\n",
            "{{ text|lower:\"arg\" }}\n",
            "{% comment %}{% if and %}{% endcomment %}\n",
            "{{ result|truncatewords }}\n",
        );

        let rendered = crate::testing::render_validate_snapshot(&db, "test.html", 0, source);
        insta::assert_snapshot!(rendered);
    }

    // Extends validation (S122, S123)

    #[test]
    fn extends_as_first_tag_no_errors() {
        let db = standard_db();
        let source = r#"{% extends "base.html" %}"#;
        let errors = collect_all_errors(&db, source);
        let extends_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExtendsMustBeFirst { .. }
                        | ValidationError::MultipleExtends { .. }
                )
            })
            .collect();
        assert!(
            extends_errors.is_empty(),
            "No extends errors expected, got: {extends_errors:?}"
        );
    }

    #[test]
    fn text_whitespace_before_extends_no_errors() {
        let db = standard_db();
        let source = "  \n\n  {% extends \"base.html\" %}";
        let errors = collect_all_errors(&db, source);
        let extends_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExtendsMustBeFirst { .. }
                        | ValidationError::MultipleExtends { .. }
                )
            })
            .collect();
        assert!(
            extends_errors.is_empty(),
            "Text/whitespace before extends should be fine, got: {extends_errors:?}"
        );
    }

    #[test]
    fn comment_before_extends_no_errors() {
        let db = standard_db();
        let source = "{# this is a comment #}{% extends \"base.html\" %}";
        let errors = collect_all_errors(&db, source);
        let extends_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExtendsMustBeFirst { .. }
                        | ValidationError::MultipleExtends { .. }
                )
            })
            .collect();
        assert!(
            extends_errors.is_empty(),
            "Comment before extends should be fine, got: {extends_errors:?}"
        );
    }

    #[test]
    fn no_extends_at_all_no_errors() {
        let db = standard_db();
        let source = "{% if user %}hello{% endif %}";
        let errors = collect_all_errors(&db, source);
        let extends_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExtendsMustBeFirst { .. }
                        | ValidationError::MultipleExtends { .. }
                )
            })
            .collect();
        assert!(
            extends_errors.is_empty(),
            "No extends = no extends errors, got: {extends_errors:?}"
        );
    }

    #[test]
    fn tag_before_extends_s122() {
        let db = standard_db();
        let source = "{% load i18n %}{% extends \"base.html\" %}";
        let errors = collect_all_errors(&db, source);
        let s122: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
            .collect();
        assert_eq!(s122.len(), 1, "Expected S122, got: {s122:?}");
    }

    #[test]
    fn variable_before_extends_s122() {
        let db = standard_db();
        let source = "{{ variable }}{% extends \"base.html\" %}";
        let errors = collect_all_errors(&db, source);
        let s122: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
            .collect();
        assert_eq!(s122.len(), 1, "Expected S122, got: {s122:?}");
    }

    #[test]
    fn multiple_extends_s123() {
        let db = standard_db();
        let source = r#"{% extends "base.html" %}{% extends "other.html" %}"#;
        let errors = collect_all_errors(&db, source);
        let s123: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::MultipleExtends { .. }))
            .collect();
        assert_eq!(s123.len(), 1, "Expected S123, got: {s123:?}");
        // First extends should NOT produce S122
        let s122: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
            .collect();
        assert!(s122.is_empty(), "First extends is valid, got: {s122:?}");
    }

    #[test]
    fn tag_before_extends_and_multiple_extends_s122_and_s123() {
        let db = standard_db();
        let source = r#"{% load i18n %}{% extends "a.html" %}{% extends "b.html" %}"#;
        let errors = collect_all_errors(&db, source);
        let s122: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
            .collect();
        let s123: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::MultipleExtends { .. }))
            .collect();
        assert_eq!(s122.len(), 1, "Expected S122, got: {s122:?}");
        assert_eq!(s123.len(), 1, "Expected S123, got: {s123:?}");
    }

    // Corpus / template validation tests
    //
    // These tests extract rules from real Django source files and validate
    // real templates against those rules, proving zero false positives for
    // argument validation (S114, S115, S116, S117) at scale.
    //
    // All tests skip gracefully when the corpus is unavailable.
    // Run `cargo run -p djls-corpus -- sync` to populate it.

    use djls_corpus::Corpus;

    use crate::testing::build_entry_specs;
    use crate::testing::build_specs_from_extraction;
    use crate::testing::collect_argument_validation_errors_with_revision;

    struct FailureEntry {
        path: Utf8PathBuf,
        errors: Vec<String>,
    }

    fn format_failures(failures: &[FailureEntry]) -> String {
        let mut out = String::new();
        for f in failures.iter().take(20) {
            let _ = writeln!(out, "  {}:", f.path);
            for err in &f.errors {
                let _ = writeln!(out, "    - {err}");
            }
        }
        if failures.len() > 20 {
            let _ = writeln!(out, "  ... and {} more", failures.len() - 20);
        }
        out
    }

    #[test]
    fn corpus_templates_have_no_argument_false_positives() {
        let corpus = Corpus::require();

        let templates = corpus.templates_in(corpus.root());
        let mut by_entry: BTreeMap<Utf8PathBuf, Vec<Utf8PathBuf>> = BTreeMap::new();

        for template_path in templates {
            let Some(entry_dir) = corpus.entry_dir_for_path(&template_path) else {
                continue;
            };

            by_entry.entry(entry_dir).or_default().push(template_path);
        }

        for templates in by_entry.values_mut() {
            templates.sort();
        }

        let mut failures = Vec::new();

        for (entry_dir, templates) in by_entry {
            if templates.is_empty() {
                continue;
            }

            let (specs, arities) = build_entry_specs(&corpus, &entry_dir);
            let db = TestDatabase::new()
                .with_specs(specs)
                .with_arity_specs(arities);

            for (i, template_path) in templates.into_iter().enumerate() {
                let Ok(content) = std::fs::read_to_string(template_path.as_std_path()) else {
                    continue;
                };

                let errors = collect_argument_validation_errors_with_revision(
                    &db,
                    "corpus_test.html",
                    i as u64,
                    &content,
                );
                if errors.is_empty() {
                    continue;
                }

                failures.push(FailureEntry {
                    path: template_path,
                    errors: errors
                        .into_iter()
                        .take(5)
                        .map(|e| format!("{e:?}"))
                        .collect(),
                });
            }
        }

        assert!(
            failures.is_empty(),
            "Corpus templates have false positives:\n{}",
            format_failures(&failures)
        );
    }

    #[test]
    fn corpus_known_invalid_templates_produce_errors() {
        let corpus = Corpus::require();

        let Some(django_dir) = corpus.latest_package("django") else {
            eprintln!("No Django in corpus.");
            return;
        };

        let (specs, arities) = build_specs_from_extraction(&corpus, &django_dir);

        let db = TestDatabase::new()
            .with_specs(specs)
            .with_arity_specs(arities);

        // for tag with wrong number of args
        let errors = collect_argument_validation_errors_with_revision(
            &db,
            "corpus_test.html",
            0,
            "{% for %}content{% endfor %}",
        );
        assert!(
            !errors.is_empty(),
            "Expected errors for {{% for %}} with no args"
        );

        // if expression syntax error
        let errors = collect_argument_validation_errors_with_revision(
            &db,
            "corpus_test.html",
            1,
            "{% if and x %}content{% endif %}",
        );
        let expr_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
            .collect();
        assert!(
            !expr_errors.is_empty(),
            "Expected expression syntax error for {{% if and x %}}"
        );
    }
}
