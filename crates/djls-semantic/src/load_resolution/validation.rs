use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use djls_templates::NodeList;
use salsa::Accumulator;

use super::compute_loaded_libraries;
use super::symbols::AvailableSymbols;
use super::symbols::FilterAvailability;
use super::symbols::TagAvailability;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Validate tag scoping for all tags in a template.
///
/// Checks each tag against the set of tags available at its position
/// (builtins + tags from loaded libraries), producing:
///
/// - **S108** (`UnknownTag`): tag is not known at all (not in inspector inventory)
/// - **S109** (`UnloadedTag`): tag is known but its library isn't loaded
/// - **S110** (`AmbiguousUnloadedTag`): tag is known but defined in multiple unloaded libraries
///
/// **Guards:**
/// - If the inspector inventory is `None`, all scoping diagnostics are suppressed.
/// - Structural tags (openers, closers, intermediates with `TagSpec`s) are skipped.
/// - The `load` tag itself is skipped (it's a builtin that defines scoping).
pub fn validate_tag_scoping(
    db: &dyn Db,
    nodelist: NodeList<'_>,
    opaque_regions: &crate::OpaqueRegions,
) {
    let Some(inventory) = db.inspector_inventory() else {
        return;
    };

    let tag_specs = db.tag_specs();
    let loaded_libraries = compute_loaded_libraries(db, nodelist);

    for node in nodelist.nodelist(db) {
        let Node::Tag { name, span, .. } = node else {
            continue;
        };

        if opaque_regions.is_opaque(span.start()) {
            continue;
        }

        // Skip the "load" tag itself — it defines scoping, not a user-visible tag
        if name == "load" {
            continue;
        }

        // Skip closers and intermediates — their availability is determined
        // by their opener tag, not by load scoping.
        if is_closer_or_intermediate(name, &tag_specs) {
            continue;
        }

        let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
        let symbols = AvailableSymbols::at_position(&loaded_libraries, &inventory, span.start());

        match symbols.check(name) {
            TagAvailability::Available => {}
            TagAvailability::Unknown => {
                ValidationErrorAccumulator(ValidationError::UnknownTag {
                    tag: name.clone(),
                    span: marker_span,
                })
                .accumulate(db);
            }
            TagAvailability::Unloaded { library } => {
                ValidationErrorAccumulator(ValidationError::UnloadedTag {
                    tag: name.clone(),
                    library,
                    span: marker_span,
                })
                .accumulate(db);
            }
            TagAvailability::AmbiguousUnloaded { libraries } => {
                ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedTag {
                    tag: name.clone(),
                    libraries,
                    span: marker_span,
                })
                .accumulate(db);
            }
        }
    }
}

/// Check whether a tag is a closer or intermediate — these are part of block
/// structure and their availability is determined by their opener tag, not by
/// load scoping. For example, `{% endif %}` and `{% else %}` should never
/// produce S108/S109/S110.
///
/// Openers and standalone tags are NOT excluded — they need scoping checks
/// even if they have a `TagSpec` (e.g., `{% trans %}` has a spec for argument
/// validation but still requires `{% load i18n %}`).
fn is_closer_or_intermediate(name: &str, tag_specs: &crate::TagSpecs) -> bool {
    tag_specs.get_end_spec_for_closer(name).is_some()
        || tag_specs.get_intermediate_spec(name).is_some()
}

/// Validate filter scoping for all filters in variable nodes in a template.
///
/// Checks each filter in `{{ var|filter }}` expressions against the set of
/// filters available at its position (builtins + filters from loaded libraries),
/// producing:
///
/// - **S111** (`UnknownFilter`): filter is not known at all (not in inspector inventory)
/// - **S112** (`UnloadedFilter`): filter is known but its library isn't loaded
/// - **S113** (`AmbiguousUnloadedFilter`): filter is known but defined in multiple unloaded libraries
///
/// **Guards:**
/// - If the inspector inventory is `None`, all filter scoping diagnostics are suppressed.
pub fn validate_filter_scoping(
    db: &dyn Db,
    nodelist: NodeList<'_>,
    opaque_regions: &crate::OpaqueRegions,
) {
    let Some(inventory) = db.inspector_inventory() else {
        return;
    };

    let loaded_libraries = compute_loaded_libraries(db, nodelist);

    for node in nodelist.nodelist(db) {
        let Node::Variable { filters, span, .. } = node else {
            continue;
        };

        if opaque_regions.is_opaque(span.start()) {
            continue;
        }

        for filter in filters {
            let symbols =
                AvailableSymbols::at_position(&loaded_libraries, &inventory, span.start());

            match symbols.check_filter(filter.name()) {
                FilterAvailability::Available => {}
                FilterAvailability::Unknown => {
                    ValidationErrorAccumulator(ValidationError::UnknownFilter {
                        filter: filter.name().to_string(),
                        span: filter.span(),
                    })
                    .accumulate(db);
                }
                FilterAvailability::Unloaded { library } => {
                    ValidationErrorAccumulator(ValidationError::UnloadedFilter {
                        filter: filter.name().to_string(),
                        library,
                        span: filter.span(),
                    })
                    .accumulate(db);
                }
                FilterAvailability::AmbiguousUnloaded { libraries } => {
                    ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedFilter {
                        filter: filter.name().to_string(),
                        libraries,
                        span: filter.span(),
                    })
                    .accumulate(db);
                }
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
    use djls_project::TemplateTags;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use crate::blocks::TagIndex;
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
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: None,
            }
        }

        fn with_inventory(inventory: TemplateTags) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: Some(inventory),
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

        fn filter_arity_specs(&self) -> crate::filter_arity::FilterAritySpecs {
            crate::filter_arity::FilterAritySpecs::new()
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

    fn library_tag_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"library": {"load_name": load_name, "module": module}},
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

    fn library_filter_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"library": {"load_name": load_name, "module": module}},
            "defining_module": module,
            "doc": null,
        })
    }

    fn make_inventory(
        tags: &[serde_json::Value],
        libraries: &HashMap<String, String>,
        builtins: &[String],
    ) -> TemplateTags {
        make_inventory_with_filters(tags, &[], libraries, builtins)
    }

    fn make_inventory_with_filters(
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

    fn test_inventory() -> TemplateTags {
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            builtin_tag_json("for", "django.template.defaulttags"),
            builtin_tag_json("block", "django.template.loader_tags"),
            builtin_tag_json("csrf_token", "django.template.defaulttags"),
            builtin_tag_json("verbatim", "django.template.defaulttags"),
            builtin_tag_json("comment", "django.template.defaulttags"),
            library_tag_json("trans", "i18n", "django.templatetags.i18n"),
            library_tag_json("blocktrans", "i18n", "django.templatetags.i18n"),
            library_tag_json("static", "static", "django.templatetags.static"),
        ];

        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.loader_tags".to_string(),
        ];

        make_inventory(&tags, &libraries, &builtins)
    }

    fn collect_scoping_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
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
                    ValidationError::UnknownTag { .. }
                        | ValidationError::UnloadedTag { .. }
                        | ValidationError::AmbiguousUnloadedTag { .. }
                )
            })
            .collect()
    }

    #[test]
    fn unknown_tag_produces_s108() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% xyz %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnknownTag { tag, .. } if tag == "xyz"),
            "Expected UnknownTag for 'xyz', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn unloaded_library_tag_produces_s109() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% trans 'hello' %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnloadedTag { tag, library, .. }
                    if tag == "trans" && library == "i18n"
            ),
            "Expected UnloadedTag for 'trans' requiring 'i18n', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn tag_in_multiple_libraries_produces_s110() {
        let tags = vec![
            library_tag_json("shared", "lib_a", "app.templatetags.lib_a"),
            library_tag_json("shared", "lib_b", "app.templatetags.lib_b"),
        ];
        let mut libraries = HashMap::new();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());
        let inventory = make_inventory(&tags, &libraries, &[]);

        let db = TestDatabase::with_inventory(inventory);
        let errors = collect_scoping_errors(&db, "{% shared %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::AmbiguousUnloadedTag { tag, libraries, .. }
                    if tag == "shared" && libraries == &["lib_a", "lib_b"]
            ),
            "Expected AmbiguousUnloadedTag for 'shared', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn inspector_unavailable_no_scoping_diagnostics() {
        // No inventory set — inspector unavailable
        let db = TestDatabase::new();
        let errors = collect_scoping_errors(&db, "{% xyz %}{% trans 'hello' %}");

        assert!(
            errors.is_empty(),
            "No scoping diagnostics when inspector unavailable, got: {errors:?}"
        );
    }

    #[test]
    fn structural_tags_skip_scoping_checks() {
        let db = TestDatabase::with_inventory(test_inventory());
        // endif, else, elif are structural — they shouldn't produce S108
        let errors =
            collect_scoping_errors(&db, "{% if True %}{% elif False %}{% else %}{% endif %}");

        assert!(
            errors.is_empty(),
            "Structural tags should not produce scoping errors, got: {errors:?}"
        );
    }

    #[test]
    fn loaded_library_tag_no_error() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% load i18n %}\n{% trans 'hello' %}");

        assert!(
            errors.is_empty(),
            "Loaded library tag should not produce error, got: {errors:?}"
        );
    }

    #[test]
    fn tag_before_load_produces_error() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% trans 'hello' %}\n{% load i18n %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnloadedTag { tag, library, .. }
                if tag == "trans" && library == "i18n"),
            "Tag before load should produce UnloadedTag, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn selective_import_makes_only_imported_symbol_available() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source =
            "{% load trans from i18n %}\n{% trans 'hello' %}\n{% blocktrans %}{% endblocktrans %}";
        let errors = collect_scoping_errors(&db, source);

        // trans should be available, blocktrans should NOT (only selectively imported trans)
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnloadedTag { tag, library, .. }
                if tag == "blocktrans" && library == "i18n"),
            "Selectively-unimported tag should produce UnloadedTag, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn builtin_tag_always_available() {
        let db = TestDatabase::with_inventory(test_inventory());
        // csrf_token is a builtin — should be available without any load
        let errors = collect_scoping_errors(&db, "{% csrf_token %}");

        assert!(
            errors.is_empty(),
            "Builtin tags should always be available, got: {errors:?}"
        );
    }

    #[test]
    fn load_tag_itself_not_flagged() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% load i18n %}");

        assert!(
            errors.is_empty(),
            "Load tag itself should not be flagged, got: {errors:?}"
        );
    }

    // --- Filter scoping tests ---

    fn collect_filter_scoping_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
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
                    ValidationError::UnknownFilter { .. }
                        | ValidationError::UnloadedFilter { .. }
                        | ValidationError::AmbiguousUnloadedFilter { .. }
                )
            })
            .collect()
    }

    fn test_inventory_with_filters() -> TemplateTags {
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            builtin_tag_json("for", "django.template.defaulttags"),
            builtin_tag_json("block", "django.template.loader_tags"),
            builtin_tag_json("csrf_token", "django.template.defaulttags"),
            builtin_tag_json("verbatim", "django.template.defaulttags"),
            builtin_tag_json("comment", "django.template.defaulttags"),
            library_tag_json("trans", "i18n", "django.templatetags.i18n"),
            library_tag_json("static", "static", "django.templatetags.static"),
        ];

        let filters = vec![
            builtin_filter_json("title", "django.template.defaultfilters"),
            builtin_filter_json("lower", "django.template.defaultfilters"),
            builtin_filter_json("default", "django.template.defaultfilters"),
            library_filter_json(
                "apnumber",
                "humanize",
                "django.contrib.humanize.templatetags.humanize",
            ),
            library_filter_json(
                "intcomma",
                "humanize",
                "django.contrib.humanize.templatetags.humanize",
            ),
        ];

        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert(
            "humanize".to_string(),
            "django.contrib.humanize.templatetags.humanize".to_string(),
        );

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.loader_tags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];

        make_inventory_with_filters(&tags, &filters, &libraries, &builtins)
    }

    #[test]
    fn unknown_filter_produces_s111() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let errors = collect_filter_scoping_errors(&db, "{{ value|nonexistent }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnknownFilter { filter, .. } if filter == "nonexistent"),
            "Expected UnknownFilter for 'nonexistent', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn unloaded_library_filter_produces_s112() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let errors = collect_filter_scoping_errors(&db, "{{ value|apnumber }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnloadedFilter { filter, library, .. }
                    if filter == "apnumber" && library == "humanize"
            ),
            "Expected UnloadedFilter for 'apnumber' requiring 'humanize', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn filter_in_multiple_libraries_produces_s113() {
        let filters = vec![
            library_filter_json("myfilter", "lib_a", "app.templatetags.lib_a"),
            library_filter_json("myfilter", "lib_b", "app.templatetags.lib_b"),
        ];
        let mut libraries = HashMap::new();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());
        let inventory = make_inventory_with_filters(&[], &filters, &libraries, &[]);

        let db = TestDatabase::with_inventory(inventory);
        let errors = collect_filter_scoping_errors(&db, "{{ value|myfilter }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::AmbiguousUnloadedFilter { filter, libraries, .. }
                    if filter == "myfilter" && libraries == &["lib_a", "lib_b"]
            ),
            "Expected AmbiguousUnloadedFilter for 'myfilter', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn filter_after_load_is_valid() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let errors =
            collect_filter_scoping_errors(&db, "{% load humanize %}\n{{ value|apnumber }}");

        assert!(
            errors.is_empty(),
            "Filter after load should not produce error, got: {errors:?}"
        );
    }

    #[test]
    fn builtin_filter_always_valid() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let errors = collect_filter_scoping_errors(&db, "{{ value|title }}");

        assert!(
            errors.is_empty(),
            "Builtin filter should always be valid, got: {errors:?}"
        );
    }

    #[test]
    fn inspector_unavailable_no_filter_diagnostics() {
        let db = TestDatabase::new();
        let errors = collect_filter_scoping_errors(&db, "{{ value|nonexistent }}{{ x|apnumber }}");

        assert!(
            errors.is_empty(),
            "No filter scoping diagnostics when inspector unavailable, got: {errors:?}"
        );
    }

    #[test]
    fn filter_chain_validates_each_filter() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        // title is builtin (valid), apnumber requires humanize (unloaded)
        let errors = collect_filter_scoping_errors(&db, "{{ value|title|apnumber }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnloadedFilter { filter, library, .. }
                    if filter == "apnumber" && library == "humanize"
            ),
            "Expected UnloadedFilter for 'apnumber' in chain, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn selective_import_filter() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let source =
            "{% load apnumber from humanize %}\n{{ value|apnumber }}\n{{ value|intcomma }}";
        let errors = collect_filter_scoping_errors(&db, source);

        // apnumber should be available (selectively imported), intcomma should NOT
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnloadedFilter { filter, library, .. }
                    if filter == "intcomma" && library == "humanize"
            ),
            "Selectively-unimported filter should produce UnloadedFilter, got: {:?}",
            errors[0]
        );
    }

    // ── Opaque region tests ────────────────────────────────────

    #[test]
    fn verbatim_block_content_skipped() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source =
            "{% verbatim %}{% trans 'hello' %}{% endverbatim %}\n{% if True %}{% endif %}";
        let errors = collect_scoping_errors(&db, source);

        // trans inside verbatim should NOT trigger S109 (UnloadedTag)
        // if/endif are builtins and should validate fine
        assert!(
            errors.is_empty(),
            "Expected no errors — content inside verbatim should be skipped. Got: {errors:?}"
        );
    }

    #[test]
    fn comment_block_content_skipped() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source = "{% comment %}{% trans 'hello' %}{% endcomment %}";
        let errors = collect_scoping_errors(&db, source);

        assert!(
            errors.is_empty(),
            "Expected no errors — content inside comment should be skipped. Got: {errors:?}"
        );
    }

    #[test]
    fn non_opaque_blocks_validated_normally() {
        let db = TestDatabase::with_inventory(test_inventory());
        // trans inside if block (non-opaque) should still be validated
        let source = "{% if True %}{% trans 'hello' %}{% endif %}";
        let errors = collect_scoping_errors(&db, source);

        // trans is a library tag not loaded — should get S109
        assert_eq!(errors.len(), 1, "Expected S109 for unloaded trans. Got: {errors:?}");
        assert!(matches!(
            &errors[0],
            ValidationError::UnloadedTag { tag, .. } if tag == "trans"
        ));
    }

    #[test]
    fn content_after_opaque_block_still_validated() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source = "{% verbatim %}{% trans 'skip' %}{% endverbatim %}{% trans 'check' %}";
        let errors = collect_scoping_errors(&db, source);

        // First trans inside verbatim: skipped
        // Second trans after verbatim: should get S109
        assert_eq!(
            errors.len(),
            1,
            "Expected 1 error for trans after verbatim. Got: {errors:?}"
        );
        assert!(matches!(
            &errors[0],
            ValidationError::UnloadedTag { tag, .. } if tag == "trans"
        ));
    }

    #[test]
    fn filter_inside_verbatim_skipped() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let source = "{% verbatim %}{{ value|intcomma }}{% endverbatim %}";
        let errors = collect_filter_scoping_errors(&db, source);

        assert!(
            errors.is_empty(),
            "Expected no errors — filter inside verbatim should be skipped. Got: {errors:?}"
        );
    }
}
