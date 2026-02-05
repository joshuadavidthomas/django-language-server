use djls_source::Span;
use djls_templates::Node;
use rustc_hash::FxHashSet;

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
