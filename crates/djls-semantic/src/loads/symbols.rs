use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_project::TagProvenance;
use djls_project::TemplateSymbol;
use djls_project::TemplateTags;

use super::load::LoadState;
use super::LoadedLibraries;

/// The result of checking a tag name against the available symbols at a position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagAvailability {
    /// The tag is available (builtin or from a loaded library).
    Available,
    /// The tag is known but its library is not loaded. Contains exactly one
    /// candidate library name.
    Unloaded { library: String },
    /// The tag is known but defined in multiple unloaded libraries. Contains
    /// all candidate library names, sorted alphabetically.
    AmbiguousUnloaded { libraries: Vec<String> },
    /// The tag is completely unknown — not in builtins and not in the inspector
    /// inventory.
    Unknown,
}

/// The result of checking a filter name against the available symbols at a position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterAvailability {
    /// The filter is available (builtin or from a loaded library).
    Available,
    /// The filter is known but its library is not loaded. Contains exactly one
    /// candidate library name.
    Unloaded { library: String },
    /// The filter is known but defined in multiple unloaded libraries. Contains
    /// all candidate library names, sorted alphabetically.
    AmbiguousUnloaded { libraries: Vec<String> },
    /// The filter is completely unknown — not in builtins and not in the inspector
    /// inventory.
    Unknown,
}

/// The set of tags and filters available at a given position in a template,
/// plus a mapping of unavailable-but-known symbols to their required library/libraries.
///
/// Constructed from `LoadedLibraries`, inspector inventory (`TemplateTags`),
/// and a byte position in the template.
#[derive(Clone, Debug)]
pub struct AvailableSymbols {
    /// Tag names that are available at this position (builtins + loaded library tags).
    available: BTreeSet<String>,
    /// Tag names → set of candidate libraries. Only populated for tags NOT in `available`.
    candidates: BTreeMap<String, BTreeSet<String>>,
    /// Filter names that are available at this position (builtins + loaded library filters).
    available_filters: BTreeSet<String>,
    /// Filter names → set of candidate libraries. Only populated for filters NOT in `available_filters`.
    filter_candidates: BTreeMap<String, BTreeSet<String>>,
    /// The load state at this position, retained for library availability queries.
    load_state: LoadState,
}

impl AvailableSymbols {
    /// Build available symbols for a given position using load state and inspector inventory.
    #[must_use]
    pub fn at_position(
        loaded_libraries: &LoadedLibraries,
        inventory: &TemplateTags,
        position: u32,
    ) -> Self {
        let load_state = loaded_libraries.available_at(position);
        Self::from_load_state(&load_state, inventory)
    }

    /// Build available symbols from a pre-computed `LoadState` and inspector inventory.
    #[must_use]
    pub fn from_load_state(load_state: &LoadState, inventory: &TemplateTags) -> Self {
        let mut available = BTreeSet::new();
        let mut candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        // Build a reverse index: tag_name → set of candidate library load_names
        // Also collect builtins directly into available.
        for tag in inventory.iter() {
            match tag.provenance() {
                TagProvenance::Builtin { .. } => {
                    available.insert(tag.name().to_string());
                }
                TagProvenance::Library { load_name, .. } => {
                    candidates
                        .entry(tag.name().to_string())
                        .or_default()
                        .insert(load_name.clone());
                }
            }
        }

        // Now move library tags that are available (loaded) from candidates → available
        let loaded_names: Vec<String> = candidates.keys().cloned().collect();
        for tag_name in loaded_names {
            let Some(libs) = candidates.get(&tag_name) else {
                continue;
            };
            let is_available = libs.iter().any(|lib| {
                load_state.is_fully_loaded(lib) || load_state.is_symbol_available(lib, &tag_name)
            });
            if is_available {
                available.insert(tag_name.clone());
                candidates.remove(&tag_name);
            }
        }

        // Build filter availability using the same pattern
        let mut available_filters = BTreeSet::new();
        let mut filter_candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        for filter in inventory.filters() {
            match filter.provenance() {
                TagProvenance::Builtin { .. } => {
                    available_filters.insert(filter.name().to_string());
                }
                TagProvenance::Library { load_name, .. } => {
                    filter_candidates
                        .entry(filter.name().to_string())
                        .or_default()
                        .insert(load_name.clone());
                }
            }
        }

        // Move loaded library filters from filter_candidates → available_filters
        let loaded_filter_names: Vec<String> = filter_candidates.keys().cloned().collect();
        for filter_name in loaded_filter_names {
            let Some(libs) = filter_candidates.get(&filter_name) else {
                continue;
            };
            let is_available = libs.iter().any(|lib| {
                load_state.is_fully_loaded(lib) || load_state.is_symbol_available(lib, &filter_name)
            });
            if is_available {
                available_filters.insert(filter_name.clone());
                filter_candidates.remove(&filter_name);
            }
        }

        Self {
            available,
            candidates,
            available_filters,
            filter_candidates,
            load_state: load_state.clone(),
        }
    }

    /// Check whether a tag name is available at this position.
    #[must_use]
    pub fn check(&self, tag_name: &str) -> TagAvailability {
        if self.available.contains(tag_name) {
            return TagAvailability::Available;
        }

        if let Some(libs) = self.candidates.get(tag_name) {
            // BTreeSet iterates in sorted order, so no explicit sort needed
            let libs: Vec<String> = libs.iter().cloned().collect();
            return match libs.as_slice() {
                [] => TagAvailability::Unknown,
                [single] => TagAvailability::Unloaded {
                    library: single.clone(),
                },
                _ => TagAvailability::AmbiguousUnloaded { libraries: libs },
            };
        }

        TagAvailability::Unknown
    }

    /// Check whether a filter name is available at this position.
    #[must_use]
    pub fn check_filter(&self, filter_name: &str) -> FilterAvailability {
        if self.available_filters.contains(filter_name) {
            return FilterAvailability::Available;
        }

        if let Some(libs) = self.filter_candidates.get(filter_name) {
            // BTreeSet iterates in sorted order, so no explicit sort needed
            let libs: Vec<String> = libs.iter().cloned().collect();
            return match libs.as_slice() {
                [] => FilterAvailability::Unknown,
                [single] => FilterAvailability::Unloaded {
                    library: single.clone(),
                },
                _ => FilterAvailability::AmbiguousUnloaded { libraries: libs },
            };
        }

        FilterAvailability::Unknown
    }

    /// Returns the set of available tag names.
    #[must_use]
    pub fn available_tags(&self) -> &BTreeSet<String> {
        &self.available
    }

    /// Returns the set of available filter names.
    #[must_use]
    pub fn available_filters(&self) -> &BTreeSet<String> {
        &self.available_filters
    }

    /// Returns the mapping of unavailable tag names to their candidate libraries.
    #[must_use]
    pub fn unavailable_candidates(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.candidates
    }

    /// Check whether a library is fully loaded at this position.
    ///
    /// Used by filter completions to determine if a library's filters should
    /// be shown. A library is considered loaded if it appears in a
    /// `{% load lib %}` statement before the cursor position.
    #[must_use]
    pub fn is_library_loaded(&self, library: &str) -> bool {
        self.load_state.is_fully_loaded(library)
    }

    /// Check whether a specific symbol from a library is available at this position.
    ///
    /// Returns true if the library is fully loaded, or if the symbol was
    /// selectively imported via `{% load sym from lib %}`.
    #[must_use]
    pub fn is_symbol_imported(&self, library: &str, symbol: &str) -> bool {
        self.load_state.is_symbol_available(library, symbol)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use djls_source::Span;

    use super::super::LoadKind;
    use super::super::LoadStatement;
    use super::*;

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

    fn make_load(span: (u32, u32), kind: LoadKind) -> LoadStatement {
        LoadStatement::new(Span::new(span.0, span.1), kind)
    }

    fn test_inventory() -> TemplateTags {
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            builtin_tag_json("for", "django.template.defaulttags"),
            builtin_tag_json("block", "django.template.loader_tags"),
            library_tag_json("trans", "i18n", "django.templatetags.i18n"),
            library_tag_json("blocktrans", "i18n", "django.templatetags.i18n"),
            library_tag_json("static", "static", "django.templatetags.static"),
            library_tag_json("get_static_prefix", "static", "django.templatetags.static"),
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

    #[test]
    fn builtins_always_available() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 0);

        assert_eq!(symbols.check("if"), TagAvailability::Available);
        assert_eq!(symbols.check("for"), TagAvailability::Available);
        assert_eq!(symbols.check("block"), TagAvailability::Available);
    }

    #[test]
    fn library_tag_before_load_is_unloaded() {
        let inventory = test_inventory();
        // Load i18n at position 50..70
        let loaded = LoadedLibraries::new(vec![make_load(
            (50, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // At position 10 (before load), trans should be unloaded
        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 10);
        assert_eq!(
            symbols.check("trans"),
            TagAvailability::Unloaded {
                library: "i18n".into()
            }
        );
    }

    #[test]
    fn library_tag_after_load_is_available() {
        let inventory = test_inventory();
        // Load i18n at position 50..70
        let loaded = LoadedLibraries::new(vec![make_load(
            (50, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // At position 100 (after load), trans should be available
        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);
        assert_eq!(symbols.check("trans"), TagAvailability::Available);
        assert_eq!(symbols.check("blocktrans"), TagAvailability::Available);
    }

    #[test]
    fn selective_import_only_makes_imported_symbols_available() {
        let inventory = test_inventory();
        // Selectively import trans from i18n
        let loaded = LoadedLibraries::new(vec![make_load(
            (10, 30),
            LoadKind::SelectiveImport {
                symbols: vec!["trans".into()],
                library: "i18n".into(),
            },
        )]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        // trans is selectively imported → available
        assert_eq!(symbols.check("trans"), TagAvailability::Available);
        // blocktrans is NOT imported → unloaded
        assert_eq!(
            symbols.check("blocktrans"),
            TagAvailability::Unloaded {
                library: "i18n".into()
            }
        );
    }

    #[test]
    fn full_load_overrides_selective() {
        let inventory = test_inventory();
        // First selectively import trans, then fully load i18n
        let loaded = LoadedLibraries::new(vec![
            make_load(
                (10, 30),
                LoadKind::SelectiveImport {
                    symbols: vec!["trans".into()],
                    library: "i18n".into(),
                },
            ),
            make_load(
                (50, 20),
                LoadKind::FullLoad {
                    libraries: vec!["i18n".into()],
                },
            ),
        ]);

        // After full load, both should be available
        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);
        assert_eq!(symbols.check("trans"), TagAvailability::Available);
        assert_eq!(symbols.check("blocktrans"), TagAvailability::Available);
    }

    #[test]
    fn unknown_tag_is_unknown() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        assert_eq!(symbols.check("nonexistent_tag"), TagAvailability::Unknown);
        assert_eq!(symbols.check("foobar"), TagAvailability::Unknown);
    }

    #[test]
    fn tag_in_multiple_libraries_produces_ambiguous() {
        // Create inventory with a tag defined in two libraries
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            library_tag_json("mytag", "lib_a", "app.templatetags.lib_a"),
            library_tag_json("mytag", "lib_b", "app.templatetags.lib_b"),
        ];

        let mut libraries = HashMap::new();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_inventory(&tags, &libraries, &[]);
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check("mytag"),
            TagAvailability::AmbiguousUnloaded {
                libraries: vec!["lib_a".into(), "lib_b".into()]
            }
        );
    }

    #[test]
    fn ambiguous_resolved_by_loading_one_library() {
        let tags = vec![
            library_tag_json("mytag", "lib_a", "app.templatetags.lib_a"),
            library_tag_json("mytag", "lib_b", "app.templatetags.lib_b"),
        ];

        let mut libraries = HashMap::new();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_inventory(&tags, &libraries, &[]);

        // Load lib_a
        let loaded = LoadedLibraries::new(vec![make_load(
            (10, 20),
            LoadKind::FullLoad {
                libraries: vec!["lib_a".into()],
            },
        )]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        // Even though mytag is in lib_b too, loading lib_a makes it available
        assert_eq!(symbols.check("mytag"), TagAvailability::Available);
    }

    #[test]
    fn multiple_loads_cumulative() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![
            make_load(
                (10, 20),
                LoadKind::FullLoad {
                    libraries: vec!["i18n".into()],
                },
            ),
            make_load(
                (40, 20),
                LoadKind::FullLoad {
                    libraries: vec!["static".into()],
                },
            ),
        ]);

        // After both loads
        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        assert_eq!(symbols.check("trans"), TagAvailability::Available);
        assert_eq!(symbols.check("static"), TagAvailability::Available);
        assert_eq!(
            symbols.check("get_static_prefix"),
            TagAvailability::Available
        );
    }

    #[test]
    fn between_loads_partial_availability() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![
            make_load(
                (10, 20),
                LoadKind::FullLoad {
                    libraries: vec!["i18n".into()],
                },
            ),
            make_load(
                (80, 20),
                LoadKind::FullLoad {
                    libraries: vec!["static".into()],
                },
            ),
        ]);

        // After first load but before second
        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 50);

        assert_eq!(symbols.check("trans"), TagAvailability::Available);
        assert_eq!(
            symbols.check("static"),
            TagAvailability::Unloaded {
                library: "static".into()
            }
        );
    }

    #[test]
    fn available_tags_returns_correct_set() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![make_load(
            (10, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        let available = symbols.available_tags();
        assert!(available.contains("if"));
        assert!(available.contains("for"));
        assert!(available.contains("block"));
        assert!(available.contains("trans"));
        assert!(available.contains("blocktrans"));
        // static tags are NOT loaded
        assert!(!available.contains("static"));
        assert!(!available.contains("get_static_prefix"));
    }

    #[test]
    fn unavailable_candidates_correct() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        let candidates = symbols.unavailable_candidates();
        // All library tags should be in candidates since nothing is loaded
        assert!(candidates.contains_key("trans"));
        assert!(candidates.contains_key("blocktrans"));
        assert!(candidates.contains_key("static"));
        assert!(candidates.contains_key("get_static_prefix"));

        // Each should map to their library
        assert!(candidates["trans"].contains("i18n"));
        assert!(candidates["static"].contains("static"));
    }

    #[test]
    fn empty_inventory_everything_unknown() {
        let inventory = make_inventory(&[], &HashMap::new(), &[]);
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        assert_eq!(symbols.check("anything"), TagAvailability::Unknown);
        assert!(symbols.available_tags().is_empty());
        assert!(symbols.unavailable_candidates().is_empty());
    }

    #[test]
    fn selective_import_with_ambiguous_tag() {
        // Tag "shared" exists in both lib_a and lib_b
        let tags = vec![
            library_tag_json("shared", "lib_a", "app.templatetags.lib_a"),
            library_tag_json("shared", "lib_b", "app.templatetags.lib_b"),
        ];

        let mut libraries = HashMap::new();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_inventory(&tags, &libraries, &[]);

        // Selectively import "shared" from lib_a
        let loaded = LoadedLibraries::new(vec![make_load(
            (10, 30),
            LoadKind::SelectiveImport {
                symbols: vec!["shared".into()],
                library: "lib_a".into(),
            },
        )]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        // Should be available because we imported it from lib_a
        assert_eq!(symbols.check("shared"), TagAvailability::Available);
    }

    // --- Filter availability tests ---

    fn test_inventory_with_filters() -> TemplateTags {
        let tags = vec![builtin_tag_json("if", "django.template.defaulttags")];
        let filters = vec![
            builtin_filter_json("title", "django.template.defaultfilters"),
            builtin_filter_json("lower", "django.template.defaultfilters"),
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
        libraries.insert(
            "humanize".to_string(),
            "django.contrib.humanize.templatetags.humanize".to_string(),
        );

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];

        make_inventory_with_filters(&tags, &filters, &libraries, &builtins)
    }

    #[test]
    fn builtin_filter_always_available() {
        let inventory = test_inventory_with_filters();
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 0);

        assert_eq!(symbols.check_filter("title"), FilterAvailability::Available);
        assert_eq!(symbols.check_filter("lower"), FilterAvailability::Available);
    }

    #[test]
    fn library_filter_before_load_is_unloaded() {
        let inventory = test_inventory_with_filters();
        let loaded = LoadedLibraries::new(vec![make_load(
            (50, 20),
            LoadKind::FullLoad {
                libraries: vec!["humanize".into()],
            },
        )]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 10);
        assert_eq!(
            symbols.check_filter("apnumber"),
            FilterAvailability::Unloaded {
                library: "humanize".into()
            }
        );
    }

    #[test]
    fn library_filter_after_load_is_available() {
        let inventory = test_inventory_with_filters();
        let loaded = LoadedLibraries::new(vec![make_load(
            (50, 20),
            LoadKind::FullLoad {
                libraries: vec!["humanize".into()],
            },
        )]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);
        assert_eq!(
            symbols.check_filter("apnumber"),
            FilterAvailability::Available
        );
        assert_eq!(
            symbols.check_filter("intcomma"),
            FilterAvailability::Available
        );
    }

    #[test]
    fn unknown_filter_is_unknown() {
        let inventory = test_inventory_with_filters();
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_filter("nonexistent"),
            FilterAvailability::Unknown
        );
    }

    #[test]
    fn filter_in_multiple_libraries_produces_ambiguous() {
        let filters = vec![
            library_filter_json("shared", "lib_a", "app.templatetags.lib_a"),
            library_filter_json("shared", "lib_b", "app.templatetags.lib_b"),
        ];
        let mut libraries = HashMap::new();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_inventory_with_filters(&[], &filters, &libraries, &[]);
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_filter("shared"),
            FilterAvailability::AmbiguousUnloaded {
                libraries: vec!["lib_a".into(), "lib_b".into()]
            }
        );
    }

    #[test]
    fn selective_import_filter_available() {
        let inventory = test_inventory_with_filters();
        let loaded = LoadedLibraries::new(vec![make_load(
            (10, 30),
            LoadKind::SelectiveImport {
                symbols: vec!["apnumber".into()],
                library: "humanize".into(),
            },
        )]);

        let symbols = AvailableSymbols::at_position(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_filter("apnumber"),
            FilterAvailability::Available
        );
        assert_eq!(
            symbols.check_filter("intcomma"),
            FilterAvailability::Unloaded {
                library: "humanize".into()
            }
        );
    }
}
