use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use djls_templates::NodeList;
use salsa::Accumulator;

use super::compute_loaded_libraries;
use super::parse_load_bits;
use super::symbols::AvailableSymbols;
use super::symbols::FilterAvailability;
use super::symbols::TagAvailability;
use super::LoadKind;
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
    let env_inventory = db.environment_inventory();
    let env_tags = env_inventory
        .as_ref()
        .map(djls_extraction::EnvironmentInventory::tags_by_name);

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
                // Check environment inventory: is the tag installed but not in INSTALLED_APPS?
                if let Some(env_tags) = &env_tags {
                    if let Some(env_symbols) = env_tags.get(name.as_str()) {
                        if env_symbols.len() == 1 {
                            let sym = &env_symbols[0];
                            ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                                tag: name.clone(),
                                app: sym.app_module.clone(),
                                load_name: sym.library_load_name.clone(),
                                span: marker_span,
                            })
                            .accumulate(db);
                        } else {
                            // Multiple candidates — include all in the message
                            // Pick the first one for the primary diagnostic, list all apps
                            let sym = &env_symbols[0];
                            let mut msg_parts: Vec<String> = env_symbols
                                .iter()
                                .map(|s| format!("'{}' (app: {})", s.library_load_name, s.app_module))
                                .collect();
                            msg_parts.sort();
                            ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                                tag: name.clone(),
                                app: sym.app_module.clone(),
                                load_name: sym.library_load_name.clone(),
                                span: marker_span,
                            })
                            .accumulate(db);
                        }
                        continue;
                    }
                }
                // Truly unknown — not in environment at all
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
    let env_inventory = db.environment_inventory();
    let env_filters = env_inventory
        .as_ref()
        .map(djls_extraction::EnvironmentInventory::filters_by_name);

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

            match symbols.check_filter(&filter.name) {
                FilterAvailability::Available => {}
                FilterAvailability::Unknown => {
                    // Check environment inventory: is the filter installed but not in INSTALLED_APPS?
                    if let Some(env_filters) = &env_filters {
                        if let Some(env_symbols) = env_filters.get(filter.name.as_str()) {
                            if !env_symbols.is_empty() {
                                let sym = &env_symbols[0];
                                ValidationErrorAccumulator(
                                    ValidationError::FilterNotInInstalledApps {
                                        filter: filter.name.clone(),
                                        app: sym.app_module.clone(),
                                        load_name: sym.library_load_name.clone(),
                                        span: filter.span,
                                    },
                                )
                                .accumulate(db);
                                continue;
                            }
                        }
                    }
                    // Truly unknown — not in environment at all
                    ValidationErrorAccumulator(ValidationError::UnknownFilter {
                        filter: filter.name.clone(),
                        span: filter.span,
                    })
                    .accumulate(db);
                }
                FilterAvailability::Unloaded { library } => {
                    ValidationErrorAccumulator(ValidationError::UnloadedFilter {
                        filter: filter.name.clone(),
                        library,
                        span: filter.span,
                    })
                    .accumulate(db);
                }
                FilterAvailability::AmbiguousUnloaded { libraries } => {
                    ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedFilter {
                        filter: filter.name.clone(),
                        libraries,
                        span: filter.span,
                    })
                    .accumulate(db);
                }
            }
        }
    }
}

/// Validate `{% load %}` library names against the inspector inventory.
///
/// For each `{% load %}` tag, checks that all referenced library names
/// exist in the inspector's `libraries()` map, producing:
///
/// - **S120** (`UnknownLibrary`): library name is not known to the inspector or environment
/// - **S121** (`LibraryNotInInstalledApps`): library exists in the environment but the app
///   is not in `INSTALLED_APPS`
///
/// Handles both full loads (`{% load i18n %}`) and selective imports
/// (`{% load trans from i18n %}` — validates `i18n`).
///
/// **Guards:**
/// - If the inspector inventory is `None`, all library diagnostics are suppressed.
pub fn validate_load_libraries(
    db: &dyn Db,
    nodelist: NodeList<'_>,
    opaque_regions: &crate::OpaqueRegions,
) {
    let Some(inventory) = db.inspector_inventory() else {
        return;
    };

    let known_libraries = inventory.libraries();
    let env_inventory = db.environment_inventory();

    for node in nodelist.nodelist(db) {
        let Node::Tag {
            name, bits, span, ..
        } = node
        else {
            continue;
        };

        if name != "load" {
            continue;
        }

        if opaque_regions.is_opaque(span.start()) {
            continue;
        }

        let Some(kind) = parse_load_bits(bits) else {
            continue;
        };

        let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

        let libraries_to_check: Vec<&str> = match &kind {
            LoadKind::FullLoad { libraries } => libraries.iter().map(String::as_str).collect(),
            LoadKind::SelectiveImport { library, .. } => vec![library.as_str()],
        };

        for lib_name in libraries_to_check {
            if known_libraries.contains_key(lib_name) {
                continue;
            }

            if let Some(ref env) = env_inventory {
                if env.has_library(lib_name) {
                    let env_libs = env.libraries_for_name(lib_name);
                    let candidates: Vec<String> = env_libs
                        .iter()
                        .map(|lib| lib.app_module.clone())
                        .collect();
                    let app = candidates.first().cloned().unwrap_or_default();
                    ValidationErrorAccumulator(ValidationError::LibraryNotInInstalledApps {
                        name: lib_name.to_string(),
                        app,
                        candidates,
                        span: marker_span,
                    })
                    .accumulate(db);
                    continue;
                }
            }

            ValidationErrorAccumulator(ValidationError::UnknownLibrary {
                name: lib_name.to_string(),
                span: marker_span,
            })
            .accumulate(db);
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
        env_inventory: Option<djls_extraction::EnvironmentInventory>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: None,
                env_inventory: None,
            }
        }

        fn with_inventory(inventory: TemplateTags) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: Some(inventory),
                env_inventory: None,
            }
        }

        fn with_inventories(
            inventory: TemplateTags,
            env_inventory: djls_extraction::EnvironmentInventory,
        ) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: Some(inventory),
                env_inventory: Some(env_inventory),
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

        fn environment_inventory(&self) -> Option<djls_extraction::EnvironmentInventory> {
            self.env_inventory.clone()
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
        let source = "{% verbatim %}{% trans 'hello' %}{% endverbatim %}\n{% if True %}{% endif %}";
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
        assert_eq!(
            errors.len(),
            1,
            "Expected S109 for unloaded trans. Got: {errors:?}"
        );
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

    // ── Library name validation tests (S120) ───────────────────

    fn collect_library_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
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
                    ValidationError::UnknownLibrary { .. }
                        | ValidationError::LibraryNotInInstalledApps { .. }
                )
            })
            .collect()
    }

    #[test]
    fn known_library_valid() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_library_errors(&db, "{% load i18n %}");

        assert!(
            errors.is_empty(),
            "Known library should not produce error, got: {errors:?}"
        );
    }

    #[test]
    fn unknown_library_produces_s120() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_library_errors(&db, "{% load fdsafdsafdsafdsa %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnknownLibrary { name, .. }
                    if name == "fdsafdsafdsafdsa"
            ),
            "Expected UnknownLibrary for 'fdsafdsafdsafdsa', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn selective_import_known_library_valid() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_library_errors(&db, "{% load trans from i18n %}");

        assert!(
            errors.is_empty(),
            "Selective import from known library should not produce error, got: {errors:?}"
        );
    }

    #[test]
    fn selective_import_unknown_library_produces_s120() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_library_errors(&db, "{% load foo from nonexistent %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnknownLibrary { name, .. }
                    if name == "nonexistent"
            ),
            "Expected UnknownLibrary for 'nonexistent', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn inspector_unavailable_no_library_diagnostics() {
        let db = TestDatabase::new();
        let errors = collect_library_errors(&db, "{% load nonexistent %}");

        assert!(
            errors.is_empty(),
            "No library diagnostics when inspector unavailable, got: {errors:?}"
        );
    }

    #[test]
    fn multi_library_load_each_validated() {
        let db = TestDatabase::with_inventory(test_inventory());
        // i18n is known, nonexistent is not
        let errors = collect_library_errors(&db, "{% load i18n nonexistent %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnknownLibrary { name, .. }
                    if name == "nonexistent"
            ),
            "Expected UnknownLibrary for 'nonexistent' in multi-load, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn multi_library_load_both_unknown() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_library_errors(&db, "{% load foo bar %}");

        assert_eq!(
            errors.len(),
            2,
            "Expected 2 UnknownLibrary errors, got: {errors:?}"
        );
        let names: Vec<&str> = errors
            .iter()
            .filter_map(|e| match e {
                ValidationError::UnknownLibrary { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    // ── Three-layer resolution tests (S118/S119) ───────────────

    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use djls_extraction::EnvironmentInventory;
    use djls_extraction::EnvironmentLibrary;

    fn make_env_inventory(libraries: Vec<EnvironmentLibrary>) -> EnvironmentInventory {
        let mut map: BTreeMap<String, Vec<EnvironmentLibrary>> = BTreeMap::new();
        for lib in libraries {
            map.entry(lib.load_name.clone()).or_default().push(lib);
        }
        EnvironmentInventory::new(map)
    }

    fn make_env_library(
        load_name: &str,
        app_module: &str,
        tags: &[&str],
        filters: &[&str],
    ) -> EnvironmentLibrary {
        EnvironmentLibrary {
            load_name: load_name.to_string(),
            app_module: app_module.to_string(),
            module_path: format!("{app_module}.templatetags.{load_name}"),
            source_path: PathBuf::from(format!("/fake/{load_name}.py")),
            tags: tags.iter().copied().map(str::to_string).collect(),
            filters: filters.iter().copied().map(str::to_string).collect(),
        }
    }

    fn collect_all_scoping_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
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
                        | ValidationError::TagNotInInstalledApps { .. }
                        | ValidationError::UnknownFilter { .. }
                        | ValidationError::UnloadedFilter { .. }
                        | ValidationError::AmbiguousUnloadedFilter { .. }
                        | ValidationError::FilterNotInInstalledApps { .. }
                )
            })
            .collect()
    }

    #[test]
    fn tag_in_env_but_not_installed_apps_produces_s118() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &["ordinal", "intword"],
            &[],
        )]);
        // Inspector has no humanize tags (not in INSTALLED_APPS)
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let errors = collect_all_scoping_errors(&db, "{% ordinal 42 %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::TagNotInInstalledApps { tag, app, load_name, .. }
                    if tag == "ordinal"
                        && app == "django.contrib.humanize"
                        && load_name == "humanize"
            ),
            "Expected TagNotInInstalledApps for 'ordinal', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn filter_in_env_but_not_installed_apps_produces_s119() {
        // Use a simple inventory without humanize — so intcomma is "unknown" to inspector
        let simple_tags = vec![builtin_tag_json("if", "django.template.defaulttags")];
        let simple_filters = vec![builtin_filter_json("title", "django.template.defaultfilters")];
        let simple_inventory = make_inventory_with_filters(
            &simple_tags,
            &simple_filters,
            &HashMap::new(),
            &[
                "django.template.defaulttags".to_string(),
                "django.template.defaultfilters".to_string(),
            ],
        );

        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &[],
            &["intcomma"],
        )]);
        let db = TestDatabase::with_inventories(simple_inventory, env);
        let errors = collect_all_scoping_errors(&db, "{{ value|intcomma }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::FilterNotInInstalledApps { filter, app, load_name, .. }
                    if filter == "intcomma"
                        && app == "django.contrib.humanize"
                        && load_name == "humanize"
            ),
            "Expected FilterNotInInstalledApps for 'intcomma', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn truly_unknown_tag_still_s108_with_env() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &["ordinal"],
            &[],
        )]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        // "xyz" is not in inspector AND not in environment
        let errors = collect_all_scoping_errors(&db, "{% xyz %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnknownTag { tag, .. } if tag == "xyz"),
            "Expected UnknownTag for 'xyz', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn truly_unknown_filter_still_s111_with_env() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &[],
            &["intcomma"],
        )]);
        let db = TestDatabase::with_inventories(test_inventory_with_filters(), env);
        let errors = collect_all_scoping_errors(&db, "{{ value|nonexistent }}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnknownFilter { filter, .. } if filter == "nonexistent"),
            "Expected UnknownFilter for 'nonexistent', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn env_unavailable_falls_through_to_s108() {
        // No environment inventory, but inspector is available
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_all_scoping_errors(&db, "{% xyz %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnknownTag { tag, .. } if tag == "xyz"),
            "Expected UnknownTag when env unavailable, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn tag_in_multiple_env_packages_produces_s118() {
        let env = make_env_inventory(vec![
            make_env_library("utils_a", "app_a", &["shared_tag"], &[]),
            make_env_library("utils_b", "app_b", &["shared_tag"], &[]),
        ]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let errors = collect_all_scoping_errors(&db, "{% shared_tag %}");

        // Should produce S118 (found in environment), not S108
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::TagNotInInstalledApps { tag, .. } if tag == "shared_tag"),
            "Expected TagNotInInstalledApps for 'shared_tag' with multiple env candidates, got: {:?}",
            errors[0]
        );
    }

    // Three-layer resolution for {% load %} libraries (S121)

    #[test]
    fn load_library_in_env_but_not_installed_apps_produces_s121() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &["ordinal"],
            &["intcomma"],
        )]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let errors = collect_library_errors(&db, "{% load humanize %}");

        assert_eq!(errors.len(), 1, "Expected 1 error, got: {errors:?}");
        assert!(
            matches!(
                &errors[0],
                ValidationError::LibraryNotInInstalledApps { name, app, .. }
                    if name == "humanize"
                        && app == "django.contrib.humanize"
            ),
            "Expected LibraryNotInInstalledApps for 'humanize', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn load_truly_unknown_library_still_s120_with_env() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &[],
            &[],
        )]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let errors = collect_library_errors(&db, "{% load totallyunknown %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnknownLibrary { name, .. }
                    if name == "totallyunknown"
            ),
            "Expected UnknownLibrary for 'totallyunknown', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn load_env_unavailable_falls_through_to_s120() {
        // Inspector available but no environment inventory
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_library_errors(&db, "{% load nonexistent %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnknownLibrary { name, .. }
                    if name == "nonexistent"
            ),
            "Expected UnknownLibrary when env unavailable, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn load_selective_import_env_library_produces_s121() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &["ordinal"],
            &["intcomma"],
        )]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let errors = collect_library_errors(&db, "{% load intcomma from humanize %}");

        assert_eq!(errors.len(), 1, "Expected 1 error, got: {errors:?}");
        assert!(
            matches!(
                &errors[0],
                ValidationError::LibraryNotInInstalledApps { name, app, .. }
                    if name == "humanize"
                        && app == "django.contrib.humanize"
            ),
            "Expected LibraryNotInInstalledApps for 'humanize', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn load_ambiguous_library_across_apps_produces_s121_with_candidates() {
        let env = make_env_inventory(vec![
            make_env_library("utils", "app_a", &[], &[]),
            make_env_library("utils", "app_b", &[], &[]),
        ]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let errors = collect_library_errors(&db, "{% load utils %}");

        assert_eq!(errors.len(), 1, "Expected 1 error, got: {errors:?}");
        match &errors[0] {
            ValidationError::LibraryNotInInstalledApps {
                name, candidates, ..
            } => {
                assert_eq!(name, "utils");
                assert_eq!(candidates.len(), 2, "Expected 2 candidates, got: {candidates:?}");
                assert!(candidates.contains(&"app_a".to_string()));
                assert!(candidates.contains(&"app_b".to_string()));
            }
            other => panic!("Expected LibraryNotInInstalledApps, got: {other:?}"),
        }
    }

    #[test]
    fn multi_load_mixed_env_and_unknown() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &[],
            &[],
        )]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        // i18n is known (in inspector), humanize is in env but not installed, xyz is truly unknown
        let errors = collect_library_errors(&db, "{% load i18n humanize xyz %}");

        assert_eq!(errors.len(), 2, "Expected 2 errors, got: {errors:?}");
        let has_s121 = errors.iter().any(|e| {
            matches!(
                e,
                ValidationError::LibraryNotInInstalledApps { name, .. }
                    if name == "humanize"
            )
        });
        let has_s120 = errors.iter().any(|e| {
            matches!(
                e,
                ValidationError::UnknownLibrary { name, .. }
                    if name == "xyz"
            )
        });
        assert!(has_s121, "Expected LibraryNotInInstalledApps for 'humanize'");
        assert!(has_s120, "Expected UnknownLibrary for 'xyz'");
    }

    // ── Integration test: all three layers in a single template ─

    fn collect_all_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
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
                        | ValidationError::TagNotInInstalledApps { .. }
                        | ValidationError::UnknownFilter { .. }
                        | ValidationError::UnloadedFilter { .. }
                        | ValidationError::AmbiguousUnloadedFilter { .. }
                        | ValidationError::FilterNotInInstalledApps { .. }
                        | ValidationError::UnknownLibrary { .. }
                        | ValidationError::LibraryNotInInstalledApps { .. }
                )
            })
            .collect()
    }

    fn three_layer_db() -> TestDatabase {
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            builtin_tag_json("csrf_token", "django.template.defaulttags"),
            builtin_tag_json("verbatim", "django.template.defaulttags"),
            builtin_tag_json("comment", "django.template.defaulttags"),
            library_tag_json("trans", "i18n", "django.templatetags.i18n"),
        ];
        let filters = vec![
            builtin_filter_json("title", "django.template.defaultfilters"),
            library_filter_json("lower_i18n", "i18n", "django.templatetags.i18n"),
        ];
        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];
        let inspector = make_inventory_with_filters(&tags, &filters, &libraries, &builtins);

        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &["ordinal"],
            &["intcomma"],
        )]);

        TestDatabase::with_inventories(inspector, env)
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn three_layer_integration_all_diagnostic_codes() {
        let db = three_layer_db();

        let source = "\
{% load i18n humanize nonexistent %}
{% csrf_token %}
{{ value|title }}
{% trans 'hello' %}
{{ value|lower_i18n }}
{% ordinal 42 %}
{{ value|intcomma }}
{% floobarblatz %}
{{ value|zzfilter }}";

        let errors = collect_all_errors(&db, source);

        // S120: truly unknown library
        assert!(
            errors.iter().any(|e| matches!(
                e, ValidationError::UnknownLibrary { name, .. } if name == "nonexistent"
            )),
            "Expected S120 UnknownLibrary for 'nonexistent'. All errors: {errors:#?}"
        );

        // S121: library in env but not `INSTALLED_APPS`
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::LibraryNotInInstalledApps { name, app, .. }
                    if name == "humanize" && app == "django.contrib.humanize"
            )),
            "Expected S121 LibraryNotInInstalledApps for 'humanize'. All errors: {errors:#?}"
        );

        // S118: tag in env but not `INSTALLED_APPS`
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::TagNotInInstalledApps { tag, app, load_name, .. }
                    if tag == "ordinal"
                        && app == "django.contrib.humanize"
                        && load_name == "humanize"
            )),
            "Expected S118 TagNotInInstalledApps for 'ordinal'. All errors: {errors:#?}"
        );

        // S119: filter in env but not `INSTALLED_APPS`
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::FilterNotInInstalledApps { filter, app, load_name, .. }
                    if filter == "intcomma"
                        && app == "django.contrib.humanize"
                        && load_name == "humanize"
            )),
            "Expected S119 FilterNotInInstalledApps for 'intcomma'. All errors: {errors:#?}"
        );

        // S108: truly unknown tag
        assert!(
            errors.iter().any(|e| matches!(
                e, ValidationError::UnknownTag { tag, .. } if tag == "floobarblatz"
            )),
            "Expected S108 UnknownTag for 'floobarblatz'. All errors: {errors:#?}"
        );

        // S111: truly unknown filter
        assert!(
            errors.iter().any(|e| matches!(
                e, ValidationError::UnknownFilter { filter, .. } if filter == "zzfilter"
            )),
            "Expected S111 UnknownFilter for 'zzfilter'. All errors: {errors:#?}"
        );

        // No false positives for valid builtins and loaded library items
        let unexpected = errors.iter().find(|e| match e {
            ValidationError::UnknownTag { tag, .. } => tag == "csrf_token" || tag == "if",
            ValidationError::UnloadedTag { tag, .. } => tag == "trans",
            ValidationError::UnknownFilter { filter, .. } => {
                filter == "title" || filter == "lower_i18n"
            }
            ValidationError::UnloadedFilter { filter, .. } => filter == "lower_i18n",
            _ => false,
        });
        assert!(
            unexpected.is_none(),
            "Unexpected error for valid builtin/loaded items: {unexpected:?}"
        );

        // Total: S108 + S111 + S118 + S119 + S120 + S121 = 6
        assert_eq!(
            errors.len(),
            6,
            "Expected exactly 6 errors. Got {}: {errors:#?}",
            errors.len()
        );
    }

    /// Integration test: S109/S112 layer — tags/filters in `INSTALLED_APPS` but
    /// not loaded at the usage position.
    #[test]
    fn three_layer_integration_unloaded_library_tags_filters() {
        let inspector = test_inventory_with_filters();

        // No environment inventory — keeps test focused on the unloaded layer
        let db = TestDatabase::with_inventory(inspector);

        // i18n is in inspector (INSTALLED_APPS) but NOT loaded
        // humanize is in inspector (INSTALLED_APPS) but NOT loaded
        let source = "\
{% trans 'hello' %}
{{ value|apnumber }}
{% load i18n %}
{% trans 'now loaded' %}";

        let errors = collect_all_errors(&db, source);

        // trans before load → S109
        let s109 = errors.iter().find(|e| {
            matches!(
                e,
                ValidationError::UnloadedTag { tag, library, .. }
                    if tag == "trans" && library == "i18n"
            )
        });
        assert!(
            s109.is_some(),
            "Expected S109 for 'trans' before load. All errors: {errors:#?}"
        );

        // apnumber before load → S112
        let s112 = errors.iter().find(|e| {
            matches!(
                e,
                ValidationError::UnloadedFilter { filter, library, .. }
                    if filter == "apnumber" && library == "humanize"
            )
        });
        assert!(
            s112.is_some(),
            "Expected S112 for 'apnumber' before load. All errors: {errors:#?}"
        );

        // trans after load → valid (no error for second trans)
        let trans_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::UnloadedTag { tag, .. } if tag == "trans"))
            .collect();
        assert_eq!(
            trans_errors.len(),
            1,
            "Only the first trans (before load) should error. Got: {trans_errors:#?}"
        );

        assert_eq!(
            errors.len(),
            2,
            "Expected exactly 2 errors (S109 + S112). Got {} errors: {errors:#?}",
            errors.len()
        );
    }
}
