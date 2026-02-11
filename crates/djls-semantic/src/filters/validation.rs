use djls_templates::Node;
use djls_templates::NodeList;
use salsa::Accumulator;

use crate::Db;
use crate::OpaqueRegions;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Validate filter argument arity for all variable nodes in a template.
///
/// For each filter in `{{ var|filter }}` or `{{ var|filter:arg }}`, checks
/// whether the filter's usage matches its extracted arity specification:
///
/// - **S115** (`FilterMissingArgument`): filter requires an argument but none provided
/// - **S116** (`FilterUnexpectedArgument`): filter does not accept an argument but one was provided
///
/// **Guards:**
/// - If the inspector inventory is `None`, all arity diagnostics are suppressed.
/// - Nodes inside opaque regions are skipped.
/// - Filters with no known arity spec are silently skipped (no false positives).
pub fn validate_filter_arity(db: &dyn Db, nodelist: NodeList<'_>, opaque_regions: &OpaqueRegions) {
    let template_libraries = db.template_libraries();
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

    let arity_specs = db.filter_arity_specs();
    if arity_specs.is_empty() {
        return;
    }

    for node in nodelist.nodelist(db) {
        let Node::Variable { filters, span, .. } = node else {
            continue;
        };

        if opaque_regions.is_opaque(span.start()) {
            continue;
        }

        for filter in filters {
            let Some(arity) = arity_specs.get(&filter.name) else {
                continue;
            };

            let has_arg = filter.arg.is_some();

            if arity.expects_arg && !arity.arg_optional && !has_arg {
                // S115: required argument missing
                ValidationErrorAccumulator(ValidationError::FilterMissingArgument {
                    filter: filter.name.clone(),
                    span: filter.span,
                })
                .accumulate(db);
            } else if !arity.expects_arg && has_arg {
                // S116: unexpected argument provided
                ValidationErrorAccumulator(ValidationError::FilterUnexpectedArgument {
                    filter: filter.name.clone(),
                    span: filter.span,
                })
                .accumulate(db);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use djls_python::FilterArity;
    use djls_python::SymbolKey;

    use crate::filters::arity::FilterAritySpecs;
    use crate::testing::builtin_filter_json;
    use crate::testing::builtin_tag_json;
    use crate::testing::make_template_libraries;
    use crate::testing::TestDatabase;
    use crate::ValidationError;

    fn make_template_libraries_with_filters(
        filters: &[serde_json::Value],
    ) -> djls_project::TemplateLibraries {
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            builtin_tag_json("verbatim", "django.template.defaulttags"),
            builtin_tag_json("comment", "django.template.defaulttags"),
        ];

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];

        make_template_libraries(&tags, filters, &HashMap::new(), &builtins)
    }

    fn default_arities() -> FilterAritySpecs {
        let mut specs = FilterAritySpecs::new();
        // title: no arg
        specs.insert(
            SymbolKey::filter("django.template.defaultfilters", "title"),
            FilterArity {
                expects_arg: false,
                arg_optional: false,
            },
        );
        // lower: no arg
        specs.insert(
            SymbolKey::filter("django.template.defaultfilters", "lower"),
            FilterArity {
                expects_arg: false,
                arg_optional: false,
            },
        );
        // default: required arg
        specs.insert(
            SymbolKey::filter("django.template.defaultfilters", "default"),
            FilterArity {
                expects_arg: true,
                arg_optional: false,
            },
        );
        // truncatewords: required arg
        specs.insert(
            SymbolKey::filter("django.template.defaultfilters", "truncatewords"),
            FilterArity {
                expects_arg: true,
                arg_optional: false,
            },
        );
        // truncatechars: required arg
        specs.insert(
            SymbolKey::filter("django.template.defaultfilters", "truncatechars"),
            FilterArity {
                expects_arg: true,
                arg_optional: false,
            },
        );
        // date: optional arg
        specs.insert(
            SymbolKey::filter("django.template.defaultfilters", "date"),
            FilterArity {
                expects_arg: true,
                arg_optional: true,
            },
        );
        specs
    }

    fn render_arity_snapshot(db: &TestDatabase, source: &str) -> String {
        crate::testing::render_validate_snapshot_filtered(db, "test.html", 0, source, |err| {
            matches!(
                err,
                ValidationError::FilterMissingArgument { .. }
                    | ValidationError::FilterUnexpectedArgument { .. }
            )
        })
    }

    fn test_db() -> TestDatabase {
        let filters = vec![
            builtin_filter_json("title", "django.template.defaultfilters"),
            builtin_filter_json("lower", "django.template.defaultfilters"),
            builtin_filter_json("default", "django.template.defaultfilters"),
            builtin_filter_json("truncatewords", "django.template.defaultfilters"),
            builtin_filter_json("truncatechars", "django.template.defaultfilters"),
            builtin_filter_json("date", "django.template.defaultfilters"),
        ];
        let template_libraries = make_template_libraries_with_filters(&filters);
        TestDatabase::new()
            .with_template_libraries(template_libraries)
            .with_arity_specs(default_arities())
    }

    #[test]
    fn filter_missing_required_arg_s115() {
        let db = test_db();
        let rendered = render_arity_snapshot(&db, "{{ value|truncatewords }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_unexpected_arg_s116() {
        let db = test_db();
        let rendered = render_arity_snapshot(&db, "{{ value|title:\"arg\" }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_with_required_arg_no_error() {
        let db = test_db();
        let rendered = render_arity_snapshot(&db, "{{ value|truncatewords:30 }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_no_arg_no_error() {
        let db = test_db();
        let rendered = render_arity_snapshot(&db, "{{ value|title }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn optional_arg_both_ways_no_error() {
        let db = test_db();
        let rendered = render_arity_snapshot(&db, "{{ value|date }}{{ value|date:\"Y-m-d\" }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_chain_validates_each() {
        let db = test_db();
        let rendered = render_arity_snapshot(&db, "{{ value|title:\"bad\"|truncatewords }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn unknown_filter_no_arity_error() {
        let db = test_db();
        let rendered = render_arity_snapshot(&db, "{{ value|unknown_filter }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn inspector_unavailable_no_arity_diagnostics() {
        let db = TestDatabase::new().with_arity_specs(default_arities());
        let rendered = render_arity_snapshot(&db, "{{ value|truncatewords }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn opaque_region_skipped() {
        let db = test_db();
        let rendered = render_arity_snapshot(
            &db,
            "{% verbatim %}{{ value|truncatewords }}{% endverbatim %}",
        );
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn empty_arity_specs_no_diagnostics() {
        let filters = vec![builtin_filter_json(
            "title",
            "django.template.defaultfilters",
        )];
        let template_libraries = make_template_libraries_with_filters(&filters);
        let db = TestDatabase::new()
            .with_template_libraries(template_libraries)
            .with_arity_specs(FilterAritySpecs::new());

        let rendered = render_arity_snapshot(&db, "{{ value|title:\"bad\" }}");
        insta::assert_snapshot!(rendered);
    }
}
