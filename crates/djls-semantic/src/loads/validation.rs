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
    let template_libraries = db.template_libraries();
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

    let tag_specs = db.tag_specs();
    let loaded_libraries = compute_loaded_libraries(db, nodelist);

    let env_tags =
        template_libraries.scanned_symbol_candidates_by_name(djls_project::TemplateSymbolKind::Tag);

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
        let symbols =
            AvailableSymbols::at_position(&loaded_libraries, &template_libraries, span.start());

        match symbols.check(name) {
            TagAvailability::Available => {}
            TagAvailability::Unknown => {
                // Check environment inventory: is the tag installed but not in INSTALLED_APPS?
                if let Some(env_tags) = &env_tags {
                    if let Some(key) = djls_project::TemplateSymbolName::new(name.as_str()) {
                        if let Some(env_symbols) = env_tags.get(&key) {
                            let sym = &env_symbols[0];
                            ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                                tag: name.clone(),
                                app: sym.app_module.as_str().to_string(),
                                load_name: sym.library_name.as_str().to_string(),
                                span: marker_span,
                            })
                            .accumulate(db);
                            continue;
                        }
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
    let template_libraries = db.template_libraries();
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

    let loaded_libraries = compute_loaded_libraries(db, nodelist);

    let env_filters = template_libraries
        .scanned_symbol_candidates_by_name(djls_project::TemplateSymbolKind::Filter);

    for node in nodelist.nodelist(db) {
        let Node::Variable { filters, span, .. } = node else {
            continue;
        };

        if opaque_regions.is_opaque(span.start()) {
            continue;
        }

        let symbols =
            AvailableSymbols::at_position(&loaded_libraries, &template_libraries, span.start());

        for filter in filters {
            match symbols.check_filter(&filter.name) {
                FilterAvailability::Available => {}
                FilterAvailability::Unknown => {
                    // Check environment inventory: is the filter installed but not in INSTALLED_APPS?
                    if let Some(env_filters) = &env_filters {
                        if let Some(key) =
                            djls_project::TemplateSymbolName::new(filter.name.as_str())
                        {
                            if let Some(env_symbols) = env_filters.get(&key) {
                                let sym = &env_symbols[0];
                                ValidationErrorAccumulator(
                                    ValidationError::FilterNotInInstalledApps {
                                        filter: filter.name.clone(),
                                        app: sym.app_module.as_str().to_string(),
                                        load_name: sym.library_name.as_str().to_string(),
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
    let template_libraries = db.template_libraries();
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

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
            if template_libraries.is_enabled_library_str(lib_name) {
                continue;
            }

            let candidates = template_libraries.scanned_app_modules_for_library_str(lib_name);
            if !candidates.is_empty() {
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

    use camino::Utf8PathBuf;
    use djls_project::LibraryName;
    use djls_project::PyModuleName;
    use djls_project::ScannedTemplateLibraries;
    use djls_project::ScannedTemplateLibrary;
    use djls_project::ScannedTemplateSymbol;
    use djls_project::TemplateLibraries;
    use djls_project::TemplateSymbolKind;
    use djls_project::TemplateSymbolName;

    use crate::testing::builtin_filter_json;
    use crate::testing::builtin_tag_json;
    use crate::testing::library_filter_json;
    use crate::testing::library_tag_json;
    use crate::testing::make_template_libraries;
    use crate::testing::make_template_libraries_tags_only;
    use crate::testing::TestDatabase;
    use crate::ValidationError;

    fn test_inventory() -> TemplateLibraries {
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

        make_template_libraries_tags_only(&tags, &libraries, &builtins)
    }

    fn render_tag_scoping_snapshot(db: &TestDatabase, source: &str) -> String {
        crate::testing::render_validate_snapshot_filtered(db, "test.html", 0, source, |err| {
            matches!(
                err,
                ValidationError::UnknownTag { .. }
                    | ValidationError::UnloadedTag { .. }
                    | ValidationError::AmbiguousUnloadedTag { .. }
            )
        })
    }

    #[test]
    fn unknown_tag_produces_s108() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_tag_scoping_snapshot(&db, "{% xyz %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn unloaded_library_tag_produces_s109() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_tag_scoping_snapshot(&db, "{% trans 'hello' %}");
        insta::assert_snapshot!(rendered);
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
        let inventory = make_template_libraries_tags_only(&tags, &libraries, &[]);

        let db = TestDatabase::with_inventory(inventory);
        let rendered = render_tag_scoping_snapshot(&db, "{% shared %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn inspector_unavailable_no_scoping_diagnostics() {
        let db = TestDatabase::new();
        let rendered = render_tag_scoping_snapshot(&db, "{% xyz %}{% trans 'hello' %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn structural_tags_skip_scoping_checks() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered =
            render_tag_scoping_snapshot(&db, "{% if True %}{% elif False %}{% else %}{% endif %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn loaded_library_tag_no_error() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_tag_scoping_snapshot(&db, "{% load i18n %}\n{% trans 'hello' %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn tag_before_load_produces_error() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_tag_scoping_snapshot(&db, "{% trans 'hello' %}\n{% load i18n %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn selective_import_makes_only_imported_symbol_available() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source =
            "{% load trans from i18n %}\n{% trans 'hello' %}\n{% blocktrans %}{% endblocktrans %}";
        let rendered = render_tag_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn builtin_tag_always_available() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_tag_scoping_snapshot(&db, "{% csrf_token %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn load_tag_itself_not_flagged() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_tag_scoping_snapshot(&db, "{% load i18n %}");
        insta::assert_snapshot!(rendered);
    }

    // --- Filter scoping tests ---

    fn render_filter_scoping_snapshot(db: &TestDatabase, source: &str) -> String {
        crate::testing::render_validate_snapshot_filtered(db, "test.html", 0, source, |err| {
            matches!(
                err,
                ValidationError::UnknownFilter { .. }
                    | ValidationError::UnloadedFilter { .. }
                    | ValidationError::AmbiguousUnloadedFilter { .. }
            )
        })
    }

    fn test_inventory_with_filters() -> TemplateLibraries {
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

        make_template_libraries(&tags, &filters, &libraries, &builtins)
    }

    #[test]
    fn unknown_filter_produces_s111() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let rendered = render_filter_scoping_snapshot(&db, "{{ value|nonexistent }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn unloaded_library_filter_produces_s112() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let rendered = render_filter_scoping_snapshot(&db, "{{ value|apnumber }}");
        insta::assert_snapshot!(rendered);
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
        let inventory = make_template_libraries(&[], &filters, &libraries, &[]);

        let db = TestDatabase::with_inventory(inventory);
        let rendered = render_filter_scoping_snapshot(&db, "{{ value|myfilter }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_after_load_is_valid() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let rendered =
            render_filter_scoping_snapshot(&db, "{% load humanize %}\n{{ value|apnumber }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn builtin_filter_always_valid() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let rendered = render_filter_scoping_snapshot(&db, "{{ value|title }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn inspector_unavailable_no_filter_diagnostics() {
        let db = TestDatabase::new();
        let rendered =
            render_filter_scoping_snapshot(&db, "{{ value|nonexistent }}{{ x|apnumber }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_chain_validates_each_filter() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let rendered = render_filter_scoping_snapshot(&db, "{{ value|title|apnumber }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn selective_import_filter() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let source =
            "{% load apnumber from humanize %}\n{{ value|apnumber }}\n{{ value|intcomma }}";
        let rendered = render_filter_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }

    // Opaque region tests

    #[test]
    fn verbatim_block_content_skipped() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source = "{% verbatim %}{% trans 'hello' %}{% endverbatim %}\n{% if True %}{% endif %}";
        let rendered = render_tag_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn comment_block_content_skipped() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source = "{% comment %}{% trans 'hello' %}{% endcomment %}";
        let rendered = render_tag_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn non_opaque_blocks_validated_normally() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source = "{% if True %}{% trans 'hello' %}{% endif %}";
        let rendered = render_tag_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn content_after_opaque_block_still_validated() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source = "{% verbatim %}{% trans 'skip' %}{% endverbatim %}{% trans 'check' %}";
        let rendered = render_tag_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_inside_verbatim_skipped() {
        let db = TestDatabase::with_inventory(test_inventory_with_filters());
        let source = "{% verbatim %}{{ value|intcomma }}{% endverbatim %}";
        let rendered = render_filter_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }

    // Library name validation tests (S120)

    fn render_library_snapshot(db: &TestDatabase, source: &str) -> String {
        crate::testing::render_validate_snapshot_filtered(db, "test.html", 0, source, |err| {
            matches!(
                err,
                ValidationError::UnknownLibrary { .. } | ValidationError::LibraryNotInInstalledApps { .. }
            )
        })
    }

    #[test]
    fn known_library_valid() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_library_snapshot(&db, "{% load i18n %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn unknown_library_produces_s120() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_library_snapshot(&db, "{% load fdsafdsafdsafdsa %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn selective_import_known_library_valid() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_library_snapshot(&db, "{% load trans from i18n %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn selective_import_unknown_library_produces_s120() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_library_snapshot(&db, "{% load foo from nonexistent %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn inspector_unavailable_no_library_diagnostics() {
        let db = TestDatabase::new();
        let rendered = render_library_snapshot(&db, "{% load nonexistent %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn multi_library_load_each_validated() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_library_snapshot(&db, "{% load i18n nonexistent %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn multi_library_load_both_unknown() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_library_snapshot(&db, "{% load foo bar %}");
        insta::assert_snapshot!(rendered);
    }

    // Three-layer resolution tests (S118/S119)

    use std::collections::BTreeMap;

    fn make_env_inventory(libraries: Vec<ScannedTemplateLibrary>) -> ScannedTemplateLibraries {
        let mut map: BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>> = BTreeMap::new();
        for lib in libraries {
            map.entry(lib.name.clone()).or_default().push(lib);
        }
        ScannedTemplateLibraries::new(map)
    }

    fn make_env_library(
        load_name: &str,
        app_module: &str,
        tags: &[&str],
        filters: &[&str],
    ) -> ScannedTemplateLibrary {
        let name = LibraryName::new(load_name).unwrap();
        let app_module = PyModuleName::new(app_module).unwrap();
        let module = PyModuleName::new(&format!("{app_module}.templatetags.{load_name}")).unwrap();

        let mut symbols = Vec::new();

        for tag in tags {
            symbols.push(ScannedTemplateSymbol {
                kind: TemplateSymbolKind::Tag,
                name: TemplateSymbolName::new(tag).unwrap(),
            });
        }

        for filter in filters {
            symbols.push(ScannedTemplateSymbol {
                kind: TemplateSymbolKind::Filter,
                name: TemplateSymbolName::new(filter).unwrap(),
            });
        }

        ScannedTemplateLibrary {
            name,
            app_module,
            module,
            source_path: Utf8PathBuf::from(format!("/fake/{load_name}.py")),
            symbols,
        }
    }

    fn render_scoping_snapshot(db: &TestDatabase, source: &str) -> String {
        crate::testing::render_validate_snapshot_filtered(db, "test.html", 0, source, |err| {
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
    }

    #[test]
    fn tag_in_env_but_not_installed_apps_produces_s118() {
        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &["ordinal", "intword"],
            &[],
        )]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let rendered = render_scoping_snapshot(&db, "{% ordinal 42 %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn filter_in_env_but_not_installed_apps_produces_s119() {
        // Use a simple inventory without humanize — so intcomma is "unknown" to inspector
        let simple_tags = vec![builtin_tag_json("if", "django.template.defaulttags")];
        let simple_filters = vec![builtin_filter_json(
            "title",
            "django.template.defaultfilters",
        )];
        let simple_inventory = make_template_libraries(
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
        let rendered = render_scoping_snapshot(&db, "{{ value|intcomma }}");
        insta::assert_snapshot!(rendered);
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
        let rendered = render_scoping_snapshot(&db, "{% xyz %}");
        insta::assert_snapshot!(rendered);
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
        let rendered = render_scoping_snapshot(&db, "{{ value|nonexistent }}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn env_unavailable_falls_through_to_s108() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_scoping_snapshot(&db, "{% xyz %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn tag_in_multiple_env_packages_produces_s118() {
        let env = make_env_inventory(vec![
            make_env_library("utils_a", "app_a", &["shared_tag"], &[]),
            make_env_library("utils_b", "app_b", &["shared_tag"], &[]),
        ]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let rendered = render_scoping_snapshot(&db, "{% shared_tag %}");
        insta::assert_snapshot!(rendered);
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
        let rendered = render_library_snapshot(&db, "{% load humanize %}");
        insta::assert_snapshot!(rendered);
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
        let rendered = render_library_snapshot(&db, "{% load totallyunknown %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn load_env_unavailable_falls_through_to_s120() {
        let db = TestDatabase::with_inventory(test_inventory());
        let rendered = render_library_snapshot(&db, "{% load nonexistent %}");
        insta::assert_snapshot!(rendered);
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
        let rendered = render_library_snapshot(&db, "{% load intcomma from humanize %}");
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn load_ambiguous_library_across_apps_produces_s121_with_candidates() {
        let env = make_env_inventory(vec![
            make_env_library("utils", "app_a", &[], &[]),
            make_env_library("utils", "app_b", &[], &[]),
        ]);
        let db = TestDatabase::with_inventories(test_inventory(), env);
        let rendered = render_library_snapshot(&db, "{% load utils %}");
        insta::assert_snapshot!(rendered);
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
        let rendered = render_library_snapshot(&db, "{% load i18n humanize xyz %}");
        insta::assert_snapshot!(rendered);
    }

    // Integration test: all three layers in a single template

    fn render_all_scoping_snapshot(db: &TestDatabase, source: &str) -> String {
        crate::testing::render_validate_snapshot_filtered(db, "test.html", 0, source, |err| {
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
        let inspector = make_template_libraries(&tags, &filters, &libraries, &builtins);

        let env = make_env_inventory(vec![make_env_library(
            "humanize",
            "django.contrib.humanize",
            &["ordinal"],
            &["intcomma"],
        )]);

        TestDatabase::with_inventories(inspector, env)
    }

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

        let rendered = render_all_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
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

        let rendered = render_all_scoping_snapshot(&db, source);
        insta::assert_snapshot!(rendered);
    }
}
