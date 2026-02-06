//! Filter arity validation using M4 scoping + M5 extraction.
//!
//! Checks that filters are called with the correct number of arguments:
//! - S115: Filter requires an argument but none provided
//! - S116: Filter does not accept an argument but one was provided

use djls_extraction::FilterArity;
use djls_extraction::SymbolKey;
use djls_project::FilterProvenance;
use djls_templates::Node;
use salsa::Accumulator;

use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::load_resolution::compute_loaded_libraries;
use crate::load_resolution::LoadState;
use crate::load_resolution::LoadedLibraries;
use crate::opaque::compute_opaque_regions;
use crate::Db;

/// Build `LoadState` for symbols available at a given position.
///
/// Processes load statements in document order up to `position`,
/// using the same state-machine approach as M3/M4.
fn build_load_state_at(loaded: &LoadedLibraries, position: u32) -> LoadState {
    let mut state = LoadState::default();
    for stmt in loaded.loads() {
        if stmt.span.end() <= position {
            state.process(stmt);
        }
    }
    state
}

/// Resolve which filter implementation is in scope at a given position.
///
/// Returns a `SymbolKey` if the filter can be unambiguously resolved, `None` otherwise.
///
/// Resolution rules:
/// - Builtin filters are always available as fallback
/// - Library filters require a loaded library or selective import
/// - If a library filter is loaded, it shadows any builtin of the same name
/// - If multiple library filters are loaded (ambiguous), returns `None`
/// - Among multiple builtins, the one from the last module in `builtins()` wins
fn resolve_filter_symbol(
    filter_name: &str,
    position: u32,
    loaded: &LoadedLibraries,
    db: &dyn Db,
) -> Option<SymbolKey> {
    let inventory = db.inspector_inventory()?;

    let state = build_load_state_at(loaded, position);

    let mut library_candidates: Vec<String> = Vec::new();
    let mut builtin_candidates: Vec<String> = Vec::new();

    for filter in inventory.iter_filters() {
        if filter.name() != filter_name {
            continue;
        }

        match filter.provenance() {
            FilterProvenance::Builtin { module } => {
                builtin_candidates.push(module.clone());
            }
            FilterProvenance::Library { load_name, module } => {
                if state.is_tag_available(filter_name, load_name) {
                    library_candidates.push(module.clone());
                }
            }
        }
    }

    // Library filters take precedence over builtins
    if library_candidates.is_empty() {
        // Fall back to builtins: later in builtins() wins (Django semantics)
        let builtin_module = builtin_candidates.into_iter().max_by_key(|m| {
            inventory
                .builtins()
                .iter()
                .position(|b| b == m)
                .unwrap_or(0)
        });

        builtin_module.map(|m| SymbolKey::filter(m, filter_name))
    } else if library_candidates.len() == 1 {
        Some(SymbolKey::filter(library_candidates.remove(0), filter_name))
    } else {
        // Ambiguous: multiple loaded libraries define this filter.
        // M4's S113 handles diagnostics; we emit no arity errors.
        None
    }
}

/// Validate filter arity for all filters in the nodelist.
///
/// Iterates over `Node::Variable` nodes, resolves each filter's symbol
/// using the load-state at its position, then checks the extracted arity.
///
/// Skips filters:
/// - Inside opaque regions (e.g., `{% verbatim %}`)
/// - That can't be resolved (unknown, ambiguous, no inventory)
/// - With no extracted arity information
/// - With `Optional` or `Unknown` arity (always valid)
pub fn validate_filter_arity(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    let arity_specs = db.filter_arity_specs();
    if arity_specs.is_empty() {
        return;
    }

    let opaque = compute_opaque_regions(db, nodelist);
    let loaded = compute_loaded_libraries(db, nodelist);

    for node in nodelist.nodelist(db) {
        if let Node::Variable {
            filters,
            span: var_span,
            ..
        } = node
        {
            if opaque.is_opaque(*var_span) {
                continue;
            }

            for filter in filters {
                if opaque.is_opaque(filter.span) {
                    continue;
                }

                let Some(symbol_key) =
                    resolve_filter_symbol(&filter.name, filter.span.start(), &loaded, db)
                else {
                    continue;
                };

                let Some(arity) = arity_specs.get(&symbol_key) else {
                    continue;
                };

                match arity {
                    FilterArity::None => {
                        if filter.arg.is_some() {
                            ValidationErrorAccumulator(ValidationError::FilterUnexpectedArgument {
                                filter: filter.name.clone(),
                                span: filter.span,
                            })
                            .accumulate(db);
                        }
                    }
                    FilterArity::Required => {
                        if filter.arg.is_none() {
                            ValidationErrorAccumulator(ValidationError::FilterMissingArgument {
                                filter: filter.name.clone(),
                                span: filter.span,
                            })
                            .accumulate(db);
                        }
                    }
                    FilterArity::Optional | FilterArity::Unknown => {}
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
    use djls_project::InspectorInventory;
    use djls_project::TemplateFilter;
    use djls_source::File;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::db::FilterAritySpecs;
    use crate::db::OpaqueTagMap;
    use crate::db::ValidationErrorAccumulator;
    use crate::errors::ValidationError;
    use crate::templatetags::django_builtin_specs;
    use crate::TagIndex;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        inventory: Option<InspectorInventory>,
        arity_specs: FilterAritySpecs,
        opaque_map: OpaqueTagMap,
    }

    impl TestDatabase {
        fn new(inventory: InspectorInventory, arity_specs: FilterAritySpecs) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: Some(inventory),
                arity_specs,
                opaque_map: OpaqueTagMap::default(),
            }
        }

        fn with_opaque(
            inventory: InspectorInventory,
            arity_specs: FilterAritySpecs,
            opaque_map: OpaqueTagMap,
        ) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: Some(inventory),
                arity_specs,
                opaque_map,
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

        fn inspector_inventory(&self) -> Option<InspectorInventory> {
            self.inventory.clone()
        }

        fn filter_arity_specs(&self) -> FilterAritySpecs {
            self.arity_specs.clone()
        }

        fn opaque_tag_map(&self) -> OpaqueTagMap {
            self.opaque_map.clone()
        }
    }

    /// Helper to build a `FilterAritySpecs` from a list of (module, name, arity) tuples.
    fn make_arity_specs(entries: &[(&str, &str, FilterArity)]) -> FilterAritySpecs {
        let mut specs = FxHashMap::default();
        for (module, name, arity) in entries {
            specs.insert(SymbolKey::filter(*module, *name), arity.clone());
        }
        FilterAritySpecs::new(specs)
    }

    /// Helper: parse template and collect only filter arity errors (S115/S116).
    fn parse_and_validate(db: &TestDatabase, content: &str) -> Vec<ValidationError> {
        use djls_source::Db as SourceDb;

        let path = Utf8Path::new("/test.html");
        db.fs
            .lock()
            .unwrap()
            .add_file(path.to_owned(), content.to_string());

        let file = db.create_file(path);
        let nodelist = djls_templates::parse_template(db, file).expect("template should parse");

        // Use validate_nodelist (tracked) and filter to only S115/S116 errors
        crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::FilterMissingArgument { .. }
                        | ValidationError::FilterUnexpectedArgument { .. }
                )
            })
            .collect()
    }

    // --- Basic arity tests ---

    #[test]
    fn missing_required_argument_produces_s115() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "truncatewords",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity = make_arity_specs(&[(
            "django.template.defaultfilters",
            "truncatewords",
            FilterArity::Required,
        )]);
        let db = TestDatabase::new(inventory, arity);

        let errors = parse_and_validate(&db, "{{ title|truncatewords }}");
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::FilterMissingArgument { filter, .. } if filter == "truncatewords"
        ));
    }

    #[test]
    fn unexpected_argument_produces_s116() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "title",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity =
            make_arity_specs(&[("django.template.defaultfilters", "title", FilterArity::None)]);
        let db = TestDatabase::new(inventory, arity);

        let errors = parse_and_validate(&db, r#"{{ name|title:"unused" }}"#);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::FilterUnexpectedArgument { filter, .. } if filter == "title"
        ));
    }

    #[test]
    fn optional_arity_allows_no_argument() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "default",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity = make_arity_specs(&[(
            "django.template.defaultfilters",
            "default",
            FilterArity::Optional,
        )]);
        let db = TestDatabase::new(inventory, arity);

        let errors = parse_and_validate(&db, "{{ value|default }}");
        assert!(errors.is_empty(), "Optional arity should allow no arg");
    }

    #[test]
    fn optional_arity_allows_argument() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "default",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity = make_arity_specs(&[(
            "django.template.defaultfilters",
            "default",
            FilterArity::Optional,
        )]);
        let db = TestDatabase::new(inventory, arity);

        let errors = parse_and_validate(&db, r#"{{ value|default:"N/A" }}"#);
        assert!(errors.is_empty(), "Optional arity should allow an arg");
    }

    #[test]
    fn unknown_arity_allows_both() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "myfilter",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity = make_arity_specs(&[(
            "django.template.defaultfilters",
            "myfilter",
            FilterArity::Unknown,
        )]);
        let db = TestDatabase::new(inventory, arity);

        assert!(
            parse_and_validate(&db, "{{ x|myfilter }}").is_empty(),
            "Unknown arity should allow no arg"
        );
        assert!(
            parse_and_validate(&db, r#"{{ x|myfilter:"arg" }}"#).is_empty(),
            "Unknown arity should allow an arg"
        );
    }

    #[test]
    fn required_arity_valid_with_argument() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "truncatewords",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity = make_arity_specs(&[(
            "django.template.defaultfilters",
            "truncatewords",
            FilterArity::Required,
        )]);
        let db = TestDatabase::new(inventory, arity);

        let errors = parse_and_validate(&db, r#"{{ title|truncatewords:"5" }}"#);
        assert!(errors.is_empty(), "Required arity should pass with an arg");
    }

    #[test]
    fn none_arity_valid_without_argument() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "title",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity =
            make_arity_specs(&[("django.template.defaultfilters", "title", FilterArity::None)]);
        let db = TestDatabase::new(inventory, arity);

        let errors = parse_and_validate(&db, "{{ name|title }}");
        assert!(errors.is_empty(), "None arity should pass without an arg");
    }

    // --- Scoping tests ---

    #[test]
    fn selective_import_resolves_correct_arity() {
        let inventory = InspectorInventory::new(
            HashMap::from([("l10n".to_string(), "django.templatetags.l10n".to_string())]),
            vec![],
            vec![],
            vec![TemplateFilter::new_library(
                "localize",
                "l10n",
                "django.templatetags.l10n",
                None,
            )],
        );
        let arity =
            make_arity_specs(&[("django.templatetags.l10n", "localize", FilterArity::None)]);
        let db = TestDatabase::new(inventory, arity);

        let errors = parse_and_validate(
            &db,
            r#"{% load localize from l10n %}{{ price|localize:"unexpected" }}"#,
        );
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::FilterUnexpectedArgument { filter, .. } if filter == "localize"
        ));
    }

    #[test]
    fn ambiguous_library_filters_no_arity_error() {
        let inventory = InspectorInventory::new(
            HashMap::from([
                ("lib_a".to_string(), "myapp.templatetags.lib_a".to_string()),
                ("lib_b".to_string(), "myapp.templatetags.lib_b".to_string()),
            ]),
            vec![],
            vec![],
            vec![
                TemplateFilter::new_library("myfilter", "lib_a", "myapp.templatetags.lib_a", None),
                TemplateFilter::new_library("myfilter", "lib_b", "myapp.templatetags.lib_b", None),
            ],
        );
        let arity = make_arity_specs(&[
            (
                "myapp.templatetags.lib_a",
                "myfilter",
                FilterArity::Required,
            ),
            ("myapp.templatetags.lib_b", "myfilter", FilterArity::None),
        ]);
        let db = TestDatabase::new(inventory, arity);

        // Both libraries loaded → ambiguous → no arity error
        let errors =
            parse_and_validate(&db, "{% load lib_a %}{% load lib_b %}{{ value|myfilter }}");
        assert!(
            errors.is_empty(),
            "Ambiguous filters should not produce arity errors: {errors:?}"
        );
    }

    #[test]
    fn library_filter_shadows_builtin_arity() {
        let inventory = InspectorInventory::new(
            HashMap::from([(
                "custom_lib".to_string(),
                "myapp.templatetags.custom_lib".to_string(),
            )]),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![
                // Builtin "upper" with None arity
                TemplateFilter::new_builtin("upper", "django.template.defaultfilters", None),
                // Library "upper" with Required arity
                TemplateFilter::new_library(
                    "upper",
                    "custom_lib",
                    "myapp.templatetags.custom_lib",
                    None,
                ),
            ],
        );
        let arity = make_arity_specs(&[
            ("django.template.defaultfilters", "upper", FilterArity::None),
            (
                "myapp.templatetags.custom_lib",
                "upper",
                FilterArity::Required,
            ),
        ]);
        let db = TestDatabase::new(inventory, arity);

        // After loading custom_lib, the library's arity (Required) should apply
        let errors = parse_and_validate(&db, "{% load custom_lib %}{{ name|upper }}");
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::FilterMissingArgument { filter, .. } if filter == "upper"
        ));
    }

    #[test]
    fn builtin_tiebreak_later_module_wins() {
        // Two builtins define "escape" — later in builtins() wins
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["mod_a".to_string(), "mod_b".to_string()],
            vec![],
            vec![
                TemplateFilter::new_builtin("escape", "mod_a", None),
                TemplateFilter::new_builtin("escape", "mod_b", None),
            ],
        );
        let arity = make_arity_specs(&[
            ("mod_a", "escape", FilterArity::None),
            ("mod_b", "escape", FilterArity::Required),
        ]);
        let db = TestDatabase::new(inventory, arity);

        // mod_b is later in builtins(), so Required arity applies
        let errors = parse_and_validate(&db, "{{ html|escape }}");
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::FilterMissingArgument { filter, .. } if filter == "escape"
        ));
    }

    #[test]
    fn no_arity_errors_without_extraction() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "title",
                "django.template.defaultfilters",
                None,
            )],
        );
        // Empty arity specs — no extraction available
        let db = TestDatabase::new(inventory, FilterAritySpecs::default());

        let errors = parse_and_validate(&db, r#"{{ x|title:"unexpected" }}"#);
        assert!(
            errors.is_empty(),
            "No arity errors when extraction unavailable"
        );
    }

    // --- Opaque region tests ---

    #[test]
    fn no_arity_errors_inside_verbatim() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "truncatewords",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity = make_arity_specs(&[(
            "django.template.defaultfilters",
            "truncatewords",
            FilterArity::Required,
        )]);
        let mut opaque_map = OpaqueTagMap::default();
        opaque_map.insert("verbatim".to_string(), "endverbatim".to_string());
        let db = TestDatabase::with_opaque(inventory, arity, opaque_map);

        // The filter inside verbatim should be skipped
        let errors = parse_and_validate(
            &db,
            "{% verbatim %}{{ title|truncatewords }}{% endverbatim %}",
        );
        assert!(
            errors.is_empty(),
            "No arity errors inside opaque regions: {errors:?}"
        );
    }

    #[test]
    fn arity_errors_after_endverbatim() {
        let inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaultfilters".to_string()],
            vec![],
            vec![TemplateFilter::new_builtin(
                "truncatewords",
                "django.template.defaultfilters",
                None,
            )],
        );
        let arity = make_arity_specs(&[(
            "django.template.defaultfilters",
            "truncatewords",
            FilterArity::Required,
        )]);
        let mut opaque_map = OpaqueTagMap::default();
        opaque_map.insert("verbatim".to_string(), "endverbatim".to_string());
        let db = TestDatabase::with_opaque(inventory, arity, opaque_map);

        // Filter after endverbatim should still be validated
        let errors = parse_and_validate(
            &db,
            "{% verbatim %}safe{% endverbatim %}{{ title|truncatewords }}",
        );
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::FilterMissingArgument { filter, .. } if filter == "truncatewords"
        ));
    }
}
