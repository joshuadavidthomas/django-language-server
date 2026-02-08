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
    if db.inspector_inventory().is_none() {
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
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_extraction::FilterArity;
    use djls_extraction::SymbolKey;
    use djls_project::TemplateTags;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use crate::blocks::TagIndex;
    use crate::filter_arity::FilterAritySpecs;
    use crate::templatetags::test_tag_specs;
    use crate::validate_nodelist;
    use crate::TagSpecs;
    use crate::ValidationError;
    use crate::ValidationErrorAccumulator;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        inventory: Option<TemplateTags>,
        arity_specs: FilterAritySpecs,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: None,
                arity_specs: FilterAritySpecs::new(),
            }
        }

        fn with_inventory_and_arities(
            inventory: TemplateTags,
            arity_specs: FilterAritySpecs,
        ) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: Some(inventory),
                arity_specs,
            }
        }

        fn add_file(&self, path: &str, content: &str) {
            self.fs
                .lock()
                .unwrap()
                .add_file(path.into(), content.to_string());
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
        fn tag_specs(&self) -> TagSpecs {
            test_tag_specs()
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

        fn inspector_inventory(&self) -> Option<TemplateTags> {
            self.inventory.clone()
        }

        fn filter_arity_specs(&self) -> FilterAritySpecs {
            self.arity_specs.clone()
        }

        fn environment_inventory(&self) -> Option<djls_extraction::EnvironmentInventory> {
            None
        }
    }

    fn builtin_filter_json(name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"builtin": {"module": module}},
            "defining_module": module,
            "doc": null,
        })
    }

    fn make_inventory_with_filters(filters: &[serde_json::Value]) -> TemplateTags {
        let tags: Vec<serde_json::Value> = vec![
            serde_json::json!({
                "name": "if",
                "provenance": {"builtin": {"module": "django.template.defaulttags"}},
                "defining_module": "django.template.defaulttags",
                "doc": null,
            }),
            serde_json::json!({
                "name": "verbatim",
                "provenance": {"builtin": {"module": "django.template.defaulttags"}},
                "defining_module": "django.template.defaulttags",
                "doc": null,
            }),
            serde_json::json!({
                "name": "comment",
                "provenance": {"builtin": {"module": "django.template.defaulttags"}},
                "defining_module": "django.template.defaulttags",
                "doc": null,
            }),
        ];
        let libraries: HashMap<String, String> = HashMap::new();
        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];
        let payload = serde_json::json!({
            "tags": tags,
            "filters": filters,
            "libraries": libraries,
            "builtins": builtins,
        });
        serde_json::from_value(payload).unwrap()
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

    fn collect_arity_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(db, file).expect("should parse");
        validate_nodelist(db, nodelist);

        validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(|err| {
                matches!(
                    err,
                    ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                )
            })
            .collect()
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
        let inventory = make_inventory_with_filters(&filters);
        TestDatabase::with_inventory_and_arities(inventory, default_arities())
    }

    #[test]
    fn filter_missing_required_arg_s115() {
        let db = test_db();
        let errors = collect_arity_errors(&db, "{{ value|truncatewords }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::FilterMissingArgument { filter, .. } if filter == "truncatewords"),
            "Expected FilterMissingArgument for 'truncatewords', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn filter_unexpected_arg_s116() {
        let db = test_db();
        let errors = collect_arity_errors(&db, "{{ value|title:\"arg\" }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::FilterUnexpectedArgument { filter, .. } if filter == "title"),
            "Expected FilterUnexpectedArgument for 'title', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn filter_with_required_arg_no_error() {
        let db = test_db();
        let errors = collect_arity_errors(&db, "{{ value|truncatewords:30 }}");

        assert!(
            errors.is_empty(),
            "Filter with required arg should not produce error, got: {errors:?}"
        );
    }

    #[test]
    fn filter_no_arg_no_error() {
        let db = test_db();
        let errors = collect_arity_errors(&db, "{{ value|title }}");

        assert!(
            errors.is_empty(),
            "No-arg filter used without arg should not produce error, got: {errors:?}"
        );
    }

    #[test]
    fn optional_arg_both_ways_no_error() {
        let db = test_db();
        // date has optional arg — both with and without should be fine
        let errors = collect_arity_errors(&db, "{{ value|date }}{{ value|date:\"Y-m-d\" }}");

        assert!(
            errors.is_empty(),
            "Optional arg filter should work both ways, got: {errors:?}"
        );
    }

    #[test]
    fn filter_chain_validates_each() {
        let db = test_db();
        // title (no-arg, used with arg = S116) + truncatewords (required arg, missing = S115)
        let errors = collect_arity_errors(&db, "{{ value|title:\"bad\"|truncatewords }}");

        assert_eq!(errors.len(), 2, "Expected 2 errors, got: {errors:?}");
        let has_s116 = errors.iter().any(|e| {
            matches!(e, ValidationError::FilterUnexpectedArgument { filter, .. } if filter == "title")
        });
        let has_s115 = errors.iter().any(|e| {
            matches!(e, ValidationError::FilterMissingArgument { filter, .. } if filter == "truncatewords")
        });
        assert!(has_s116, "Expected S116 for title");
        assert!(has_s115, "Expected S115 for truncatewords");
    }

    #[test]
    fn unknown_filter_no_arity_error() {
        let db = test_db();
        // A filter not in arity specs should not produce arity errors
        let errors = collect_arity_errors(&db, "{{ value|unknown_filter }}");

        assert!(
            errors.is_empty(),
            "Unknown filter should not produce arity errors, got: {errors:?}"
        );
    }

    #[test]
    fn inspector_unavailable_no_arity_diagnostics() {
        // No inventory → no diagnostics
        let mut db = TestDatabase::new();
        db.arity_specs = default_arities();
        let errors = collect_arity_errors(&db, "{{ value|truncatewords }}");

        assert!(
            errors.is_empty(),
            "No arity diagnostics when inspector unavailable, got: {errors:?}"
        );
    }

    #[test]
    fn opaque_region_skipped() {
        let db = test_db();
        // truncatewords inside verbatim should NOT produce S115
        let errors = collect_arity_errors(
            &db,
            "{% verbatim %}{{ value|truncatewords }}{% endverbatim %}",
        );

        assert!(
            errors.is_empty(),
            "Filter inside opaque region should be skipped, got: {errors:?}"
        );
    }

    #[test]
    fn empty_arity_specs_no_diagnostics() {
        let filters = vec![builtin_filter_json(
            "title",
            "django.template.defaultfilters",
        )];
        let inventory = make_inventory_with_filters(&filters);
        let db = TestDatabase::with_inventory_and_arities(inventory, FilterAritySpecs::new());
        let errors = collect_arity_errors(&db, "{{ value|title:\"bad\" }}");

        assert!(
            errors.is_empty(),
            "Empty arity specs should not produce errors, got: {errors:?}"
        );
    }
}
