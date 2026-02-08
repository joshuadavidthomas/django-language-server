mod arguments;
mod blocks;
mod db;
mod errors;
mod filter_arity;
mod filter_validation;
mod if_expression;
mod load_resolution;
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
pub use filter_arity::FilterAritySpecs;
use filter_validation::validate_filter_arity;
use if_expression::validate_if_expressions;
pub use load_resolution::compute_loaded_libraries;
pub use load_resolution::parse_load_bits;
pub use load_resolution::validate_filter_scoping;
pub use load_resolution::validate_load_libraries;
pub use load_resolution::validate_tag_scoping;
pub use load_resolution::AvailableSymbols;
pub use load_resolution::FilterAvailability;
pub use load_resolution::LoadKind;
pub use load_resolution::LoadStatement;
pub use load_resolution::LoadedLibraries;
pub use load_resolution::TagAvailability;
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
pub use templatetags::django_builtin_specs;
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fmt::Write as _;
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
    use crate::templatetags::django_builtin_specs;
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

    fn builtin_tag_json(name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"builtin": {"module": module}},
            "defining_module": module,
            "doc": null,
        })
    }

    fn builtin_filter_json(name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"builtin": {"module": module}},
            "defining_module": module,
            "doc": null,
        })
    }

    fn library_tag_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"library": {"load_name": load_name, "module": module}},
            "defining_module": module,
            "doc": null,
        })
    }

    fn make_inventory(
        tags: &[serde_json::Value],
        filters: &[serde_json::Value],
        libraries: &HashMap<String, String>,
        builtins: &[String],
    ) -> TemplateTags {
        let payload = serde_json::json!({
            "tags": tags,
            "filters": filters,
            "libraries": libraries,
            "builtins": builtins,
        });
        serde_json::from_value(payload).unwrap()
    }

    fn default_builtins_module() -> &'static str {
        "django.template.defaulttags"
    }

    fn default_filters_module() -> &'static str {
        "django.template.defaultfilters"
    }

    fn standard_inventory() -> TemplateTags {
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
            | ValidationError::LibraryNotInInstalledApps { span, .. } => span.start(),
            ValidationError::UnbalancedStructure { opening_span, .. } => opening_span.start(),
        }
    }

    // ── Integration: Mixed diagnostics ─────────────────────────

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

    // ── Snapshot tests for diagnostic output ───────────────────

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

    // =====================================================================
    // Corpus / template validation tests
    // =====================================================================
    //
    // These tests extract rules from real Python source files and validate
    // real Django templates against those rules. They prove zero false
    // positives for argument validation (S117) at scale.
    //
    // All tests skip gracefully when source/corpus is unavailable.

    /// Locate a Django installation's source directory.
    fn find_django_source() -> Option<std::path::PathBuf> {
        if let Ok(path) = std::env::var("DJANGO_SOURCE_PATH") {
            let p = std::path::PathBuf::from(path);
            if p.is_dir() {
                return Some(p);
            }
        }

        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for venv_relative in &["../../.venv", "../../../.venv", "../../../../.venv"] {
            let venv = manifest.join(venv_relative);
            if !venv.is_dir() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(venv.join("lib")) {
                for entry in entries.flatten() {
                    let site_packages = entry.path().join("site-packages/django");
                    if site_packages.is_dir() {
                        return Some(site_packages);
                    }
                }
            }
        }
        None
    }

    /// Locate the corpus directory.
    fn find_corpus_dir() -> Option<std::path::PathBuf> {
        if let Ok(path) = std::env::var("DJLS_CORPUS_PATH") {
            let p = std::path::PathBuf::from(path);
            if p.is_dir() {
                return Some(p);
            }
        }

        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for candidate in &[
            "../../template_linter/.corpus",
            "../../../template_linter/.corpus",
        ] {
            let p = manifest.join(candidate);
            if p.is_dir() {
                return Some(p);
            }
        }
        None
    }

    /// Extract rules from a Django source tree and build `TagSpecs`.
    fn build_extraction_specs(django_root: &std::path::Path) -> TagSpecs {
        let modules = [
            ("template/defaulttags.py", "django.template.defaulttags"),
            (
                "template/defaultfilters.py",
                "django.template.defaultfilters",
            ),
            ("templatetags/i18n.py", "django.templatetags.i18n"),
            ("templatetags/static.py", "django.templatetags.static"),
            ("templatetags/l10n.py", "django.templatetags.l10n"),
            ("templatetags/tz.py", "django.templatetags.tz"),
            (
                "contrib/admin/templatetags/admin_list.py",
                "django.contrib.admin.templatetags.admin_list",
            ),
            (
                "contrib/admin/templatetags/admin_modify.py",
                "django.contrib.admin.templatetags.admin_modify",
            ),
            (
                "contrib/admin/templatetags/admin_urls.py",
                "django.contrib.admin.templatetags.admin_urls",
            ),
            (
                "contrib/admin/templatetags/log.py",
                "django.contrib.admin.templatetags.log",
            ),
        ];

        let mut combined = djls_extraction::ExtractionResult::default();
        for (rel_path, module_path) in &modules {
            let file = django_root.join(rel_path);
            if let Ok(source) = std::fs::read_to_string(&file) {
                let result = djls_extraction::extract_rules(&source, module_path);
                combined.merge(result);
            }
        }

        let mut specs = django_builtin_specs();
        specs.merge_extraction_results(&combined);
        specs
    }

    /// Build `FilterAritySpecs` from extraction results.
    fn build_extraction_arities(django_root: &std::path::Path) -> FilterAritySpecs {
        let modules = [
            (
                "template/defaultfilters.py",
                "django.template.defaultfilters",
            ),
            ("templatetags/i18n.py", "django.templatetags.i18n"),
            ("templatetags/static.py", "django.templatetags.static"),
            ("templatetags/l10n.py", "django.templatetags.l10n"),
            ("templatetags/tz.py", "django.templatetags.tz"),
        ];

        let mut arities = FilterAritySpecs::new();
        for (rel_path, module_path) in &modules {
            let file = django_root.join(rel_path);
            if let Ok(source) = std::fs::read_to_string(&file) {
                let result = djls_extraction::extract_rules(&source, module_path);
                for (key, arity) in result.filter_arities {
                    arities.insert(key, arity);
                }
            }
        }
        arities
    }

    /// A test database that uses extraction-derived `TagSpecs`.
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

        fn inspector_inventory(&self) -> Option<TemplateTags> {
            // No inspector — scoping diagnostics suppressed
            None
        }

        fn filter_arity_specs(&self) -> FilterAritySpecs {
            self.arity_specs.clone()
        }

        fn environment_inventory(&self) -> Option<djls_extraction::EnvironmentInventory> {
            None
        }
    }

    /// Collect only argument-validation and expression-validation errors.
    ///
    /// Filters to S114 (expression syntax), S115-S116 (filter arity),
    /// and S117 (extracted rule violation).
    /// Excludes structural errors (S101-S103), scoping errors (S108-S113).
    fn collect_validation_errors(
        db: &CorpusTestDatabase,
        source: &str,
        path: &str,
    ) -> Vec<ValidationError> {
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let Some(nodelist) = parse_template(db, file) else {
            return vec![];
        };
        validate_nodelist(db, nodelist);

        validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::ExpressionSyntaxError { .. }
                        | ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                        | ValidationError::ExtractedRuleViolation { .. }
                )
            })
            .collect()
    }

    /// Collect all template files under a directory tree.
    ///
    /// Excludes Jinja2 templates (in `jinja2/` directories), static files,
    /// and non-Django JS templates.
    fn collect_template_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                if !e.file_type().is_file() {
                    return false;
                }
                let ext_ok = e
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "html" || ext == "txt");
                if !ext_ok {
                    return false;
                }
                // Exclude Jinja2 templates
                let path_str = e.path().to_string_lossy();
                if path_str.contains("/jinja2/") || path_str.contains("\\jinja2\\") {
                    return false;
                }
                // Exclude static directories (may contain AngularJS or other non-Django templates)
                if path_str.contains("/static/") || path_str.contains("\\static\\") {
                    return false;
                }
                true
            })
            .map(walkdir::DirEntry::into_path)
            .collect()
    }

    #[test]
    fn corpus_django_shipped_templates_zero_false_positives() {
        let Some(django_root) = find_django_source() else {
            eprintln!("SKIP: Django source not found (set DJANGO_SOURCE_PATH or create .venv)");
            return;
        };

        let specs = build_extraction_specs(&django_root);
        let arities = build_extraction_arities(&django_root);
        let db = CorpusTestDatabase::new(specs, arities);

        // Collect templates from contrib/admin and forms
        let mut template_paths = Vec::new();
        for base in &["contrib", "forms"] {
            let dir = django_root.join(base);
            if dir.is_dir() {
                template_paths.extend(collect_template_files(&dir));
            }
        }

        if template_paths.is_empty() {
            eprintln!("SKIP: No Django shipped templates found");
            return;
        }

        let mut failures: Vec<(String, Vec<ValidationError>)> = Vec::new();

        for (i, path) in template_paths.iter().enumerate() {
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            // Use a unique path per file to avoid Salsa caching conflicts
            let test_path = format!("template_{i}.html");
            let errors = collect_validation_errors(&db, &source, &test_path);
            if !errors.is_empty() {
                let rel_path = path
                    .strip_prefix(&django_root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                failures.push((rel_path, errors));
            }
        }

        if !failures.is_empty() {
            let mut msg = format!(
                "Argument/expression validation false positives in {} of {} Django templates:\n",
                failures.len(),
                template_paths.len()
            );
            for (path, errors) in &failures {
                let _ = writeln!(msg, "\n  {path}:");
                for e in errors.iter().take(5) {
                    let _ = writeln!(msg, "    - {e}");
                }
                if errors.len() > 5 {
                    let _ = writeln!(msg, "    ... and {} more", errors.len() - 5);
                }
            }
            panic!("{msg}");
        }

        eprintln!(
            "Validated {} Django shipped templates with zero false positives",
            template_paths.len()
        );
    }

    #[test]
    fn corpus_django_versions_templates_zero_false_positives() {
        let Some(corpus) = find_corpus_dir() else {
            eprintln!("SKIP: corpus not found (set DJLS_CORPUS_PATH or sync corpus)");
            return;
        };

        let django_dir = corpus.join("packages/Django");
        if !django_dir.is_dir() {
            eprintln!("SKIP: Django not found in corpus");
            return;
        }

        let mut versions_tested = 0;

        for entry in std::fs::read_dir(&django_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let version_dir = entry.path();
            if !version_dir.is_dir() {
                continue;
            }

            // Each Django version entry should have django/ subdirectory
            let django_root = version_dir.join("django");
            if !django_root.is_dir() {
                continue;
            }

            let specs = build_extraction_specs(&django_root);
            let arities = build_extraction_arities(&django_root);
            let db = CorpusTestDatabase::new(specs, arities);

            let mut template_paths = Vec::new();
            for base in &["contrib", "forms"] {
                let dir = django_root.join(base);
                if dir.is_dir() {
                    template_paths.extend(collect_template_files(&dir));
                }
            }

            if template_paths.is_empty() {
                continue;
            }

            let mut failures: Vec<(String, Vec<ValidationError>)> = Vec::new();

            for (i, path) in template_paths.iter().enumerate() {
                let Ok(source) = std::fs::read_to_string(path) else {
                    continue;
                };
                let test_path = format!("template_{i}.html");
                let errors = collect_validation_errors(&db, &source, &test_path);
                if !errors.is_empty() {
                    let rel_path = path
                        .strip_prefix(&version_dir)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string();
                    failures.push((rel_path, errors));
                }
            }

            let version = entry.file_name().to_string_lossy().to_string();
            assert!(
                failures.is_empty(),
                "Django {version}: false positives in {} of {} templates:\n{:?}",
                failures.len(),
                template_paths.len(),
                failures
                    .iter()
                    .map(|(p, e)| format!("{p}: {e:?}"))
                    .collect::<Vec<_>>()
            );

            versions_tested += 1;
            eprintln!(
                "Django {version}: validated {} templates OK",
                template_paths.len()
            );
        }

        if versions_tested == 0 {
            eprintln!("SKIP: no Django versions with templates found in corpus");
        } else {
            eprintln!("Tested {versions_tested} Django version(s)");
        }
    }

    #[test]
    fn corpus_third_party_templates_zero_false_positives() {
        let Some(corpus) = find_corpus_dir() else {
            eprintln!("SKIP: corpus not found");
            return;
        };

        // Extract Django builtins rules from local venv
        let django_root = find_django_source();
        let base_specs = if let Some(ref root) = django_root {
            build_extraction_specs(root)
        } else {
            django_builtin_specs()
        };
        let base_arities = if let Some(ref root) = django_root {
            build_extraction_arities(root)
        } else {
            FilterAritySpecs::new()
        };

        let mut entries_tested = 0;

        // Test packages
        let packages_dir = corpus.join("packages");
        if packages_dir.is_dir() {
            for package_entry in std::fs::read_dir(&packages_dir)
                .into_iter()
                .flatten()
                .flatten()
            {
                let package_name = package_entry.file_name().to_string_lossy().to_string();
                if package_name == "Django" {
                    continue; // Tested separately
                }

                test_corpus_entry(
                    &package_entry.path(),
                    &package_name,
                    &base_specs,
                    &base_arities,
                    &mut entries_tested,
                );
            }
        }

        // Test repos
        let repos_dir = corpus.join("repos");
        if repos_dir.is_dir() {
            for repo_entry in std::fs::read_dir(&repos_dir)
                .into_iter()
                .flatten()
                .flatten()
            {
                let repo_name = repo_entry.file_name().to_string_lossy().to_string();
                test_corpus_entry(
                    &repo_entry.path(),
                    &repo_name,
                    &base_specs,
                    &base_arities,
                    &mut entries_tested,
                );
            }
        }

        if entries_tested == 0 {
            eprintln!("SKIP: no corpus entries with templates found");
        } else {
            eprintln!("Tested {entries_tested} corpus entries");
        }
    }

    /// Test a single corpus entry (package or repo) for template validation.
    fn test_corpus_entry(
        entry_root: &std::path::Path,
        entry_name: &str,
        base_specs: &TagSpecs,
        base_arities: &FilterAritySpecs,
        entries_tested: &mut usize,
    ) {
        // Find all version subdirectories or treat entry_root itself
        let version_dirs: Vec<std::path::PathBuf> =
            if let Ok(entries) = std::fs::read_dir(entry_root) {
                let dirs: Vec<_> = entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.path())
                    .collect();
                if dirs.is_empty() {
                    vec![entry_root.to_path_buf()]
                } else {
                    dirs
                }
            } else {
                return;
            };

        for version_dir in &version_dirs {
            // Collect templates
            let template_paths = collect_template_files(version_dir);
            if template_paths.is_empty() {
                continue;
            }

            // Extract entry-local rules from Python files
            let py_files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(version_dir)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_type().is_file()
                        && e.path().extension().is_some_and(|ext| ext == "py")
                        && !e.file_name().to_string_lossy().starts_with("__")
                })
                .map(walkdir::DirEntry::into_path)
                .collect();

            let mut entry_extraction = djls_extraction::ExtractionResult::default();
            for py_file in &py_files {
                if let Ok(source) = std::fs::read_to_string(py_file) {
                    let module_path = py_file
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, ".")
                        .trim_end_matches(".py")
                        .to_string();
                    let result = djls_extraction::extract_rules(&source, &module_path);
                    entry_extraction.merge(result);
                }
            }

            // Merge base specs with entry-local extraction
            let mut specs = base_specs.clone();
            specs.merge_extraction_results(&entry_extraction);

            let mut arities = base_arities.clone();
            for (key, arity) in entry_extraction.filter_arities {
                arities.insert(key, arity);
            }

            let db = CorpusTestDatabase::new(specs, arities);

            let mut failures: Vec<(String, Vec<ValidationError>)> = Vec::new();

            for (i, path) in template_paths.iter().enumerate() {
                let Ok(source) = std::fs::read_to_string(path) else {
                    continue;
                };
                let test_path = format!("template_{i}.html");
                let errors = collect_validation_errors(&db, &source, &test_path);
                if !errors.is_empty() {
                    let rel_path = path
                        .strip_prefix(version_dir)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string();
                    failures.push((rel_path, errors));
                }
            }

            if failures.is_empty() {
                eprintln!(
                    "{entry_name}: validated {} templates OK",
                    template_paths.len()
                );
            } else {
                let version = version_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                eprintln!(
                    "WARNING: {entry_name}/{version}: {} false positive(s) in {} templates",
                    failures.len(),
                    template_paths.len()
                );
                for (path, errors) in &failures {
                    for e in errors.iter().take(3) {
                        eprintln!("  {path}: {e}");
                    }
                }
                // Don't fail for third-party — extraction coverage is partial
                // Only Django shipped templates are required to be zero false positives
            }

            *entries_tested += 1;
        }
    }

    /// Known-invalid templates produce expected errors.
    #[test]
    fn corpus_known_invalid_templates_produce_errors() {
        let django_root = find_django_source();
        let specs = if let Some(ref root) = django_root {
            build_extraction_specs(root)
        } else {
            django_builtin_specs()
        };
        let arities = if let Some(ref root) = django_root {
            build_extraction_arities(root)
        } else {
            FilterAritySpecs::new()
        };
        let db = CorpusTestDatabase::new(specs, arities);

        // for tag with wrong number of args
        let errors =
            collect_validation_errors(&db, "{% for %}content{% endfor %}", "invalid_for.html");
        assert!(
            !errors.is_empty(),
            "Expected errors for {{% for %}} with no args"
        );

        // if expression syntax error
        let errors =
            collect_validation_errors(&db, "{% if and x %}content{% endif %}", "invalid_if.html");
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
