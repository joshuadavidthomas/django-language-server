use std::collections::HashMap;
use std::collections::HashSet;

use djls_source::Span;
use djls_templates::TagBit;

use crate::project::TemplateLibraries;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadArgument {
    name: String,
    span: Span,
}

impl LoadArgument {
    #[must_use]
    pub fn new(name: String, span: Span) -> Self {
        Self { name, span }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }
}

impl From<&TagBit> for LoadArgument {
    fn from(bit: &TagBit) -> Self {
        Self::new(bit.as_str().to_string(), bit.span)
    }
}

impl From<&str> for LoadArgument {
    fn from(name: &str) -> Self {
        Self::new(name.to_string(), Span::new(0, 0))
    }
}

impl From<String> for LoadArgument {
    fn from(name: String) -> Self {
        Self::new(name, Span::new(0, 0))
    }
}

/// The kind of a `{% load %}` statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadKind {
    /// `{% load i18n %}` or `{% load i18n static %}` — loads entire libraries.
    FullLoad { libraries: Vec<LoadArgument> },
    /// `{% load trans blocktrans from i18n %}` — selectively imports named symbols.
    SelectiveImport {
        symbols: Vec<LoadArgument>,
        library: LoadArgument,
    },
}

impl LoadKind {
    /// Parse a `{% load %}` tag into a load kind.
    ///
    /// Returns `None` for non-`load` tags or malformed load bits.
    #[must_use]
    pub fn from_tag(name: &str, bits: &[TagBit]) -> Option<Self> {
        if name != "load" {
            return None;
        }

        parse_load_bits(bits)
    }
}

/// A parsed `{% load %}` tag with its source span and load kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoadStatement {
    span: Span,
    kind: LoadKind,
}

impl LoadStatement {
    #[must_use]
    pub(crate) fn new(span: Span, kind: LoadKind) -> Self {
        Self { span, kind }
    }

    /// Parse a `Node::Tag` into a load statement.
    ///
    /// Returns `None` for non-`load` tags or malformed load bits.
    #[must_use]
    pub(crate) fn from_tag(name: &str, bits: &[TagBit], span: Span) -> Option<Self> {
        Some(Self::new(span, LoadKind::from_tag(name, bits)?))
    }

    #[must_use]
    pub(crate) fn span(&self) -> Span {
        self.span
    }
}

/// Parse the bits from a `Node::Tag` where `name == "load"` into a `LoadKind`.
///
/// The bit slice contains only the bits after the load tag name — the tag name
/// itself is NOT included (the parser separates it into `Node::Tag::name`).
///
/// Returns `None` if the bits are empty or malformed (e.g. `{% load from %}`
/// with no symbols, or `{% load trans from %}` with no library).
///
/// # Examples
///
/// - `["i18n"]` → `FullLoad { libraries: ["i18n"] }`
/// - `["i18n", "static"]` → `FullLoad { libraries: ["i18n", "static"] }`
/// - `["trans", "from", "i18n"]` → `SelectiveImport { symbols: ["trans"], library: "i18n" }`
/// - `["trans", "blocktrans", "from", "i18n"]` → `SelectiveImport { symbols: ["trans", "blocktrans"], library: "i18n" }`
#[must_use]
fn parse_load_bits(bits: &[TagBit]) -> Option<LoadKind> {
    if bits.is_empty() {
        return None;
    }

    let from_idx = bits.iter().position(|bit| bit.as_str() == "from");

    match from_idx {
        Some(idx) => {
            let symbols: Vec<LoadArgument> = bits[..idx].iter().map(Into::into).collect();
            let rest = &bits[idx + 1..];

            if symbols.is_empty() || rest.len() != 1 {
                return None;
            }

            Some(LoadKind::SelectiveImport {
                symbols,
                library: (&rest[0]).into(),
            })
        }
        None => Some(LoadKind::FullLoad {
            libraries: bits.iter().map(Into::into).collect(),
        }),
    }
}

/// The state of loaded libraries at a given position in a template.
///
/// Borrows library and symbol names from the [`LoadedLibraries`] that produced
/// it, avoiding per-query string allocations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoadState<'a> {
    fully_loaded: HashSet<&'a str>,
    selective: HashMap<&'a str, HashSet<&'a str>>,
}

impl LoadState<'_> {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_fully_loaded(&self, library: &str) -> bool {
        self.fully_loaded.contains(library)
    }

    #[must_use]
    pub(crate) fn is_symbol_available(&self, library: &str, symbol: &str) -> bool {
        self.fully_loaded.contains(library)
            || self
                .selective
                .get(library)
                .is_some_and(|syms| syms.contains(symbol))
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn fully_loaded_libraries(&self) -> &HashSet<&str> {
        &self.fully_loaded
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn selective_imports(&self) -> &HashMap<&str, HashSet<&str>> {
        &self.selective
    }
}

/// An ordered collection of `LoadStatement` values extracted from a template.
///
/// Supports querying what libraries/symbols are available at a given byte
/// position by filtering load statements whose span ends before the query
/// position and applying state-machine semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LoadedLibraries {
    statements: Vec<LoadStatement>,
}

impl LoadedLibraries {
    #[must_use]
    pub(crate) fn new(statements: Vec<LoadStatement>) -> Self {
        Self { statements }
    }

    #[must_use]
    pub(crate) fn statements(&self) -> &[LoadStatement] {
        &self.statements
    }

    #[must_use]
    pub(crate) fn has_unknown_load_that_can_shadow_symbol_before(
        &self,
        position: u32,
        symbol: &str,
        template_libraries: &TemplateLibraries,
    ) -> bool {
        self.statements.iter().any(|stmt| {
            if stmt.span.end() > position {
                return false;
            }

            match &stmt.kind {
                LoadKind::FullLoad { libraries } => libraries
                    .iter()
                    .any(|library| !template_libraries.is_loadable_str(library.as_str())),
                LoadKind::SelectiveImport { symbols, library } => {
                    !template_libraries.is_loadable_str(library.as_str())
                        && symbols.iter().any(|loaded| loaded.as_str() == symbol)
                }
            }
        })
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
    ///
    /// The returned `LoadState` borrows library and symbol names from `self`,
    /// avoiding all string allocations.
    #[must_use]
    pub(crate) fn available_at(&self, position: u32) -> LoadState<'_> {
        let mut fully_loaded = HashSet::default();
        let mut selective: HashMap<&str, HashSet<&str>> = HashMap::default();

        for stmt in &self.statements {
            if stmt.span.end() > position {
                continue;
            }

            match &stmt.kind {
                LoadKind::FullLoad { libraries } => {
                    for lib in libraries {
                        fully_loaded.insert(lib.as_str());
                        selective.remove(lib.as_str());
                    }
                }
                LoadKind::SelectiveImport { symbols, library } => {
                    if !fully_loaded.contains(library.as_str()) {
                        let entry = selective.entry(library.as_str()).or_default();
                        for sym in symbols {
                            entry.insert(sym.as_str());
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

    fn bits(s: &str) -> Vec<TagBit> {
        s.split_whitespace()
            .map(|text| TagBit::new(text.to_string(), Span::new(0, 0)))
            .collect()
    }

    #[test]
    fn parse_full_load_single() {
        let result = parse_load_bits(&bits("i18n"));
        assert_eq!(
            result,
            Some(LoadKind::FullLoad {
                libraries: vec!["i18n".into()]
            })
        );
    }

    #[test]
    fn parse_full_load_multiple() {
        let result = parse_load_bits(&bits("i18n static"));
        assert_eq!(
            result,
            Some(LoadKind::FullLoad {
                libraries: vec!["i18n".into(), "static".into()]
            })
        );
    }

    #[test]
    fn parse_selective_import_single() {
        let result = parse_load_bits(&bits("trans from i18n"));
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
        let result = parse_load_bits(&bits("trans blocktrans from i18n"));
        assert_eq!(
            result,
            Some(LoadKind::SelectiveImport {
                symbols: vec!["trans".into(), "blocktrans".into()],
                library: "i18n".into(),
            })
        );
    }

    #[test]
    fn load_kind_requires_load_tag_name() {
        assert_eq!(LoadKind::from_tag("include", &bits("i18n")), None);
        assert_eq!(
            LoadKind::from_tag("load", &bits("i18n")),
            Some(LoadKind::FullLoad {
                libraries: vec!["i18n".into()]
            })
        );
    }

    #[test]
    fn load_statement_requires_load_tag_name() {
        assert_eq!(
            LoadStatement::from_tag("include", &bits("i18n"), Span::new(1, 5)),
            None
        );
        assert_eq!(
            LoadStatement::from_tag("load", &bits("i18n"), Span::new(1, 5)),
            Some(LoadStatement::new(
                Span::new(1, 5),
                LoadKind::FullLoad {
                    libraries: vec!["i18n".into()]
                }
            ))
        );
    }

    #[test]
    fn parse_empty_bits() {
        assert_eq!(parse_load_bits(&[]), None);
    }

    #[test]
    fn parse_load_from_no_symbols() {
        // {% load from i18n %} — "from" at index 0 means no symbols before it
        assert_eq!(parse_load_bits(&bits("from i18n")), None);
    }

    #[test]
    fn parse_load_from_no_library() {
        // {% load trans from %} — missing library after "from"
        assert_eq!(parse_load_bits(&bits("trans from")), None);
    }

    #[test]
    fn parse_load_from_multiple_libraries() {
        // {% load trans from i18n static %} — too many tokens after "from"
        assert_eq!(parse_load_bits(&bits("trans from i18n static")), None);
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
