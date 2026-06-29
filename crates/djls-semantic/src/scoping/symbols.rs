use djls_project::TemplateLibraries;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolKind;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::scoping::LoadState;
use crate::scoping::LoadedLibraries;

/// The result of checking a tag or filter name against the available symbols at a position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolAvailability {
    /// The symbol is available (builtin or from a loaded library).
    Available,
    /// The symbol is known but its library is not loaded. Contains exactly one
    /// candidate library name.
    Unloaded { library: String },
    /// The symbol is known but defined in multiple unloaded libraries. Contains
    /// all candidate library names, sorted alphabetically.
    AmbiguousUnloaded { libraries: Vec<String> },
    /// The symbol is completely unknown — not builtin and not in any known
    /// loadable library.
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SymbolScope {
    available: FxHashSet<String>,
    candidates: FxHashMap<String, Vec<String>>,
}

impl SymbolScope {
    fn insert_available(&mut self, name: &str) {
        self.available.insert(name.to_string());
    }

    fn insert_candidate(&mut self, symbol_name: &str, library_name: &str) {
        if !self.available.contains(symbol_name) {
            self.candidates
                .entry(symbol_name.to_string())
                .or_default()
                .push(library_name.to_string());
        }
    }

    fn apply_load_state(&mut self, load_state: &LoadState<'_>) {
        let Self {
            available,
            candidates,
        } = self;

        candidates.retain(|symbol_name, libraries| {
            let is_available = libraries
                .iter()
                .any(|library| load_state.is_symbol_available(library, symbol_name.as_str()));
            if is_available {
                available.insert(symbol_name.clone());
                false
            } else {
                true
            }
        });

        for libraries in candidates.values_mut() {
            libraries.sort_unstable();
            libraries.dedup();
        }
    }

    fn check(&self, symbol_name: &str) -> SymbolAvailability {
        if self.available.contains(symbol_name) {
            return SymbolAvailability::Available;
        }

        if let Some(libraries) = self.candidates.get(symbol_name) {
            return match libraries.as_slice() {
                [] => SymbolAvailability::Unknown,
                [single] => SymbolAvailability::Unloaded {
                    library: single.clone(),
                },
                _ => SymbolAvailability::AmbiguousUnloaded {
                    libraries: libraries.clone(),
                },
            };
        }

        SymbolAvailability::Unknown
    }

    fn contains(&self, symbol_name: &str) -> bool {
        self.available.contains(symbol_name)
    }
}

/// The set of tags and filters available at a given position in a template,
/// plus a mapping of unavailable-but-known symbols to their required library/libraries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableSymbols {
    tags: SymbolScope,
    filters: SymbolScope,
}

impl AvailableSymbols {
    /// Build available symbols from a pre-computed load state and template libraries.
    #[must_use]
    fn from_load_state(load_state: &LoadState<'_>, template_libraries: &TemplateLibraries) -> Self {
        let mut tags = SymbolScope::default();
        let mut filters = SymbolScope::default();

        // Builtins are always available.
        for library in template_libraries.builtin_libraries() {
            for symbol in library.symbols() {
                match symbol.kind {
                    TemplateSymbolKind::Tag => tags.insert_available(symbol.name.as_str()),
                    TemplateSymbolKind::Filter => filters.insert_available(symbol.name.as_str()),
                }
            }
        }

        // Build reverse indices for loadable libraries.
        for (name, library) in template_libraries.loadable_libraries() {
            let load_name = name.as_str();

            for symbol in library.symbols() {
                match symbol.kind {
                    TemplateSymbolKind::Tag => {
                        tags.insert_candidate(symbol.name.as_str(), load_name);
                    }
                    TemplateSymbolKind::Filter => {
                        filters.insert_candidate(symbol.name.as_str(), load_name);
                    }
                }
            }
        }

        tags.apply_load_state(load_state);
        filters.apply_load_state(load_state);

        Self { tags, filters }
    }

    /// Check whether a tag name is available at this position.
    #[must_use]
    pub(crate) fn check_tag(&self, tag_name: &str) -> SymbolAvailability {
        self.tags.check(tag_name)
    }

    /// Check whether a template symbol is available at this position.
    #[must_use]
    pub fn contains_symbol(&self, symbol: &TemplateSymbol) -> bool {
        match symbol.kind {
            TemplateSymbolKind::Tag => self.tags.contains(symbol.name()),
            TemplateSymbolKind::Filter => self.filters.contains(symbol.name()),
        }
    }

    /// Check whether a filter name is available at this position.
    #[must_use]
    pub(crate) fn check_filter(&self, filter_name: &str) -> SymbolAvailability {
        self.filters.check(filter_name)
    }
}

/// Precomputed symbol availability at each `{% load %}` boundary in a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SymbolIndex {
    /// Symbols available before any `{% load %}` tags (builtins only).
    initial: AvailableSymbols,
    /// Sorted by position. Entry at index `i` covers positions from
    /// `boundaries[i].0` (inclusive) to `boundaries[i+1].0` (exclusive).
    boundaries: Vec<(u32, AvailableSymbols)>,
}

impl SymbolIndex {
    /// Build a `SymbolIndex` from loaded libraries and template libraries.
    #[must_use]
    pub(crate) fn build(
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
    pub(crate) fn symbols_at(&self, position: u32) -> &AvailableSymbols {
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
    use std::collections::BTreeMap;

    use djls_project::BuiltinLibrarySource;
    use djls_project::LibraryName;
    use djls_project::LoadableLibrarySource;
    use djls_project::PythonModulePath;
    use djls_project::SymbolDefinition;
    use djls_project::TemplateSymbolName;
    use djls_source::Span;

    use super::super::LoadKind;
    use super::super::LoadStatement;
    use super::*;

    fn make_load(span: (u32, u32), kind: LoadKind) -> LoadStatement {
        LoadStatement::new(Span::new(span.0, span.1), kind)
    }

    fn available_symbols_at(
        loaded_libraries: &LoadedLibraries,
        template_libraries: &TemplateLibraries,
        position: u32,
    ) -> AvailableSymbols {
        let load_state = loaded_libraries.available_at(position);
        AvailableSymbols::from_load_state(&load_state, template_libraries)
    }

    fn builtin_tag(name: &str, module: &str) -> serde_json::Value {
        template_symbol_fixture("tag", name, None, module, module)
    }

    fn library_tag(name: &str, load_name: &str, module: &str) -> serde_json::Value {
        template_symbol_fixture("tag", name, Some(load_name), module, module)
    }

    fn builtin_filter(name: &str, module: &str) -> serde_json::Value {
        template_symbol_fixture("filter", name, None, module, module)
    }

    fn library_filter(name: &str, load_name: &str, module: &str) -> serde_json::Value {
        template_symbol_fixture("filter", name, Some(load_name), module, module)
    }

    fn template_symbol_fixture(
        kind: &str,
        name: &str,
        load_name: Option<&str>,
        library_module: &str,
        module: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "kind": kind,
            "name": name,
            "load_name": load_name,
            "library_module": library_module,
            "module": module,
            "doc": null,
        })
    }

    #[derive(serde::Deserialize)]
    struct TemplateSymbolFixture {
        kind: TemplateSymbolKind,
        name: String,
        #[serde(default)]
        load_name: Option<String>,
        library_module: String,
        module: String,
        #[serde(default)]
        doc: Option<String>,
    }

    fn make_template_libraries(
        tags: &[serde_json::Value],
        filters: &[serde_json::Value],
        libraries: &FxHashMap<String, String>,
        builtins: &[String],
    ) -> TemplateLibraries {
        let mut builtin_symbols: BTreeMap<PythonModulePath, Vec<TemplateSymbol>> = BTreeMap::new();
        for module_name in builtins {
            let Ok(module) = PythonModulePath::parse(module_name) else {
                continue;
            };
            builtin_symbols.entry(module).or_default();
        }

        let mut loadable_symbols: BTreeMap<LibraryName, (PythonModulePath, Vec<TemplateSymbol>)> =
            BTreeMap::new();
        for (load_name, module_name) in libraries {
            let Ok(load_name) = LibraryName::parse(load_name) else {
                continue;
            };
            let Ok(module) = PythonModulePath::parse(module_name) else {
                continue;
            };
            loadable_symbols.insert(load_name, (module, Vec::new()));
        }

        let symbols = tags.iter().chain(filters.iter()).cloned();
        for fixture in symbols
            .map(serde_json::from_value)
            .collect::<Result<Vec<TemplateSymbolFixture>, _>>()
            .unwrap()
        {
            let Ok(name) = TemplateSymbolName::parse(&fixture.name) else {
                continue;
            };
            let definition = PythonModulePath::parse(&fixture.module)
                .map_or(SymbolDefinition::Unknown, SymbolDefinition::Module);
            let symbol = TemplateSymbol {
                kind: fixture.kind,
                name,
                definition,
                doc: fixture.doc,
            };

            match fixture.load_name {
                None => {
                    let Ok(module) = PythonModulePath::parse(&fixture.library_module) else {
                        continue;
                    };
                    if let Some(symbols) = builtin_symbols.get_mut(&module) {
                        symbols.push(symbol);
                    }
                }
                Some(load_name) => {
                    let Ok(load_name) = LibraryName::parse(&load_name) else {
                        continue;
                    };
                    let Ok(module) = PythonModulePath::parse(&fixture.library_module) else {
                        continue;
                    };
                    let entry = loadable_symbols
                        .entry(load_name)
                        .or_insert_with(|| (module.clone(), Vec::new()));
                    if entry.0 == module {
                        entry.1.push(symbol);
                    }
                }
            }
        }

        let mut builder = TemplateLibraries::builder();
        for (module, symbols) in builtin_symbols {
            builder = builder.builtin_untracked(
                BuiltinLibrarySource::DjangoDefault,
                module,
                true,
                symbols,
            );
        }
        for (load_name, (module, symbols)) in loadable_symbols {
            builder = builder.loadable_untracked(
                load_name,
                LoadableLibrarySource::ConfiguredAlias,
                module,
                true,
                symbols,
            );
        }
        builder.build()
    }

    fn make_template_libraries_tags_only(
        tags: &[serde_json::Value],
        libraries: &FxHashMap<String, String>,
        builtins: &[String],
    ) -> TemplateLibraries {
        make_template_libraries(tags, &[], libraries, builtins)
    }

    fn test_inventory() -> TemplateLibraries {
        let tags = vec![
            builtin_tag("if", "django.template.defaulttags"),
            builtin_tag("for", "django.template.defaulttags"),
            builtin_tag("block", "django.template.loader_tags"),
            library_tag("trans", "i18n", "django.templatetags.i18n"),
            library_tag("blocktrans", "i18n", "django.templatetags.i18n"),
            library_tag("static", "static", "django.templatetags.static"),
            library_tag("get_static_prefix", "static", "django.templatetags.static"),
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

        let symbols = available_symbols_at(&loaded, &inventory, 0);

        assert_eq!(symbols.check_tag("if"), SymbolAvailability::Available);
        assert_eq!(symbols.check_tag("for"), SymbolAvailability::Available);
        assert_eq!(symbols.check_tag("block"), SymbolAvailability::Available);
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
        let symbols = available_symbols_at(&loaded, &inventory, 10);
        assert_eq!(
            symbols.check_tag("trans"),
            SymbolAvailability::Unloaded {
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
        let symbols = available_symbols_at(&loaded, &inventory, 100);
        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
        assert_eq!(
            symbols.check_tag("blocktrans"),
            SymbolAvailability::Available
        );
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

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        // trans is selectively imported → available
        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
        // blocktrans is NOT imported → unloaded
        assert_eq!(
            symbols.check_tag("blocktrans"),
            SymbolAvailability::Unloaded {
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
        let symbols = available_symbols_at(&loaded, &inventory, 100);
        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
        assert_eq!(
            symbols.check_tag("blocktrans"),
            SymbolAvailability::Available
        );
    }

    #[test]
    fn unknown_tag_is_unknown() {
        let inventory = test_inventory();
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_tag("nonexistent_tag"),
            SymbolAvailability::Unknown
        );
        assert_eq!(symbols.check_tag("foobar"), SymbolAvailability::Unknown);
    }

    #[test]
    fn tag_in_multiple_libraries_produces_ambiguous() {
        // Create inventory with a tag defined in two libraries
        let tags = vec![
            builtin_tag("if", "django.template.defaulttags"),
            library_tag("mytag", "lib_a", "app.templatetags.lib_a"),
            library_tag("mytag", "lib_b", "app.templatetags.lib_b"),
        ];

        let mut libraries = FxHashMap::default();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_template_libraries_tags_only(&tags, &libraries, &[]);
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_tag("mytag"),
            SymbolAvailability::AmbiguousUnloaded {
                libraries: vec!["lib_a".into(), "lib_b".into()]
            }
        );
    }

    #[test]
    fn ambiguous_resolved_by_loading_one_library() {
        let tags = vec![
            library_tag("mytag", "lib_a", "app.templatetags.lib_a"),
            library_tag("mytag", "lib_b", "app.templatetags.lib_b"),
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

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        // Even though mytag is in lib_b too, loading lib_a makes it available
        assert_eq!(symbols.check_tag("mytag"), SymbolAvailability::Available);
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
        let symbols = available_symbols_at(&loaded, &inventory, 100);

        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
        assert_eq!(symbols.check_tag("static"), SymbolAvailability::Available);
        assert_eq!(
            symbols.check_tag("get_static_prefix"),
            SymbolAvailability::Available
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
        let symbols = available_symbols_at(&loaded, &inventory, 50);

        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
        assert_eq!(
            symbols.check_tag("static"),
            SymbolAvailability::Unloaded {
                library: "static".into()
            }
        );
    }

    #[test]
    fn empty_inventory_everything_unknown() {
        let inventory = make_template_libraries_tags_only(&[], &FxHashMap::default(), &[]);
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        assert_eq!(symbols.check_tag("anything"), SymbolAvailability::Unknown);
        assert_eq!(
            symbols.check_filter("anything"),
            SymbolAvailability::Unknown
        );
    }

    #[test]
    fn selective_import_with_ambiguous_tag() {
        // Tag "shared" exists in both lib_a and lib_b
        let tags = vec![
            library_tag("shared", "lib_a", "app.templatetags.lib_a"),
            library_tag("shared", "lib_b", "app.templatetags.lib_b"),
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

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        // Should be available because we imported it from lib_a
        assert_eq!(symbols.check_tag("shared"), SymbolAvailability::Available);
    }

    fn test_inventory_with_filters() -> TemplateLibraries {
        let tags = vec![builtin_tag("if", "django.template.defaulttags")];
        let filters = vec![
            builtin_filter("title", "django.template.defaultfilters"),
            builtin_filter("lower", "django.template.defaultfilters"),
            library_filter(
                "apnumber",
                "humanize",
                "django.contrib.humanize.templatetags.humanize",
            ),
            library_filter(
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

        let symbols = available_symbols_at(&loaded, &inventory, 0);

        assert_eq!(symbols.check_filter("title"), SymbolAvailability::Available);
        assert_eq!(symbols.check_filter("lower"), SymbolAvailability::Available);
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

        let symbols = available_symbols_at(&loaded, &inventory, 10);
        assert_eq!(
            symbols.check_filter("apnumber"),
            SymbolAvailability::Unloaded {
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

        let symbols = available_symbols_at(&loaded, &inventory, 100);
        assert_eq!(
            symbols.check_filter("apnumber"),
            SymbolAvailability::Available
        );
        assert_eq!(
            symbols.check_filter("intcomma"),
            SymbolAvailability::Available
        );
    }

    #[test]
    fn unknown_filter_is_unknown() {
        let inventory = test_inventory_with_filters();
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_filter("nonexistent"),
            SymbolAvailability::Unknown
        );
    }

    #[test]
    fn filter_in_multiple_libraries_produces_ambiguous() {
        let filters = vec![
            library_filter("shared", "lib_a", "app.templatetags.lib_a"),
            library_filter("shared", "lib_b", "app.templatetags.lib_b"),
        ];
        let mut libraries = FxHashMap::default();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());

        let inventory = make_template_libraries(&[], &filters, &libraries, &[]);
        let loaded = LoadedLibraries::new(vec![]);

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_filter("shared"),
            SymbolAvailability::AmbiguousUnloaded {
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

        let symbols = available_symbols_at(&loaded, &inventory, 100);

        assert_eq!(
            symbols.check_filter("apnumber"),
            SymbolAvailability::Available
        );
        assert_eq!(
            symbols.check_filter("intcomma"),
            SymbolAvailability::Unloaded {
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
        assert_eq!(symbols.check_tag("if"), SymbolAvailability::Available);
        assert_eq!(
            symbols.check_tag("trans"),
            SymbolAvailability::Unloaded {
                library: "i18n".into()
            }
        );

        let symbols = index.symbols_at(1000);
        assert_eq!(symbols.check_tag("if"), SymbolAvailability::Available);
        assert_eq!(
            symbols.check_tag("trans"),
            SymbolAvailability::Unloaded {
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
            symbols.check_tag("trans"),
            SymbolAvailability::Unloaded {
                library: "i18n".into()
            }
        );

        // After load: trans is available
        let symbols = index.symbols_at(100);
        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
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
            symbols.check_tag("trans"),
            SymbolAvailability::Unloaded {
                library: "i18n".into()
            }
        );
        assert_eq!(
            symbols.check_tag("static"),
            SymbolAvailability::Unloaded {
                library: "static".into()
            }
        );

        // Between loads
        let symbols = index.symbols_at(50);
        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
        assert_eq!(
            symbols.check_tag("static"),
            SymbolAvailability::Unloaded {
                library: "static".into()
            }
        );

        // After both loads
        let symbols = index.symbols_at(200);
        assert_eq!(symbols.check_tag("trans"), SymbolAvailability::Available);
        assert_eq!(symbols.check_tag("static"), SymbolAvailability::Available);
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
            let from_direct = available_symbols_at(&loaded, &inventory, pos);

            for tag in ["if", "for", "block", "trans", "blocktrans", "static"] {
                assert_eq!(
                    from_index.check_tag(tag),
                    from_direct.check_tag(tag),
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
            let from_direct = available_symbols_at(&loaded, &inventory, pos);

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
