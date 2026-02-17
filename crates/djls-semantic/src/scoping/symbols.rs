use djls_project::TemplateLibraries;
use djls_project::TemplateSymbolKind;
use rustc_hash::FxBuildHasher;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use super::LoadState;
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableSymbols {
    /// Tag names that are available at this position (builtins + loaded library tags).
    available: FxHashSet<String>,
    /// Tag names → set of candidate libraries. Only populated for tags NOT in `available`.
    candidates: FxHashMap<String, Vec<String>>,
    /// Filter names that are available at this position (builtins + loaded library filters).
    available_filters: FxHashSet<String>,
    /// Filter names → set of candidate libraries. Only populated for filters NOT in `available_filters`.
    filter_candidates: FxHashMap<String, Vec<String>>,
}

impl AvailableSymbols {
    /// Build available symbols for a given position using load state and template libraries.
    #[must_use]
    pub fn at_position(
        loaded_libraries: &LoadedLibraries,
        template_libraries: &TemplateLibraries,
        position: u32,
    ) -> Self {
        let load_state = loaded_libraries.available_at(position);
        Self::from_load_state(&load_state, template_libraries)
    }

    /// Build available symbols from a pre-computed load state and template libraries.
    #[must_use]
    fn from_load_state(load_state: &LoadState<'_>, template_libraries: &TemplateLibraries) -> Self {
        let (builtin_tag_count, builtin_filter_count) = template_libraries
            .builtin_libraries()
            .flat_map(|lib| &lib.symbols)
            .fold((0usize, 0usize), |(tags, filters), sym| match sym.kind {
                TemplateSymbolKind::Tag => (tags + 1, filters),
                TemplateSymbolKind::Filter => (tags, filters + 1),
            });

        let (loadable_tag_count, loadable_filter_count) = template_libraries
            .enabled_loadable_libraries()
            .flat_map(|(_, lib)| &lib.symbols)
            .fold((0usize, 0usize), |(tags, filters), sym| match sym.kind {
                TemplateSymbolKind::Tag => (tags + 1, filters),
                TemplateSymbolKind::Filter => (tags, filters + 1),
            });

        let mut available = FxHashSet::with_capacity_and_hasher(
            builtin_tag_count + loadable_tag_count,
            FxBuildHasher,
        );
        let mut candidates: FxHashMap<String, Vec<String>> =
            FxHashMap::with_capacity_and_hasher(loadable_tag_count, FxBuildHasher);

        let mut available_filters = FxHashSet::with_capacity_and_hasher(
            builtin_filter_count + loadable_filter_count,
            FxBuildHasher,
        );
        let mut filter_candidates: FxHashMap<String, Vec<String>> =
            FxHashMap::with_capacity_and_hasher(loadable_filter_count, FxBuildHasher);

        // Builtins are always available.
        for library in template_libraries.builtin_libraries() {
            for symbol in &library.symbols {
                let name = symbol.name.as_str().to_string();
                match symbol.kind {
                    TemplateSymbolKind::Tag => {
                        available.insert(name);
                    }
                    TemplateSymbolKind::Filter => {
                        available_filters.insert(name);
                    }
                }
            }
        }

        // Build reverse indices for enabled, loadable libraries.
        for (name, library) in template_libraries.enabled_loadable_libraries() {
            let load_name = name.as_str();

            for symbol in &library.symbols {
                let symbol_name = symbol.name.as_str();
                match symbol.kind {
                    TemplateSymbolKind::Tag => {
                        if !available.contains(symbol_name) {
                            candidates
                                .entry(symbol_name.to_string())
                                .or_default()
                                .push(load_name.to_string());
                        }
                    }
                    TemplateSymbolKind::Filter => {
                        if !available_filters.contains(symbol_name) {
                            filter_candidates
                                .entry(symbol_name.to_string())
                                .or_default()
                                .push(load_name.to_string());
                        }
                    }
                }
            }
        }

        // Move loaded library tags from candidates → available.
        candidates.retain(|tag_name, libs| {
            let is_available = libs
                .iter()
                .any(|lib| load_state.is_symbol_available(lib, tag_name.as_str()));
            if is_available {
                available.insert(tag_name.clone());
                false
            } else {
                true
            }
        });

        // Move loaded library filters from filter_candidates → available_filters.
        filter_candidates.retain(|filter_name, libs| {
            let is_available = libs
                .iter()
                .any(|lib| load_state.is_symbol_available(lib, filter_name.as_str()));
            if is_available {
                available_filters.insert(filter_name.clone());
                false
            } else {
                true
            }
        });

        // Dedup and sort candidate library lists for deterministic output.
        for libs in candidates.values_mut() {
            libs.sort_unstable();
            libs.dedup();
        }
        for libs in filter_candidates.values_mut() {
            libs.sort_unstable();
            libs.dedup();
        }

        Self {
            available,
            candidates,
            available_filters,
            filter_candidates,
        }
    }

    /// Check whether a tag name is available at this position.
    #[must_use]
    pub fn check(&self, tag_name: &str) -> TagAvailability {
        if self.available.contains(tag_name) {
            return TagAvailability::Available;
        }

        if let Some(libs) = self.candidates.get(tag_name) {
            return match libs.as_slice() {
                [] => TagAvailability::Unknown,
                [single] => TagAvailability::Unloaded {
                    library: single.clone(),
                },
                _ => TagAvailability::AmbiguousUnloaded {
                    libraries: libs.clone(),
                },
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
            return match libs.as_slice() {
                [] => FilterAvailability::Unknown,
                [single] => FilterAvailability::Unloaded {
                    library: single.clone(),
                },
                _ => FilterAvailability::AmbiguousUnloaded {
                    libraries: libs.clone(),
                },
            };
        }

        FilterAvailability::Unknown
    }

    /// Returns the set of available tag names.
    #[must_use]
    pub fn available_tags(&self) -> &FxHashSet<String> {
        &self.available
    }

    /// Returns the set of available filter names.
    #[must_use]
    pub fn available_filters(&self) -> &FxHashSet<String> {
        &self.available_filters
    }

    /// Returns the mapping of unavailable tag names to their candidate libraries.
    #[must_use]
    pub fn unavailable_candidates(&self) -> &FxHashMap<String, Vec<String>> {
        &self.candidates
    }
}

/// Precomputed symbol availability at each `{% load %}` boundary in a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolIndex {
    /// Symbols available before any `{% load %}` tags (builtins only).
    initial: AvailableSymbols,
    /// Sorted by position. Entry at index `i` covers positions from
    /// `boundaries[i].0` (inclusive) to `boundaries[i+1].0` (exclusive).
    boundaries: Vec<(u32, AvailableSymbols)>,
}

impl SymbolIndex {
    /// Build a `SymbolIndex` from loaded libraries and template libraries.
    #[must_use]
    pub fn build(
        loaded_libraries: &LoadedLibraries,
        template_libraries: &TemplateLibraries,
    ) -> Self {
        let empty_loaded = LoadedLibraries::new(vec![]);
        let empty_state = empty_loaded.available_at(0);
        let initial = AvailableSymbols::from_load_state(&empty_state, template_libraries);

        let boundaries: Vec<(u32, AvailableSymbols)> = loaded_libraries
            .statements()
            .iter()
            .map(|stmt| {
                let position = stmt.span().end();
                let load_state = loaded_libraries.available_at(position);
                let symbols = AvailableSymbols::from_load_state(&load_state, template_libraries);
                (position, symbols)
            })
            .collect();

        debug_assert!(
            boundaries.windows(2).all(|w| w[0].0 <= w[1].0),
            "SymbolIndex boundaries must be sorted by position"
        );

        Self {
            initial,
            boundaries,
        }
    }

    /// Look up the available symbols at a given byte position.
    #[must_use]
    pub fn symbols_at(&self, position: u32) -> &AvailableSymbols {
        let idx = self.boundaries.partition_point(|&(pos, _)| pos <= position);
        if idx == 0 {
            &self.initial
        } else {
            &self.boundaries[idx - 1].1
        }
    }
}

#[cfg(test)]
mod tests {
    use djls_source::Span;

    use super::super::LoadKind;
    use super::super::LoadStatement;
    use super::*;
    use crate::testing::builtin_filter_json;
    use crate::testing::builtin_tag_json;
    use crate::testing::library_filter_json;
    use crate::testing::library_tag_json;
    use crate::testing::make_template_libraries;
    use crate::testing::make_template_libraries_tags_only;

    fn make_load(span: (u32, u32), kind: LoadKind) -> LoadStatement {
        LoadStatement::new(Span::new(span.0, span.1), kind)
    }

    fn test_inventory() -> TemplateLibraries {
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            builtin_tag_json("for", "django.template.defaulttags"),
            builtin_tag_json("block", "django.template.loader_tags"),
            library_tag_json("trans", "i18n", "django.templatetags.i18n"),
            library_tag_json("blocktrans", "i18n", "django.templatetags.i18n"),
            library_tag_json("static", "static", "django.templatetags.static"),
            library_tag_json("get_static_prefix", "static", "django.templatetags.static"),
        ];

        let mut libraries = FxHashMap::default();
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

        let mut libraries = FxHashMap::default();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_template_libraries_tags_only(&tags, &libraries, &[]);
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

        let mut libraries = FxHashMap::default();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_template_libraries_tags_only(&tags, &libraries, &[]);

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
        assert!(candidates["trans"].contains(&"i18n".to_string()));
        assert!(candidates["static"].contains(&"static".to_string()));
    }

    #[test]
    fn empty_inventory_everything_unknown() {
        let inventory = make_template_libraries_tags_only(&[], &FxHashMap::default(), &[]);
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

        let mut libraries = FxHashMap::default();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_template_libraries_tags_only(&tags, &libraries, &[]);

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

    fn test_inventory_with_filters() -> TemplateLibraries {
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

        let mut libraries = FxHashMap::default();
        libraries.insert(
            "humanize".to_string(),
            "django.contrib.humanize.templatetags.humanize".to_string(),
        );

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];

        make_template_libraries(&tags, &filters, &libraries, &builtins)
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
        let mut libraries = FxHashMap::default();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_template_libraries(&[], &filters, &libraries, &[]);
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

    #[test]
    fn symbol_index_no_loads_uses_initial() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![]);

        let index = SymbolIndex::build(&loaded, &inventory);

        // All positions return builtins only
        let symbols = index.symbols_at(0);
        assert_eq!(symbols.check("if"), TagAvailability::Available);
        assert_eq!(
            symbols.check("trans"),
            TagAvailability::Unloaded {
                library: "i18n".into()
            }
        );

        let symbols = index.symbols_at(1000);
        assert_eq!(symbols.check("if"), TagAvailability::Available);
        assert_eq!(
            symbols.check("trans"),
            TagAvailability::Unloaded {
                library: "i18n".into()
            }
        );
    }

    #[test]
    fn symbol_index_boundary_lookup() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![make_load(
            (50, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        let index = SymbolIndex::build(&loaded, &inventory);

        // Before load: trans is unloaded
        let symbols = index.symbols_at(10);
        assert_eq!(
            symbols.check("trans"),
            TagAvailability::Unloaded {
                library: "i18n".into()
            }
        );

        // After load: trans is available
        let symbols = index.symbols_at(100);
        assert_eq!(symbols.check("trans"), TagAvailability::Available);
    }

    #[test]
    fn symbol_index_multiple_boundaries() {
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

        let index = SymbolIndex::build(&loaded, &inventory);

        // Before first load
        let symbols = index.symbols_at(5);
        assert_eq!(
            symbols.check("trans"),
            TagAvailability::Unloaded {
                library: "i18n".into()
            }
        );
        assert_eq!(
            symbols.check("static"),
            TagAvailability::Unloaded {
                library: "static".into()
            }
        );

        // Between loads
        let symbols = index.symbols_at(50);
        assert_eq!(symbols.check("trans"), TagAvailability::Available);
        assert_eq!(
            symbols.check("static"),
            TagAvailability::Unloaded {
                library: "static".into()
            }
        );

        // After both loads
        let symbols = index.symbols_at(200);
        assert_eq!(symbols.check("trans"), TagAvailability::Available);
        assert_eq!(symbols.check("static"), TagAvailability::Available);
    }

    #[test]
    fn symbol_index_matches_at_position() {
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

        let index = SymbolIndex::build(&loaded, &inventory);

        for pos in [0, 5, 10, 29, 30, 50, 79, 80, 99, 100, 200] {
            let from_index = index.symbols_at(pos);
            let from_direct = AvailableSymbols::at_position(&loaded, &inventory, pos);

            for tag in ["if", "for", "block", "trans", "blocktrans", "static"] {
                assert_eq!(
                    from_index.check(tag),
                    from_direct.check(tag),
                    "Mismatch at position {pos} for tag '{tag}'"
                );
            }
        }
    }

    #[test]
    fn symbol_index_filter_boundary() {
        let inventory = test_inventory_with_filters();
        let loaded = LoadedLibraries::new(vec![make_load(
            (50, 20),
            LoadKind::FullLoad {
                libraries: vec!["humanize".into()],
            },
        )]);

        let index = SymbolIndex::build(&loaded, &inventory);

        for pos in [0, 5, 10, 49, 50, 69, 70, 100, 200] {
            let from_index = index.symbols_at(pos);
            let from_direct = AvailableSymbols::at_position(&loaded, &inventory, pos);

            for filter in ["title", "lower", "apnumber", "intcomma", "nonexistent"] {
                assert_eq!(
                    from_index.check_filter(filter),
                    from_direct.check_filter(filter),
                    "Filter mismatch at position {pos} for filter '{filter}'"
                );
            }
        }
    }
}
