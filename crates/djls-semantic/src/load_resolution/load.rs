use std::collections::HashMap;
use std::collections::HashSet;

use djls_source::Span;

/// The kind of a `{% load %}` statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadKind {
    /// `{% load i18n %}` or `{% load i18n static %}` — loads entire libraries.
    FullLoad { libraries: Vec<String> },
    /// `{% load trans blocktrans from i18n %}` — selectively imports named symbols.
    SelectiveImport {
        symbols: Vec<String>,
        library: String,
    },
}

/// A parsed `{% load %}` tag with its source span and load kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadStatement {
    span: Span,
    kind: LoadKind,
}

impl LoadStatement {
    #[must_use]
    pub fn new(span: Span, kind: LoadKind) -> Self {
        Self { span, kind }
    }

    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }

    #[must_use]
    pub fn kind(&self) -> &LoadKind {
        &self.kind
    }
}

/// Parse the `bits` from a `Node::Tag` where `name == "load"` into a `LoadKind`.
///
/// Returns `None` if the bits are empty, don't start with `"load"`, or are
/// otherwise malformed (e.g. `{% load from %}` with no symbols).
///
/// # Examples
///
/// - `["load", "i18n"]` → `FullLoad { libraries: ["i18n"] }`
/// - `["load", "i18n", "static"]` → `FullLoad { libraries: ["i18n", "static"] }`
/// - `["load", "trans", "from", "i18n"]` → `SelectiveImport { symbols: ["trans"], library: "i18n" }`
/// - `["load", "trans", "blocktrans", "from", "i18n"]` → `SelectiveImport { symbols: ["trans", "blocktrans"], library: "i18n" }`
#[must_use]
pub fn parse_load_bits(bits: &[String]) -> Option<LoadKind> {
    // bits[0] is the tag name "load"
    if bits.is_empty() || bits[0] != "load" {
        return None;
    }

    let payload = &bits[1..];
    if payload.is_empty() {
        return None;
    }

    let from_idx = payload.iter().position(|b| b == "from");

    match from_idx {
        Some(idx) => {
            let symbols: Vec<String> = payload[..idx].to_vec();
            let rest = &payload[idx + 1..];

            // Must have at least one symbol and exactly one library after "from"
            if symbols.is_empty() || rest.len() != 1 {
                return None;
            }

            Some(LoadKind::SelectiveImport {
                symbols,
                library: rest[0].clone(),
            })
        }
        None => Some(LoadKind::FullLoad {
            libraries: payload.to_vec(),
        }),
    }
}

/// The state of loaded libraries at a given position in a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadState {
    fully_loaded: HashSet<String>,
    selective: HashMap<String, HashSet<String>>,
}

impl LoadState {
    #[must_use]
    pub fn is_fully_loaded(&self, library: &str) -> bool {
        self.fully_loaded.contains(library)
    }

    #[must_use]
    pub fn is_symbol_available(&self, library: &str, symbol: &str) -> bool {
        self.fully_loaded.contains(library)
            || self
                .selective
                .get(library)
                .is_some_and(|syms| syms.contains(symbol))
    }

    #[must_use]
    pub fn fully_loaded_libraries(&self) -> &HashSet<String> {
        &self.fully_loaded
    }

    #[must_use]
    pub fn selective_imports(&self) -> &HashMap<String, HashSet<String>> {
        &self.selective
    }
}

/// An ordered collection of `LoadStatement` values extracted from a template.
///
/// Supports querying what libraries/symbols are available at a given byte
/// position by filtering load statements whose span ends before the query
/// position and applying state-machine semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedLibraries {
    statements: Vec<LoadStatement>,
}

impl LoadedLibraries {
    #[must_use]
    pub fn new(statements: Vec<LoadStatement>) -> Self {
        Self { statements }
    }

    #[must_use]
    pub fn statements(&self) -> &[LoadStatement] {
        &self.statements
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    /// Compute the load state at a given byte position.
    ///
    /// Processes all load statements whose span ends before `position`,
    /// applying the following state-machine semantics:
    ///
    /// - `{% load X Y %}` (full load): add X, Y to `fully_loaded`, clear any
    ///   selective imports for those libraries
    /// - `{% load sym from X %}` (selective): if X is NOT fully loaded, add
    ///   `sym` to `selective[X]`
    #[must_use]
    pub fn available_at(&self, position: u32) -> LoadState {
        let mut fully_loaded = HashSet::default();
        let mut selective: HashMap<String, HashSet<String>> = HashMap::default();

        for stmt in &self.statements {
            if stmt.span.end() > position {
                continue;
            }

            match &stmt.kind {
                LoadKind::FullLoad { libraries } => {
                    for lib in libraries {
                        fully_loaded.insert(lib.clone());
                        selective.remove(lib);
                    }
                }
                LoadKind::SelectiveImport { symbols, library } => {
                    if !fully_loaded.contains(library) {
                        let entry = selective.entry(library.clone()).or_default();
                        for sym in symbols {
                            entry.insert(sym.clone());
                        }
                    }
                }
            }
        }

        LoadState {
            fully_loaded,
            selective,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parse_full_load_single() {
        let result = parse_load_bits(&bits("load i18n"));
        assert_eq!(
            result,
            Some(LoadKind::FullLoad {
                libraries: vec!["i18n".into()]
            })
        );
    }

    #[test]
    fn parse_full_load_multiple() {
        let result = parse_load_bits(&bits("load i18n static"));
        assert_eq!(
            result,
            Some(LoadKind::FullLoad {
                libraries: vec!["i18n".into(), "static".into()]
            })
        );
    }

    #[test]
    fn parse_selective_import_single() {
        let result = parse_load_bits(&bits("load trans from i18n"));
        assert_eq!(
            result,
            Some(LoadKind::SelectiveImport {
                symbols: vec!["trans".into()],
                library: "i18n".into(),
            })
        );
    }

    #[test]
    fn parse_selective_import_multiple() {
        let result = parse_load_bits(&bits("load trans blocktrans from i18n"));
        assert_eq!(
            result,
            Some(LoadKind::SelectiveImport {
                symbols: vec!["trans".into(), "blocktrans".into()],
                library: "i18n".into(),
            })
        );
    }

    #[test]
    fn parse_empty_bits() {
        assert_eq!(parse_load_bits(&[]), None);
    }

    #[test]
    fn parse_not_load_tag() {
        assert_eq!(parse_load_bits(&bits("if condition")), None);
    }

    #[test]
    fn parse_load_no_args() {
        assert_eq!(parse_load_bits(&bits("load")), None);
    }

    #[test]
    fn parse_load_from_no_symbols() {
        // {% load from i18n %} — no symbols before "from"
        assert_eq!(parse_load_bits(&bits("load from i18n")), None);
    }

    #[test]
    fn parse_load_from_no_library() {
        // {% load trans from %} — missing library after "from"
        assert_eq!(parse_load_bits(&bits("load trans from")), None);
    }

    #[test]
    fn parse_load_from_multiple_libraries() {
        // {% load trans from i18n static %} — too many tokens after "from"
        assert_eq!(parse_load_bits(&bits("load trans from i18n static")), None);
    }

    // --- LoadedLibraries tests ---

    fn make_load(span: (u32, u32), kind: LoadKind) -> LoadStatement {
        LoadStatement::new(Span::new(span.0, span.1), kind)
    }

    #[test]
    fn available_at_no_loads() {
        let libs = LoadedLibraries::new(vec![]);
        let state = libs.available_at(100);
        assert!(state.fully_loaded_libraries().is_empty());
        assert!(state.selective_imports().is_empty());
    }

    #[test]
    fn available_at_full_load_before_position() {
        let libs = LoadedLibraries::new(vec![make_load(
            (10, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // Position after load ends (span end = 10 + 20 = 30)
        let state = libs.available_at(50);
        assert!(state.is_fully_loaded("i18n"));
    }

    #[test]
    fn available_at_full_load_after_position() {
        let libs = LoadedLibraries::new(vec![make_load(
            (10, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // Position before load ends (span end = 30)
        let state = libs.available_at(15);
        assert!(!state.is_fully_loaded("i18n"));
    }

    #[test]
    fn available_at_selective_import() {
        let libs = LoadedLibraries::new(vec![make_load(
            (10, 20),
            LoadKind::SelectiveImport {
                symbols: vec!["trans".into()],
                library: "i18n".into(),
            },
        )]);

        let state = libs.available_at(50);
        assert!(!state.is_fully_loaded("i18n"));
        assert!(state.is_symbol_available("i18n", "trans"));
        assert!(!state.is_symbol_available("i18n", "blocktrans"));
    }

    #[test]
    fn selective_then_full_load() {
        let libs = LoadedLibraries::new(vec![
            make_load(
                (10, 20),
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

        // After selective, before full
        let state = libs.available_at(40);
        assert!(!state.is_fully_loaded("i18n"));
        assert!(state.is_symbol_available("i18n", "trans"));

        // After full load — fully loaded, selective cleared
        let state = libs.available_at(80);
        assert!(state.is_fully_loaded("i18n"));
        assert!(state.is_symbol_available("i18n", "trans"));
        assert!(state.is_symbol_available("i18n", "blocktrans"));
        assert!(state.selective_imports().get("i18n").is_none());
    }

    #[test]
    fn full_load_then_selective_is_noop() {
        let libs = LoadedLibraries::new(vec![
            make_load(
                (10, 20),
                LoadKind::FullLoad {
                    libraries: vec!["i18n".into()],
                },
            ),
            make_load(
                (50, 20),
                LoadKind::SelectiveImport {
                    symbols: vec!["trans".into()],
                    library: "i18n".into(),
                },
            ),
        ]);

        // After both — library still fully loaded, selective ignored
        let state = libs.available_at(80);
        assert!(state.is_fully_loaded("i18n"));
        assert!(state.selective_imports().get("i18n").is_none());
    }

    #[test]
    fn multiple_selective_imports_accumulate() {
        let libs = LoadedLibraries::new(vec![
            make_load(
                (10, 30),
                LoadKind::SelectiveImport {
                    symbols: vec!["trans".into()],
                    library: "i18n".into(),
                },
            ),
            make_load(
                (50, 40),
                LoadKind::SelectiveImport {
                    symbols: vec!["blocktrans".into()],
                    library: "i18n".into(),
                },
            ),
        ]);

        let state = libs.available_at(100);
        assert!(!state.is_fully_loaded("i18n"));
        assert!(state.is_symbol_available("i18n", "trans"));
        assert!(state.is_symbol_available("i18n", "blocktrans"));
    }

    #[test]
    fn position_at_exact_span_end() {
        // Span: start=10, length=20, so end=30
        // Position exactly at 30 should NOT include this load
        // (we use > not >=, so end == position means it IS included)
        let libs = LoadedLibraries::new(vec![make_load(
            (10, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // At exactly end position (30) — the load's span ends at 30,
        // and 30 > 30 is false, so it IS included
        let state = libs.available_at(30);
        assert!(state.is_fully_loaded("i18n"));

        // Just before end (29) — 30 > 29 is true, so NOT included
        let state = libs.available_at(29);
        assert!(!state.is_fully_loaded("i18n"));
    }
}
