//! Load statement resolution and symbol scoping.
//!
//! This module tracks `{% load %}` statements in a template and provides
//! position-aware queries for which tags/filters are available.

use djls_source::Span;
use djls_templates::Node;
use rustc_hash::FxHashSet;

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
}
