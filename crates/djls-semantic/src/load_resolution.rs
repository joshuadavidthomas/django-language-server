//! Load statement resolution and symbol scoping.
//!
//! This module tracks `{% load %}` statements in a template and provides
//! position-aware queries for which tags/filters are available.

use crate::errors::ValidationError;
use crate::ValidationErrorAccumulator;
use djls_project::FilterProvenance;
use djls_project::InspectorInventory;
use djls_project::TagProvenance;
use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use salsa::Accumulator;

/// A parsed `{% load %}` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadStatement {
    /// The span of the `{% load %}` tag (for diagnostics and ordering)
    pub span: Span,
    /// The kind of load: full library or selective import
    pub kind: LoadKind,
}

/// The kind of load statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadKind {
    /// Load entire libraries: `{% load i18n static %}`
    Libraries(Vec<String>),
    /// Selective import: `{% load trans blocktrans from i18n %}`
    Selective {
        /// The symbols being imported
        symbols: Vec<String>,
        /// The library they come from
        library: String,
    },
}

/// Collection of load statements in a template, ordered by position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedLibraries {
    /// Load statements in document order
    loads: Vec<LoadStatement>,
}

impl LoadedLibraries {
    /// Create an empty `LoadedLibraries`.
    #[must_use]
    pub fn new() -> Self {
        Self { loads: Vec::new() }
    }

    /// Add a load statement.
    pub fn push(&mut self, statement: LoadStatement) {
        self.loads.push(statement);
    }

    /// Get all load statements.
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
            // Only include loads that END before the position
            // (tag must be fully parsed before it takes effect)
            if stmt.span.end() <= position {
                match &stmt.kind {
                    LoadKind::Libraries(names) => {
                        libs.extend(names.iter().cloned());
                    }
                    LoadKind::Selective { library, .. } => {
                        // For selective imports, we track the library
                        // but symbols_before() handles the actual filtering
                        libs.insert(library.clone());
                    }
                }
            }
        }
        libs
    }

    /// Get specific symbols available from selective imports before a position.
    ///
    /// Returns a map of `symbol_name` → `library_name` for selective imports only.
    /// Full library loads are handled separately via `libraries_before()`.
    #[must_use]
    pub fn selective_symbols_before(&self, position: u32) -> FxHashSet<(String, String)> {
        let mut symbols = FxHashSet::default();
        for stmt in &self.loads {
            if stmt.span.end() <= position {
                if let LoadKind::Selective { symbols: syms, library } = &stmt.kind {
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
/// - `{% load lib1 lib2 %}` → `LoadKind::Libraries`([`lib1`, `lib2`])
/// - `{% load sym1 sym2 from lib %}` → `LoadKind::Selective` { symbols: [`sym1`, `sym2`], library: `lib` }
#[must_use]
pub fn parse_load_bits(bits: &[String], span: Span) -> Option<LoadStatement> {
    if bits.is_empty() {
        return None;
    }

    // Check for "from" syntax: {% load symbol1 symbol2 from library %}
    if let Some(from_idx) = bits.iter().position(|b| b == "from") {
        // Everything before "from" are symbols, everything after is the library
        if from_idx == 0 || from_idx + 1 >= bits.len() {
            // Invalid: "{% load from lib %}" or "{% load x from %}"
            return None;
        }

        let symbols: Vec<String> = bits[..from_idx].to_vec();
        // Only the first token after "from" is the library name
        let library = bits[from_idx + 1].clone();

        return Some(LoadStatement {
            span,
            kind: LoadKind::Selective { symbols, library },
        });
    }

    // Standard form: {% load lib1 lib2 %}
    let libraries: Vec<String> = bits.to_vec();
    Some(LoadStatement {
        span,
        kind: LoadKind::Libraries(libraries),
    })
}

/// Extract all `{% load %}` statements from a template.
///
/// This tracked function performs a traversal of the nodelist,
/// collecting all load statements in document order. This is important because
/// Django's parser processes tokens in order as it parses, so a `{% load %}`
/// inside a block still affects global tag availability.
///
/// **IMPORTANT**: The nodelist in djls-templates is flat (no nested structure),
/// but we must still process ALL nodes. If the parser ever changes to support
/// nested structures, this function must be updated to traverse recursively.
#[salsa::tracked]
pub fn compute_loaded_libraries(
    db: &dyn crate::Db,
    nodelist: djls_templates::NodeList<'_>,
) -> LoadedLibraries {
    let mut loaded = LoadedLibraries::new();
    let mut load_spans: Vec<(Span, LoadStatement)> = Vec::new();

    // Collect all load statements with their spans
    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            if name == "load" {
                if let Some(stmt) = parse_load_bits(bits, *span) {
                    load_spans.push((*span, stmt));
                }
            }
        }
    }

    // Sort by span start position to ensure document order
    // (The nodelist should already be in order, but sort to be safe)
    load_spans.sort_by_key(|(span, _)| span.start());

    // Add to LoadedLibraries in order
    for (_, stmt) in load_spans {
        loaded.push(stmt);
    }

    loaded
}

/// Result of querying available symbols at a position.
#[derive(Debug, Clone, Default)]
pub struct AvailableSymbols {
    /// Tag names available at this position
    pub tags: FxHashSet<String>,
}

impl AvailableSymbols {
    /// Check if a tag name is available
    #[must_use]
    pub fn has_tag(&self, name: &str) -> bool {
        self.tags.contains(name)
    }
}

/// Load state at a given position, computed by processing loads in order.
///
/// This uses a state-machine approach to correctly handle the interaction
/// between selective imports and full library loads:
/// - `{% load trans from i18n %}` adds "trans" to `selective[i18n]`
/// - `{% load i18n %}` adds "i18n" to `fully_loaded` AND clears `selective[i18n]`
///
/// This ensures that a later full load overrides earlier selective imports.
#[derive(Debug, Clone, Default)]
struct LoadState {
    /// Libraries that have been fully loaded (all tags available)
    fully_loaded: FxHashSet<String>,
    /// Selectively imported symbols: library → set of symbol names
    /// Only contains entries for libraries NOT in `fully_loaded`
    selective: FxHashMap<String, FxHashSet<String>>,
}

impl LoadState {
    /// Process a load statement, updating state accordingly.
    fn process(&mut self, stmt: &LoadStatement) {
        match &stmt.kind {
            LoadKind::Libraries(libs) => {
                for lib in libs {
                    // Full load: add to fully_loaded, clear any selective imports
                    self.fully_loaded.insert(lib.clone());
                    self.selective.remove(lib);
                }
            }
            LoadKind::Selective { symbols, library } => {
                // Only add selective imports if library not already fully loaded
                if !self.fully_loaded.contains(library) {
                    let entry = self.selective.entry(library.clone()).or_default();
                    entry.extend(symbols.iter().cloned());
                }
            }
        }
    }

    /// Check if a tag from a library is available.
    fn is_tag_available(&self, tag_name: &str, library: &str) -> bool {
        // Available if library is fully loaded
        if self.fully_loaded.contains(library) {
            return true;
        }
        // Or if specifically imported via selective import
        if let Some(symbols) = self.selective.get(library) {
            return symbols.contains(tag_name);
        }
        false
    }
}

/// Determine which tags are available at a given position in the template.
///
/// This function uses a **state-machine approach** that processes load
/// statements in document order up to `position`:
///
/// 1. For `{% load lib1 lib2 %}`: add libraries to "fully loaded" set
///    and clear any selective imports for those libraries
/// 2. For `{% load sym from lib %}`: if lib not fully loaded, add sym
///    to selective imports for lib
///
/// A library tag is available iff:
/// - Its library is in the `fully_loaded` set, OR
/// - The tag name is in the selective imports for its library
///
/// This correctly handles `{% load trans from i18n %}` followed by
/// `{% load i18n %}` — after the second load, ALL i18n tags are available.
///
/// # Arguments
/// * `loaded` - The `LoadedLibraries` computed from the template
/// * `inventory` - The `InspectorInventory` from inspector (M4+ unified shape)
/// * `position` - The byte offset to check availability at
///
/// # Returns
/// The set of tag names available at the position.
#[must_use]
pub fn available_tags_at(
    loaded: &LoadedLibraries,
    inventory: &InspectorInventory,
    position: u32,
) -> AvailableSymbols {
    let mut available = AvailableSymbols::default();

    // Build load state by processing statements in order up to position
    let mut state = LoadState::default();
    for stmt in loaded.loads() {
        // Only include loads that END before the position
        // (tag must be fully parsed before it takes effect)
        if stmt.span.end() <= position {
            state.process(stmt);
        }
    }

    // Determine available tags
    for tag in inventory.tags() {
        match tag.provenance() {
            TagProvenance::Builtin { .. } => {
                // Builtins are always available
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
    /// Filter names available at this position
    pub filters: FxHashSet<String>,
}

impl AvailableFilters {
    /// Check if a filter name is available
    #[must_use]
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.contains(name)
    }
}

/// Determine which filters are available at a given position in the template.
///
/// This function mirrors `available_tags_at()` but operates on filters.
/// It uses the same state-machine approach to process load statements
/// in document order up to `position`.
///
/// A library filter is available iff:
/// - Its library is in the `fully_loaded` set, OR
/// - The filter name is in the selective imports for its library
///
/// Builtin filters are always available.
///
/// # Arguments
/// * `loaded` - The `LoadedLibraries` computed from the template
/// * `inventory` - The `InspectorInventory` from inspector (M4+ unified shape)
/// * `position` - The byte offset to check availability at
///
/// # Returns
/// The set of filter names available at the position.
#[must_use]
pub fn available_filters_at(
    loaded: &LoadedLibraries,
    inventory: &InspectorInventory,
    position: u32,
) -> AvailableFilters {
    let mut available = AvailableFilters::default();

    // Build load state by processing statements in order up to position
    let mut state = LoadState::default();
    for stmt in loaded.loads() {
        // Only include loads that END before the position
        // (tag must be fully parsed before it takes effect)
        if stmt.span.end() <= position {
            state.process(stmt);
        }
    }

    // Determine available filters
    for filter in inventory.filters() {
        match filter.provenance() {
            FilterProvenance::Builtin { .. } => {
                // Builtins are always available
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

/// Entry for a tag in the inventory lookup.
///
/// We use Vec<String> for libraries because multiple libraries can define
/// the same tag name (collision). We need all candidates for proper error messages.
#[derive(Debug, Clone)]
enum TagInventoryEntry {
    /// Tag is a builtin (always available)
    Builtin,
    /// Tag requires loading one of these libraries
    Libraries(Vec<String>),
}

/// Entry for a filter in the inventory lookup.
///
/// Mirrors `TagInventoryEntry` for filter collision handling.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub enum FilterInventoryEntry {
    /// Filter is a builtin (always available)
    Builtin,
    /// Filter requires loading one of these libraries
    Libraries(Vec<String>),
}

/// Build a lookup from tag name to inventory entry.
///
/// **IMPORTANT**: This handles the case where multiple libraries define the
/// same tag name by collecting ALL candidate libraries, not just the first one.
/// This is essential for correct error messages.
fn build_tag_inventory(inventory: &InspectorInventory) -> FxHashMap<String, TagInventoryEntry> {
    let mut result: FxHashMap<String, TagInventoryEntry> = FxHashMap::default();

    for tag in inventory.tags() {
        let name = tag.name().to_string();
        match tag.provenance() {
            TagProvenance::Builtin { .. } => {
                // Builtin takes precedence (always available)
                result.insert(name, TagInventoryEntry::Builtin);
            }
            TagProvenance::Library { load_name, .. } => {
                // Add library to list of candidates for this tag
                if let Some(entry) = result.get_mut(&name) {
                    // Don't override Builtin
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

/// Build a lookup from filter name to inventory entry.
///
/// Mirrors `build_tag_inventory` for filter collision handling.
pub fn build_filter_inventory(
    inventory: &InspectorInventory,
) -> FxHashMap<String, FilterInventoryEntry> {
    let mut result: FxHashMap<String, FilterInventoryEntry> = FxHashMap::default();

    for filter in inventory.filters() {
        let name = filter.name().to_string();
        match filter.provenance() {
            FilterProvenance::Builtin { .. } => {
                // Builtin takes precedence (always available)
                result.insert(name, FilterInventoryEntry::Builtin);
            }
            FilterProvenance::Library { load_name, .. } => {
                // Add library to list of candidates for this filter
                if let Some(entry) = result.get_mut(&name) {
                    // Don't override Builtin
                    if let FilterInventoryEntry::Libraries(libs) = entry {
                        if !libs.contains(load_name) {
                            libs.push(load_name.clone());
                        }
                    }
                } else {
                    result.insert(name, FilterInventoryEntry::Libraries(vec![load_name.clone()]));
                }
            }
        }
    }

    result
}

/// Validate that all tags in the template are either builtins or from loaded libraries.
///
/// This function:
/// 1. Computes which libraries are loaded at each position
/// 2. For each tag, checks if it's available at that position
/// 3. Accumulates errors for unknown or unloaded tags
///
/// # Behavior
/// - If inventory is None (inspector unavailable), skip validation entirely
/// - Builtins are always valid
/// - Library tags require their library to be loaded before use
///
/// # Collision Handling
/// When multiple libraries define the same tag name:
/// - If ONE of them is loaded, the tag is valid (Django resolves at runtime)
/// - If NONE are loaded, emit S110 (`AmbiguousUnloadedTag`) listing all candidates
///
/// # `TagSpecs` Interaction
/// Tags with structural specs (openers, closers, intermediates) are skipped:
/// - They're validated by block structure (S100-S103) and argument validation (S117)
/// - This prevents emitting S108 for "endif" when inspector doesn't list it
/// - Closers like "endif" derive from opener spec, not inventory
#[salsa::tracked]
pub fn validate_tag_scoping(
    db: &dyn crate::Db,
    nodelist: djls_templates::NodeList<'_>,
) {
    // Get inventory - if unavailable, skip validation
    let Some(inventory) = db.inspector_inventory() else {
        tracing::debug!("Inspector inventory unavailable, skipping tag scoping validation");
        return;
    };

    // Compute load state
    let loaded = compute_loaded_libraries(db, nodelist);

    // Build lookup with collision handling
    let tag_inventory = build_tag_inventory(inventory);

    // Validate each tag
    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, span, .. } = node {
            // Skip the load tag itself
            if name == "load" {
                continue;
            }

            // Skip tags that have structural meaning via TagSpecs
            // These are validated by block structure validation (S100-S103) and
            // argument validation (S117), not by load scoping.
            let tag_specs = db.tag_specs();
            let has_spec = tag_specs.get(name).is_some()
                || tag_specs.get_end_spec_for_closer(name).is_some()
                || tag_specs.is_intermediate(name);

            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

            match tag_inventory.get(name) {
                None => {
                    // Tag not in inventory at all
                    if !has_spec {
                        ValidationErrorAccumulator(ValidationError::UnknownTag {
                            tag: name.clone(),
                            span: marker_span,
                        })
                        .accumulate(db);
                    }
                }
                Some(TagInventoryEntry::Builtin) => {
                    // Builtins always valid
                }
                Some(TagInventoryEntry::Libraries(candidate_libs)) => {
                    // Check if tag is available at this position
                    let available = available_tags_at(&loaded, inventory, span.start());

                    if !available.has_tag(name) {
                        // Tag not available - emit appropriate error
                        if candidate_libs.len() == 1 {
                            // Single library - simple error message
                            ValidationErrorAccumulator(ValidationError::UnloadedLibraryTag {
                                tag: name.clone(),
                                library: candidate_libs[0].clone(),
                                span: marker_span,
                            })
                            .accumulate(db);
                        } else {
                            // Multiple libraries define this tag - ambiguous
                            // Per charter: "don't guess", list all candidates
                            ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedTag {
                                tag: name.clone(),
                                libraries: candidate_libs.clone(),
                                span: marker_span,
                            })
                            .accumulate(db);
                        }
                    }
                    // If available, the tag is valid (even with collisions,
                    // Django resolves at runtime based on load order)
                }
            }
        }
    }
}

/// Validate that all filters in the template are either builtins or from loaded libraries.
///
/// This function:
/// 1. Computes which libraries are loaded at each position
/// 2. For each filter in variable expressions, checks if it's available at that position
/// 3. Accumulates errors for unknown or unloaded filters
///
/// # Behavior
/// - If inventory is None (inspector unavailable), skip validation entirely
/// - Builtin filters are always valid
/// - Library filters require their library to be loaded before use
///
/// # Collision Handling
/// When multiple libraries define the same filter name:
/// - If ONE of them is loaded, the filter is valid (Django resolves at runtime)
/// - If NONE are loaded, emit S113 (`AmbiguousUnloadedFilter`) listing all candidates
#[salsa::tracked]
pub fn validate_filter_scoping(db: &dyn crate::Db, nodelist: djls_templates::NodeList<'_>) {
    // Get inventory - if unavailable, skip validation
    let Some(inventory) = db.inspector_inventory() else {
        tracing::debug!("Inspector inventory unavailable, skipping filter scoping validation");
        return;
    };

    // Compute load state
    let loaded = compute_loaded_libraries(db, nodelist);

    // Build lookup with collision handling
    let filter_inventory = build_filter_inventory(inventory);

    // Validate each variable node's filters
    for node in nodelist.nodelist(db) {
        if let Node::Variable { filters, .. } = node {
            for filter in filters {
                validate_single_filter(db, filter, &filter_inventory, &loaded, inventory);
            }
        }
    }
}

/// Validate a single filter against the inventory and load state.
fn validate_single_filter(
    db: &dyn crate::Db,
    filter: &djls_templates::Filter,
    filter_lookup: &FxHashMap<String, FilterInventoryEntry>,
    loaded: &LoadedLibraries,
    inventory: &InspectorInventory,
) {
    let name = &filter.name;
    let span = filter.name_span();

    match filter_lookup.get(name) {
        None => {
            // Filter not in inventory at all
            ValidationErrorAccumulator(ValidationError::UnknownFilter {
                filter: name.clone(),
                span,
            })
            .accumulate(db);
        }
        Some(FilterInventoryEntry::Builtin) => {
            // Builtins always valid
        }
        Some(FilterInventoryEntry::Libraries(candidate_libs)) => {
            let available = available_filters_at(loaded, inventory, filter.span.start());

            if !available.has_filter(name) {
                if candidate_libs.len() == 1 {
                    // Single library - simple error message
                    ValidationErrorAccumulator(ValidationError::UnloadedLibraryFilter {
                        filter: name.clone(),
                        library: candidate_libs[0].clone(),
                        span,
                    })
                    .accumulate(db);
                } else {
                    // Multiple libraries define this filter - ambiguous
                    ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedFilter {
                        filter: name.clone(),
                        libraries: candidate_libs.clone(),
                        span,
                    })
                    .accumulate(db);
                }
            }
            // If available, the filter is valid (even with collisions,
            // Django resolves at runtime based on load order)
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
    fn test_parse_load_invalid_from() {
        // "{% load from i18n %}" - no symbols before from
        let bits = vec!["from".to_string(), "i18n".to_string()];
        let span = Span::new(0, 15);

        assert!(parse_load_bits(&bits, span).is_none());
    }

    #[test]
    fn test_libraries_before_position() {
        let mut libs = LoadedLibraries::new();

        // {% load i18n %} at position 0-15
        libs.push(LoadStatement {
            span: Span::new(0, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });

        // {% load static %} at position 50-68
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

        // {% load trans from i18n %} at position 0-25
        libs.push(LoadStatement {
            span: Span::new(0, 25),
            kind: LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            },
        });

        let symbols = libs.selective_symbols_before(50);
        assert!(symbols.contains(&("trans".to_string(), "i18n".to_string())));
    }

    mod availability_tests {
        use super::*;

        fn make_test_inventory() -> InspectorInventory {
            InspectorInventory::new(
                std::collections::HashMap::from([
                    ("i18n".to_string(), "django.templatetags.i18n".to_string()),
                    ("static".to_string(), "django.templatetags.static".to_string()),
                ]),
                vec!["django.template.defaulttags".to_string()],
                vec![
                    // Builtin
                    djls_project::TemplateTag::new_builtin(
                        "if",
                        "django.template.defaulttags",
                        None,
                    ),
                    // Library tags
                    djls_project::TemplateTag::new_library(
                        "trans",
                        "i18n",
                        "django.templatetags.i18n",
                        None,
                    ),
                    djls_project::TemplateTag::new_library(
                        "blocktrans",
                        "i18n",
                        "django.templatetags.i18n",
                        None,
                    ),
                    djls_project::TemplateTag::new_library(
                        "static",
                        "static",
                        "django.templatetags.static",
                        None,
                    ),
                ],
                vec![], // no filters
            )
        }

        #[test]
        fn test_builtins_always_available() {
            let loaded = LoadedLibraries::new();
            let inventory = make_test_inventory();

            let available = available_tags_at(&loaded, &inventory, 0);

            assert!(available.has_tag("if"), "Builtins should always be available");
            assert!(
                !available.has_tag("trans"),
                "Library tags should NOT be available without load"
            );
        }

        #[test]
        fn test_library_tag_after_load() {
            let mut loaded = LoadedLibraries::new();
            loaded.push(LoadStatement {
                span: Span::new(0, 15), // {% load i18n %}
                kind: LoadKind::Libraries(vec!["i18n".to_string()]),
            });
            let inventory = make_test_inventory();

            // Before the load tag ends
            let before = available_tags_at(&loaded, &inventory, 5);
            assert!(
                !before.has_tag("trans"),
                "trans should not be available inside load tag"
            );

            // After the load tag
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
        fn test_selective_import() {
            let mut loaded = LoadedLibraries::new();
            loaded.push(LoadStatement {
                span: Span::new(0, 30), // {% load trans from i18n %}
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
        fn test_selective_then_full_load() {
            let mut loaded = LoadedLibraries::new();
            // First: {% load trans from i18n %}
            loaded.push(LoadStatement {
                span: Span::new(0, 30),
                kind: LoadKind::Selective {
                    symbols: vec!["trans".to_string()],
                    library: "i18n".to_string(),
                },
            });
            // Later: {% load i18n %}
            loaded.push(LoadStatement {
                span: Span::new(100, 15),
                kind: LoadKind::Libraries(vec!["i18n".to_string()]),
            });
            let inventory = make_test_inventory();

            // After selective, before full
            let middle = available_tags_at(&loaded, &inventory, 50);
            assert!(middle.has_tag("trans"));
            assert!(!middle.has_tag("blocktrans"));

            // After full load - THIS IS THE KEY TEST
            // The state-machine approach ensures full load clears selective state
            let after = available_tags_at(&loaded, &inventory, 150);
            assert!(after.has_tag("trans"), "trans still available after full load");
            assert!(
                after.has_tag("blocktrans"),
                "blocktrans NOW available after full load"
            );
        }

        #[test]
        fn test_full_then_selective_no_effect() {
            let mut loaded = LoadedLibraries::new();
            // First: {% load i18n %}
            loaded.push(LoadStatement {
                span: Span::new(0, 15),
                kind: LoadKind::Libraries(vec!["i18n".to_string()]),
            });
            // Later: {% load trans from i18n %} - should be no-op since lib already loaded
            loaded.push(LoadStatement {
                span: Span::new(100, 30),
                kind: LoadKind::Selective {
                    symbols: vec!["trans".to_string()],
                    library: "i18n".to_string(),
                },
            });
            let inventory = make_test_inventory();

            // After both loads - all i18n tags should still be available
            let after = available_tags_at(&loaded, &inventory, 200);
            assert!(after.has_tag("trans"));
            assert!(
                after.has_tag("blocktrans"),
                "Full load takes precedence"
            );
        }

        #[test]
        fn test_multiple_selective_same_lib() {
            let mut loaded = LoadedLibraries::new();
            // First: {% load trans from i18n %}
            loaded.push(LoadStatement {
                span: Span::new(0, 30),
                kind: LoadKind::Selective {
                    symbols: vec!["trans".to_string()],
                    library: "i18n".to_string(),
                },
            });
            // Later: {% load blocktrans from i18n %}
            loaded.push(LoadStatement {
                span: Span::new(100, 35),
                kind: LoadKind::Selective {
                    symbols: vec!["blocktrans".to_string()],
                    library: "i18n".to_string(),
                },
            });
            let inventory = make_test_inventory();

            // After first selective
            let middle = available_tags_at(&loaded, &inventory, 50);
            assert!(middle.has_tag("trans"));
            assert!(!middle.has_tag("blocktrans"));

            // After second selective - both should be available
            let after = available_tags_at(&loaded, &inventory, 200);
            assert!(after.has_tag("trans"));
            assert!(after.has_tag("blocktrans"));
        }
    }

    mod filter_availability_tests {
        use super::*;

        fn make_test_inventory_with_filters() -> InspectorInventory {
            InspectorInventory::new(
                std::collections::HashMap::from([
                    ("i18n".to_string(), "django.templatetags.i18n".to_string()),
                    ("humanize".to_string(), "django.contrib.humanize.templatetags.humanize".to_string()),
                ]),
                vec!["django.template.defaultfilters".to_string()],
                vec![], // no tags
                vec![
                    // Builtin filters
                    djls_project::TemplateFilter::new_builtin(
                        "title",
                        "django.template.defaultfilters",
                        None,
                    ),
                    djls_project::TemplateFilter::new_builtin(
                        "upper",
                        "django.template.defaultfilters",
                        None,
                    ),
                    djls_project::TemplateFilter::new_builtin(
                        "default",
                        "django.template.defaultfilters",
                        None,
                    ),
                    // Library filters
                    djls_project::TemplateFilter::new_library(
                        "localize",
                        "i18n",
                        "django.templatetags.i18n",
                        None,
                    ),
                    djls_project::TemplateFilter::new_library(
                        "unlocalize",
                        "i18n",
                        "django.templatetags.i18n",
                        None,
                    ),
                    djls_project::TemplateFilter::new_library(
                        "intcomma",
                        "humanize",
                        "django.contrib.humanize.templatetags.humanize",
                        None,
                    ),
                ],
            )
        }

        #[test]
        fn test_builtin_filters_always_available() {
            let loaded = LoadedLibraries::new();
            let inventory = make_test_inventory_with_filters();

            let available = available_filters_at(&loaded, &inventory, 0);

            assert!(available.has_filter("title"), "Builtin filters should always be available");
            assert!(available.has_filter("upper"), "Builtin filters should always be available");
            assert!(available.has_filter("default"), "Builtin filters should always be available");
            assert!(
                !available.has_filter("localize"),
                "Library filters should NOT be available without load"
            );
        }

        #[test]
        fn test_library_filter_after_load() {
            let mut loaded = LoadedLibraries::new();
            loaded.push(LoadStatement {
                span: Span::new(0, 15), // {% load i18n %}
                kind: LoadKind::Libraries(vec!["i18n".to_string()]),
            });
            let inventory = make_test_inventory_with_filters();

            // Before the load tag ends
            let before = available_filters_at(&loaded, &inventory, 5);
            assert!(
                !before.has_filter("localize"),
                "localize should not be available inside load tag"
            );

            // After the load tag
            let after = available_filters_at(&loaded, &inventory, 20);
            assert!(after.has_filter("localize"), "localize should be available after load");
            assert!(
                after.has_filter("unlocalize"),
                "unlocalize should be available after load"
            );
            assert!(
                !after.has_filter("intcomma"),
                "intcomma should NOT be available (not loaded)"
            );
        }

        #[test]
        fn test_selective_filter_import() {
            let mut loaded = LoadedLibraries::new();
            loaded.push(LoadStatement {
                span: Span::new(0, 35), // {% load localize from i18n %}
                kind: LoadKind::Selective {
                    symbols: vec!["localize".to_string()],
                    library: "i18n".to_string(),
                },
            });
            let inventory = make_test_inventory_with_filters();

            let available = available_filters_at(&loaded, &inventory, 50);

            assert!(
                available.has_filter("localize"),
                "localize should be available (selectively imported)"
            );
            assert!(
                !available.has_filter("unlocalize"),
                "unlocalize should NOT be available (not in selective import)"
            );
        }

        #[test]
        fn test_selective_then_full_load_filter() {
            let mut loaded = LoadedLibraries::new();
            // First: {% load localize from i18n %}
            loaded.push(LoadStatement {
                span: Span::new(0, 35),
                kind: LoadKind::Selective {
                    symbols: vec!["localize".to_string()],
                    library: "i18n".to_string(),
                },
            });
            // Later: {% load i18n %}
            loaded.push(LoadStatement {
                span: Span::new(100, 15),
                kind: LoadKind::Libraries(vec!["i18n".to_string()]),
            });
            let inventory = make_test_inventory_with_filters();

            // After selective, before full
            let middle = available_filters_at(&loaded, &inventory, 50);
            assert!(middle.has_filter("localize"));
            assert!(!middle.has_filter("unlocalize"));

            // After full load - THIS IS THE KEY TEST
            let after = available_filters_at(&loaded, &inventory, 150);
            assert!(after.has_filter("localize"), "localize still available after full load");
            assert!(
                after.has_filter("unlocalize"),
                "unlocalize NOW available after full load"
            );
        }

        #[test]
        fn test_filter_inventory_entry_collision() {
            let inventory = make_test_inventory_with_filters();
            let lookup = build_filter_inventory(&inventory);

            // Builtin filters should be Builtin entry
            assert!(
                matches!(lookup.get("title"), Some(FilterInventoryEntry::Builtin)),
                "title should be Builtin"
            );

            // Library filters should be Libraries entry
            assert!(
                matches!(lookup.get("localize"), Some(FilterInventoryEntry::Libraries(_))),
                "localize should be Libraries"
            );

            // Check the library name is correct
            if let Some(FilterInventoryEntry::Libraries(libs)) = lookup.get("localize") {
                assert!(libs.contains(&"i18n".to_string()));
            }
        }
    }
}
