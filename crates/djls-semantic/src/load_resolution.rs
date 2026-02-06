use djls_project::FilterProvenance;
use djls_project::InspectorInventory;
use djls_project::TagProvenance;
use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use salsa::Accumulator;

use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// A parsed `{% load %}` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadStatement {
    pub span: Span,
    pub kind: LoadKind,
}

/// The kind of load statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadKind {
    /// Load entire libraries: `{% load i18n static %}`
    Libraries(Vec<String>),
    /// Selective import: `{% load trans blocktrans from i18n %}`
    Selective {
        symbols: Vec<String>,
        library: String,
    },
}

/// Collection of load statements in a template, ordered by position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedLibraries {
    loads: Vec<LoadStatement>,
}

impl LoadedLibraries {
    #[must_use]
    pub fn new() -> Self {
        Self { loads: Vec::new() }
    }

    pub fn push(&mut self, statement: LoadStatement) {
        self.loads.push(statement);
    }

    #[must_use]
    pub fn loads(&self) -> &[LoadStatement] {
        &self.loads
    }

    /// Get libraries loaded before a given position (exclusive).
    ///
    /// Returns the set of library names that have been loaded
    /// in `{% load %}` statements appearing before `position`.
    #[must_use]
    pub fn libraries_before(&self, position: u32) -> FxHashSet<String> {
        let mut libs = FxHashSet::default();
        for stmt in &self.loads {
            if stmt.span.end() <= position {
                match &stmt.kind {
                    LoadKind::Libraries(names) => {
                        libs.extend(names.iter().cloned());
                    }
                    LoadKind::Selective { library, .. } => {
                        libs.insert(library.clone());
                    }
                }
            }
        }
        libs
    }

    /// Get specific symbols available from selective imports before a position.
    ///
    /// Returns a set of (`symbol_name`, `library_name`) pairs for selective imports only.
    #[must_use]
    pub fn selective_symbols_before(&self, position: u32) -> FxHashSet<(String, String)> {
        let mut symbols = FxHashSet::default();
        for stmt in &self.loads {
            if stmt.span.end() <= position {
                if let LoadKind::Selective {
                    symbols: syms,
                    library,
                } = &stmt.kind
                {
                    for sym in syms {
                        symbols.insert((sym.clone(), library.clone()));
                    }
                }
            }
        }
        symbols
    }

    /// Check if a specific library is loaded before a position.
    #[must_use]
    pub fn is_library_loaded_before(&self, library: &str, position: u32) -> bool {
        self.libraries_before(position).contains(library)
    }
}

/// Parse the `bits` of a `{% load %}` tag into a `LoadStatement`.
///
/// Handles two forms:
/// - `{% load lib1 lib2 %}` → `LoadKind::Libraries(["lib1", "lib2"])`
/// - `{% load sym1 sym2 from lib %}` → `LoadKind::Selective { symbols: ["sym1", "sym2"], library: "lib" }`
#[must_use]
pub fn parse_load_bits(bits: &[String], span: Span) -> Option<LoadStatement> {
    if bits.is_empty() {
        return None;
    }

    if let Some(from_idx) = bits.iter().position(|b| b == "from") {
        if from_idx == 0 || from_idx + 1 >= bits.len() {
            return None;
        }

        let symbols: Vec<String> = bits[..from_idx].to_vec();
        let library = bits[from_idx + 1].clone();

        return Some(LoadStatement {
            span,
            kind: LoadKind::Selective { symbols, library },
        });
    }

    Some(LoadStatement {
        span,
        kind: LoadKind::Libraries(bits.to_vec()),
    })
}

/// Extract all `{% load %}` statements from a template nodelist.
///
/// Performs a single pass over the nodelist, collecting all load statements
/// in document order (sorted by span start position).
///
/// Django's template parser processes tokens linearly, so `{% load %}` tags
/// affect global tag availability regardless of nesting. The djls-templates
/// parser currently produces a flat nodelist, but we sort by position to be
/// safe if that ever changes.
#[salsa::tracked]
pub fn compute_loaded_libraries(
    db: &dyn crate::Db,
    nodelist: djls_templates::NodeList<'_>,
) -> LoadedLibraries {
    let mut load_spans: Vec<(Span, LoadStatement)> = Vec::new();

    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            if name == "load" {
                if let Some(stmt) = parse_load_bits(bits, *span) {
                    load_spans.push((*span, stmt));
                }
            }
        }
    }

    load_spans.sort_by_key(|(span, _)| span.start());

    let mut loaded = LoadedLibraries::new();
    for (_, stmt) in load_spans {
        loaded.push(stmt);
    }

    loaded
}

/// Result of querying available symbols at a position.
#[derive(Debug, Clone, Default)]
pub struct AvailableSymbols {
    tags: FxHashSet<String>,
}

impl AvailableSymbols {
    /// Check if a tag name is available at this position.
    #[must_use]
    pub fn has_tag(&self, name: &str) -> bool {
        self.tags.contains(name)
    }
}

/// Load state at a given position, computed by processing loads in order.
///
/// Uses a state-machine approach to correctly handle the interaction
/// between selective imports and full library loads:
/// - `{% load trans from i18n %}` adds "trans" to selective\[i18n\]
/// - `{% load i18n %}` adds "i18n" to fully\_loaded AND clears selective\[i18n\]
#[derive(Debug, Clone, Default)]
struct LoadState {
    fully_loaded: FxHashSet<String>,
    selective: FxHashMap<String, FxHashSet<String>>,
}

impl LoadState {
    fn process(&mut self, stmt: &LoadStatement) {
        match &stmt.kind {
            LoadKind::Libraries(libs) => {
                for lib in libs {
                    self.fully_loaded.insert(lib.clone());
                    self.selective.remove(lib);
                }
            }
            LoadKind::Selective { symbols, library } => {
                if !self.fully_loaded.contains(library) {
                    let entry = self.selective.entry(library.clone()).or_default();
                    entry.extend(symbols.iter().cloned());
                }
            }
        }
    }

    fn is_tag_available(&self, tag_name: &str, library: &str) -> bool {
        if self.fully_loaded.contains(library) {
            return true;
        }
        if let Some(symbols) = self.selective.get(library) {
            return symbols.contains(tag_name);
        }
        false
    }
}

/// Determine which tags are available at a given byte offset in the template.
///
/// Processes load statements in document order up to `position`:
/// 1. `{% load lib1 lib2 %}`: add libraries to "fully loaded" set,
///    clear any selective imports for those libraries
/// 2. `{% load sym from lib %}`: if lib not fully loaded, add sym
///    to selective imports for lib
///
/// Builtins are always available. A library tag is available iff its library
/// is fully loaded or the tag name was selectively imported.
#[must_use]
pub fn available_tags_at(
    loaded: &LoadedLibraries,
    inventory: &InspectorInventory,
    position: u32,
) -> AvailableSymbols {
    let mut available = AvailableSymbols::default();

    let mut state = LoadState::default();
    for stmt in loaded.loads() {
        if stmt.span.end() <= position {
            state.process(stmt);
        }
    }

    for tag in inventory.iter_tags() {
        match tag.provenance() {
            TagProvenance::Builtin { .. } => {
                available.tags.insert(tag.name().to_string());
            }
            TagProvenance::Library { load_name, .. } => {
                if state.is_tag_available(tag.name(), load_name) {
                    available.tags.insert(tag.name().to_string());
                }
            }
        }
    }

    available
}

/// Result of querying available filters at a position.
#[derive(Debug, Clone, Default)]
pub struct AvailableFilters {
    filters: FxHashSet<String>,
}

impl AvailableFilters {
    /// Check if a filter name is available at this position.
    #[must_use]
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.contains(name)
    }
}

/// Determine which filters are available at a given byte offset in the template.
///
/// Same logic as `available_tags_at` but for filters:
/// builtins are always available, library filters require a loaded library
/// or selective import.
#[must_use]
pub fn available_filters_at(
    loaded: &LoadedLibraries,
    inventory: &InspectorInventory,
    position: u32,
) -> AvailableFilters {
    let mut available = AvailableFilters::default();

    let mut state = LoadState::default();
    for stmt in loaded.loads() {
        if stmt.span.end() <= position {
            state.process(stmt);
        }
    }

    for filter in inventory.iter_filters() {
        match filter.provenance() {
            FilterProvenance::Builtin { .. } => {
                available.filters.insert(filter.name().to_string());
            }
            FilterProvenance::Library { load_name, .. } => {
                if state.is_tag_available(filter.name(), load_name) {
                    available.filters.insert(filter.name().to_string());
                }
            }
        }
    }

    available
}

#[derive(Debug, Clone)]
enum TagInventoryEntry {
    Builtin,
    Libraries(Vec<String>),
}

fn build_tag_inventory(inventory: &InspectorInventory) -> FxHashMap<String, TagInventoryEntry> {
    let mut result: FxHashMap<String, TagInventoryEntry> = FxHashMap::default();

    for tag in inventory.iter_tags() {
        let name = tag.name().to_string();
        match tag.provenance() {
            TagProvenance::Builtin { .. } => {
                result.insert(name, TagInventoryEntry::Builtin);
            }
            TagProvenance::Library { load_name, .. } => {
                if let Some(entry) = result.get_mut(&name) {
                    if let TagInventoryEntry::Libraries(libs) = entry {
                        if !libs.contains(load_name) {
                            libs.push(load_name.clone());
                        }
                    }
                } else {
                    result.insert(name, TagInventoryEntry::Libraries(vec![load_name.clone()]));
                }
            }
        }
    }

    result
}

/// Validate that all tags in the template are either builtins or from loaded libraries.
///
/// Skips validation entirely when inspector inventory is unavailable.
/// Tags with structural specs (openers, closers, intermediates) are skipped
/// since they are validated by block structure (S100-S103) and argument
/// validation (S104-S107).
#[salsa::tracked]
pub fn validate_tag_scoping(db: &dyn crate::Db, nodelist: djls_templates::NodeList<'_>) {
    let Some(inventory) = db.inspector_inventory() else {
        return;
    };

    let loaded = compute_loaded_libraries(db, nodelist);
    let tag_inventory = build_tag_inventory(&inventory);
    let tag_specs = db.tag_specs();

    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, span, .. } = node {
            if name == "load" {
                continue;
            }

            let has_spec = tag_specs.get(name).is_some()
                || tag_specs.get_end_spec_for_closer(name).is_some()
                || tag_specs.get_intermediate_spec(name).is_some();

            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

            match tag_inventory.get(name) {
                None => {
                    if !has_spec {
                        ValidationErrorAccumulator(ValidationError::UnknownTag {
                            tag: name.clone(),
                            span: marker_span,
                        })
                        .accumulate(db);
                    }
                }
                Some(TagInventoryEntry::Builtin) => {}
                Some(TagInventoryEntry::Libraries(candidate_libs)) => {
                    let available = available_tags_at(&loaded, &inventory, span.start());

                    if !available.has_tag(name) {
                        if candidate_libs.len() == 1 {
                            ValidationErrorAccumulator(ValidationError::UnloadedLibraryTag {
                                tag: name.clone(),
                                library: candidate_libs[0].clone(),
                                span: marker_span,
                            })
                            .accumulate(db);
                        } else {
                            ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedTag {
                                tag: name.clone(),
                                libraries: candidate_libs.clone(),
                                span: marker_span,
                            })
                            .accumulate(db);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_load_single_library() {
        let bits = vec!["i18n".to_string()];
        let span = Span::new(0, 10);

        let stmt = parse_load_bits(&bits, span).expect("should parse");
        assert_eq!(stmt.kind, LoadKind::Libraries(vec!["i18n".to_string()]));
    }

    #[test]
    fn test_parse_load_multiple_libraries() {
        let bits = vec!["i18n".to_string(), "static".to_string()];
        let span = Span::new(0, 20);

        let stmt = parse_load_bits(&bits, span).expect("should parse");
        assert_eq!(
            stmt.kind,
            LoadKind::Libraries(vec!["i18n".to_string(), "static".to_string()])
        );
    }

    #[test]
    fn test_parse_load_selective_single() {
        let bits = vec!["trans".to_string(), "from".to_string(), "i18n".to_string()];
        let span = Span::new(0, 25);

        let stmt = parse_load_bits(&bits, span).expect("should parse");
        assert_eq!(
            stmt.kind,
            LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_load_selective_multiple() {
        let bits = vec![
            "trans".to_string(),
            "blocktrans".to_string(),
            "from".to_string(),
            "i18n".to_string(),
        ];
        let span = Span::new(0, 35);

        let stmt = parse_load_bits(&bits, span).expect("should parse");
        assert_eq!(
            stmt.kind,
            LoadKind::Selective {
                symbols: vec!["trans".to_string(), "blocktrans".to_string()],
                library: "i18n".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_load_empty() {
        let bits: Vec<String> = vec![];
        let span = Span::new(0, 5);

        assert!(parse_load_bits(&bits, span).is_none());
    }

    #[test]
    fn test_parse_load_invalid_from_no_symbols() {
        let bits = vec!["from".to_string(), "i18n".to_string()];
        let span = Span::new(0, 15);

        assert!(parse_load_bits(&bits, span).is_none());
    }

    #[test]
    fn test_parse_load_invalid_from_no_library() {
        let bits = vec!["trans".to_string(), "from".to_string()];
        let span = Span::new(0, 15);

        assert!(parse_load_bits(&bits, span).is_none());
    }

    #[test]
    fn test_libraries_before_position() {
        let mut libs = LoadedLibraries::new();

        // {% load i18n %} at position 0, length 15 → end = 15
        libs.push(LoadStatement {
            span: Span::new(0, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });

        // {% load static %} at position 50, length 18 → end = 68
        libs.push(LoadStatement {
            span: Span::new(50, 18),
            kind: LoadKind::Libraries(vec!["static".to_string()]),
        });

        // Before any load
        assert!(libs.libraries_before(0).is_empty());

        // After first load, before second
        let at_30 = libs.libraries_before(30);
        assert!(at_30.contains("i18n"));
        assert!(!at_30.contains("static"));

        // After both loads
        let at_100 = libs.libraries_before(100);
        assert!(at_100.contains("i18n"));
        assert!(at_100.contains("static"));
    }

    #[test]
    fn test_selective_symbols_before() {
        let mut libs = LoadedLibraries::new();

        // {% load trans from i18n %} at position 0, length 25 → end = 25
        libs.push(LoadStatement {
            span: Span::new(0, 25),
            kind: LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            },
        });

        // Before the load ends
        assert!(libs.selective_symbols_before(10).is_empty());

        // After the load
        let symbols = libs.selective_symbols_before(50);
        assert!(symbols.contains(&("trans".to_string(), "i18n".to_string())));
    }

    #[test]
    fn test_is_library_loaded_before() {
        let mut libs = LoadedLibraries::new();

        libs.push(LoadStatement {
            span: Span::new(0, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });

        assert!(!libs.is_library_loaded_before("i18n", 10));
        assert!(libs.is_library_loaded_before("i18n", 20));
        assert!(!libs.is_library_loaded_before("static", 20));
    }
}

#[cfg(test)]
mod availability_tests {
    use std::collections::HashMap;

    use djls_project::TemplateTag;

    use super::*;

    fn make_test_inventory() -> InspectorInventory {
        InspectorInventory::new(
            HashMap::from([
                ("i18n".to_string(), "django.templatetags.i18n".to_string()),
                (
                    "static".to_string(),
                    "django.templatetags.static".to_string(),
                ),
            ]),
            vec!["django.template.defaulttags".to_string()],
            vec![
                TemplateTag::new_builtin("if", "django.template.defaulttags", None),
                TemplateTag::new_library("trans", "i18n", "django.templatetags.i18n", None),
                TemplateTag::new_library("blocktrans", "i18n", "django.templatetags.i18n", None),
                TemplateTag::new_library("static", "static", "django.templatetags.static", None),
            ],
            vec![],
        )
    }

    #[test]
    fn builtins_always_available() {
        let loaded = LoadedLibraries::new();
        let inventory = make_test_inventory();

        let available = available_tags_at(&loaded, &inventory, 0);

        assert!(
            available.has_tag("if"),
            "Builtins should always be available"
        );
        assert!(
            !available.has_tag("trans"),
            "Library tags should NOT be available without load"
        );
    }

    #[test]
    fn library_tag_after_load() {
        let mut loaded = LoadedLibraries::new();
        loaded.push(LoadStatement {
            span: Span::new(0, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });
        let inventory = make_test_inventory();

        let before = available_tags_at(&loaded, &inventory, 5);
        assert!(
            !before.has_tag("trans"),
            "trans should not be available inside load tag"
        );

        let after = available_tags_at(&loaded, &inventory, 20);
        assert!(after.has_tag("trans"), "trans should be available after load");
        assert!(
            after.has_tag("blocktrans"),
            "blocktrans should be available after load"
        );
        assert!(
            !after.has_tag("static"),
            "static should NOT be available (not loaded)"
        );
    }

    #[test]
    fn selective_import() {
        let mut loaded = LoadedLibraries::new();
        loaded.push(LoadStatement {
            span: Span::new(0, 30),
            kind: LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            },
        });
        let inventory = make_test_inventory();

        let available = available_tags_at(&loaded, &inventory, 50);

        assert!(
            available.has_tag("trans"),
            "trans should be available (selectively imported)"
        );
        assert!(
            !available.has_tag("blocktrans"),
            "blocktrans should NOT be available (not in selective import)"
        );
    }

    #[test]
    fn selective_then_full_load() {
        let mut loaded = LoadedLibraries::new();
        loaded.push(LoadStatement {
            span: Span::new(0, 30),
            kind: LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            },
        });
        loaded.push(LoadStatement {
            span: Span::new(100, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });
        let inventory = make_test_inventory();

        let middle = available_tags_at(&loaded, &inventory, 50);
        assert!(middle.has_tag("trans"));
        assert!(!middle.has_tag("blocktrans"));

        let after = available_tags_at(&loaded, &inventory, 150);
        assert!(
            after.has_tag("trans"),
            "trans still available after full load"
        );
        assert!(
            after.has_tag("blocktrans"),
            "blocktrans NOW available after full load"
        );
    }

    #[test]
    fn full_then_selective_no_effect() {
        let mut loaded = LoadedLibraries::new();
        loaded.push(LoadStatement {
            span: Span::new(0, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });
        loaded.push(LoadStatement {
            span: Span::new(100, 30),
            kind: LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            },
        });
        let inventory = make_test_inventory();

        let after = available_tags_at(&loaded, &inventory, 200);
        assert!(after.has_tag("trans"));
        assert!(after.has_tag("blocktrans"), "Full load takes precedence");
    }

    #[test]
    fn multiple_selective_same_lib() {
        let mut loaded = LoadedLibraries::new();
        loaded.push(LoadStatement {
            span: Span::new(0, 30),
            kind: LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            },
        });
        loaded.push(LoadStatement {
            span: Span::new(100, 35),
            kind: LoadKind::Selective {
                symbols: vec!["blocktrans".to_string()],
                library: "i18n".to_string(),
            },
        });
        let inventory = make_test_inventory();

        let middle = available_tags_at(&loaded, &inventory, 50);
        assert!(middle.has_tag("trans"));
        assert!(!middle.has_tag("blocktrans"));

        let after = available_tags_at(&loaded, &inventory, 200);
        assert!(after.has_tag("trans"));
        assert!(after.has_tag("blocktrans"));
    }
}
