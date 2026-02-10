mod arguments;
mod blocks;
mod db;
mod errors;
mod extends;
mod filters;
mod if_expression;
mod loads;
mod opaque;
mod primitives;
mod resolution;
pub mod rule_evaluation;
mod semantic;
mod templatetags;
mod traits;

use arguments::validate_all_tag_arguments;
pub use blocks::build_block_tree;
pub use blocks::TagIndex;
pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
use extends::validate_extends;
use filters::validate_filter_arity;
pub use filters::FilterAritySpecs;
use if_expression::validate_if_expressions;
pub use loads::compute_loaded_libraries;
pub use loads::parse_load_bits;
pub use loads::validate_filter_scoping;
pub use loads::validate_load_libraries;
pub use loads::validate_tag_scoping;
pub use loads::AvailableSymbols;
pub use loads::FilterAvailability;
pub use loads::LoadKind;
pub use loads::LoadStatement;
pub use loads::LoadedLibraries;
pub use loads::TagAvailability;
use opaque::compute_opaque_regions;
pub use opaque::OpaqueRegions;
pub use primitives::Tag;
pub use primitives::Template;
pub use primitives::TemplateName;
pub use resolution::find_references_to_template;
pub use resolution::resolve_template;
pub use resolution::ResolveResult;
pub use resolution::TemplateReference;
pub use semantic::build_semantic_forest;
pub use templatetags::EndTag;
pub use templatetags::TagSpec;
pub use templatetags::TagSpecs;

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
    if nodelist.nodelist(db).is_empty() {
        return;
    }

    let block_tree = build_block_tree(db, nodelist);
    let _forest = build_semantic_forest(db, block_tree, nodelist);
    let opaque_regions = compute_opaque_regions(db, nodelist);
    validate_all_tag_arguments(db, nodelist, &opaque_regions);
    validate_tag_scoping(db, nodelist, &opaque_regions);
    validate_filter_scoping(db, nodelist, &opaque_regions);
    validate_load_libraries(db, nodelist, &opaque_regions);
    validate_if_expressions(db, nodelist, &opaque_regions);
    validate_filter_arity(db, nodelist, &opaque_regions);
    validate_extends(db, nodelist);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_project::TemplateLibraries;
    use djls_python::FilterArity;
    use djls_python::SymbolKey;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use crate::blocks::TagIndex;
    use crate::filters::arity::FilterAritySpecs;
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
        template_libraries: TemplateLibraries,
        arity_specs: FilterAritySpecs,
    }

    impl TestDatabase {
        fn with_inventory_and_arities(
            template_libraries: TemplateLibraries,
            arity_specs: FilterAritySpecs,
        ) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                template_libraries,
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

        fn template_libraries(&self) -> TemplateLibraries {
            self.template_libraries.clone()
        }

        fn filter_arity_specs(&self) -> FilterAritySpecs {
            self.arity_specs.clone()
        }
    }

    fn builtin_tag_json(name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "tag",
            "name": name,
            "load_name": null,
            "library_module": module,
            "module": module,
            "doc": null,
        })
    }

    fn builtin_filter_json(name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "filter",
            "name": name,
            "load_name": null,
            "library_module": module,
            "module": module,
            "doc": null,
        })
    }

    fn library_tag_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "tag",
            "name": name,
            "load_name": load_name,
            "library_module": module,
            "module": module,
            "doc": null,
        })
    }

    fn make_inventory(
        tags: &[serde_json::Value],
        filters: &[serde_json::Value],
        libraries: &HashMap<String, String>,
        builtins: &[String],
    ) -> TemplateLibraries {
        use std::collections::BTreeMap;

        let mut symbols: Vec<djls_project::InspectorTemplateLibrarySymbolWire> = tags
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()
            .unwrap();

        symbols.extend(
            filters
                .iter()
                .cloned()
                .map(serde_json::from_value)
                .collect::<Result<Vec<djls_project::InspectorTemplateLibrarySymbolWire>, _>>()
                .unwrap(),
        );

        let response = djls_project::TemplateLibrariesResponse {
            symbols,
            libraries: libraries
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            builtins: builtins.to_vec(),
        };

        TemplateLibraries::default().apply_inspector(Some(response))
    }

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
        make_inventory(&tags, &filters, &libraries, &builtins)
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
        TestDatabase::with_inventory_and_arities(standard_inventory(), standard_arities())
    }

    fn collect_all_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(db, file).expect("should parse");
        validate_nodelist(db, nodelist);

        validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .collect()
    }

    fn error_span_start(e: &ValidationError) -> u32 {
        match e {
            ValidationError::ExpressionSyntaxError { span, .. }
            | ValidationError::FilterMissingArgument { span, .. }
            | ValidationError::FilterUnexpectedArgument { span, .. }
            | ValidationError::UnloadedTag { span, .. }
            | ValidationError::UnknownTag { span, .. }
            | ValidationError::UnclosedTag { span, .. }
            | ValidationError::OrphanedTag { span, .. }
            | ValidationError::AmbiguousUnloadedTag { span, .. }
            | ValidationError::UnknownFilter { span, .. }
            | ValidationError::UnloadedFilter { span, .. }
            | ValidationError::AmbiguousUnloadedFilter { span, .. }
            | ValidationError::UnmatchedBlockName { span, .. }
            | ValidationError::ExtractedRuleViolation { span, .. }
            | ValidationError::TagNotInInstalledApps { span, .. }
            | ValidationError::FilterNotInInstalledApps { span, .. }
            | ValidationError::UnknownLibrary { span, .. }
            | ValidationError::LibraryNotInInstalledApps { span, .. }
            | ValidationError::ExtendsMustBeFirst { span, .. }
            | ValidationError::MultipleExtends { span, .. } => span.start(),
            ValidationError::UnbalancedStructure { opening_span, .. } => opening_span.start(),
        }
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
        let mut errors = collect_all_errors(&db, source);
        errors.sort_by_key(error_span_start);

        insta::assert_yaml_snapshot!(errors);
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
        let mut errors = collect_all_errors(&db, source);
        errors.sort_by_key(error_span_start);

        insta::assert_yaml_snapshot!(errors);
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

    use djls_corpus::module_path_from_file;
    use djls_corpus::Corpus;

    /// A test database using extraction-derived `TagSpecs`.
    ///
    /// No inspector inventory — scoping diagnostics (S108-S113) suppressed.
    /// Tests argument validation (S117), expression validation (S114),
    /// and filter arity (S115/S116).
    #[salsa::db]
    #[derive(Clone)]
    struct CorpusTestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        specs: TagSpecs,
        arity_specs: FilterAritySpecs,
    }

    impl CorpusTestDatabase {
        fn new(specs: TagSpecs, arity_specs: FilterAritySpecs) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                specs,
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
    impl salsa::Database for CorpusTestDatabase {}

    #[salsa::db]
    impl djls_source::Db for CorpusTestDatabase {
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
    impl djls_templates::Db for CorpusTestDatabase {}

    #[salsa::db]
    impl crate::Db for CorpusTestDatabase {
        fn tag_specs(&self) -> TagSpecs {
            self.specs.clone()
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

        fn template_libraries(&self) -> TemplateLibraries {
            TemplateLibraries::default()
        }

        fn filter_arity_specs(&self) -> FilterAritySpecs {
            self.arity_specs.clone()
        }
    }

    fn extract_and_merge(
        corpus: &Corpus,
        dir: &Utf8Path,
        specs: &mut TagSpecs,
        arities: &mut FilterAritySpecs,
    ) {
        for file_path in &corpus.extraction_targets_in(dir) {
            let Ok(source) = std::fs::read_to_string(file_path.as_std_path()) else {
                continue;
            };
            let module_path = module_path_from_file(file_path);
            let result = djls_python::extract_rules(&source, &module_path);
            arities.merge_extraction_result(&result);
            specs.merge_extraction_results(&result);
        }
    }

    fn build_specs_from_extraction(
        corpus: &Corpus,
        entry_dir: &Utf8Path,
    ) -> (TagSpecs, FilterAritySpecs) {
        let mut specs = TagSpecs::default();
        let mut arities = FilterAritySpecs::new();
        extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);
        (specs, arities)
    }

    fn is_argument_validation_error(err: &ValidationError) -> bool {
        matches!(
            err,
            ValidationError::ExpressionSyntaxError { .. }
                | ValidationError::FilterMissingArgument { .. }
                | ValidationError::FilterUnexpectedArgument { .. }
                | ValidationError::ExtractedRuleViolation { .. }
        )
    }

    fn validate_corpus_template(
        content: &str,
        specs: &TagSpecs,
        arities: &FilterAritySpecs,
    ) -> Vec<ValidationError> {
        let db = CorpusTestDatabase::new(specs.clone(), arities.clone());

        let path = "corpus_test.html";
        db.add_file(path, content);
        let file = db.create_file(Utf8Path::new(path));

        let Some(nodelist) = parse_template(&db, file) else {
            return Vec::new();
        };

        validate_nodelist(&db, nodelist);

        validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(is_argument_validation_error)
            .collect()
    }

    #[test]
    fn corpus_known_invalid_templates_produce_errors() {
        let corpus = Corpus::require();

        let Some(django_dir) = corpus.latest_package("django") else {
            eprintln!("No Django in corpus.");
            return;
        };

        let (specs, arities) = build_specs_from_extraction(&corpus, &django_dir);

        // for tag with wrong number of args
        let errors = validate_corpus_template("{% for %}content{% endfor %}", &specs, &arities);
        assert!(
            !errors.is_empty(),
            "Expected errors for {{% for %}} with no args"
        );

        // if expression syntax error
        let errors = validate_corpus_template("{% if and x %}content{% endif %}", &specs, &arities);
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
