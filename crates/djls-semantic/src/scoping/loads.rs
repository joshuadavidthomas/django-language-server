use std::collections::BTreeMap;

use djls_project::LoadableLibraryLookup;
use djls_source::Span;
use djls_templates::TagBit;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    /// Parse arguments after the occurrence has already been resolved to the
    /// Template Library loader role. This avoids coupling semantics to the
    /// conventional `load` spelling.
    #[must_use]
    pub(crate) fn from_loader_bits(bits: &[TagBit]) -> Option<Self> {
        parse_load_bits(bits)
    }

    #[must_use]
    pub(crate) fn into_library_arguments(self) -> Vec<LoadArgument> {
        match self {
            Self::FullLoad { libraries } => libraries,
            Self::SelectiveImport { library, .. } => vec![library],
        }
    }
}

/// A parsed `{% load %}` tag with its source span and load kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LoadStatement {
    span: Span,
    kind: LoadKind,
}

impl LoadStatement {
    #[must_use]
    pub(crate) fn new(span: Span, kind: LoadKind) -> Self {
        Self { span, kind }
    }

    #[must_use]
    pub(crate) fn from_loader_bits(bits: &[TagBit], span: Span) -> Option<Self> {
        Some(Self::new(span, LoadKind::from_loader_bits(bits)?))
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

/// Coordinates of one full or selective import in the ordered statement list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct IndexedLoadEvent {
    statement: usize,
    argument: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct LoadIndex {
    full_events: Vec<IndexedLoadEvent>,
    full_statements_by_library: BTreeMap<String, Vec<usize>>,
    selective_events_by_symbol: BTreeMap<String, Vec<IndexedLoadEvent>>,
    selective_statements_by_library_symbol: BTreeMap<String, BTreeMap<String, Vec<usize>>>,
}

impl LoadIndex {
    fn build(statements: &[LoadStatement]) -> Self {
        let mut index = Self::default();
        for (statement_index, statement) in statements.iter().enumerate() {
            match &statement.kind {
                LoadKind::FullLoad { libraries } => {
                    for (argument, library) in libraries.iter().enumerate() {
                        index.full_events.push(IndexedLoadEvent {
                            statement: statement_index,
                            argument,
                        });
                        index
                            .full_statements_by_library
                            .entry(library.as_str().to_string())
                            .or_default()
                            .push(statement_index);
                    }
                }
                LoadKind::SelectiveImport { symbols, library } => {
                    for (argument, symbol) in symbols.iter().enumerate() {
                        let event = IndexedLoadEvent {
                            statement: statement_index,
                            argument,
                        };
                        index
                            .selective_events_by_symbol
                            .entry(symbol.as_str().to_string())
                            .or_default()
                            .push(event);
                        index
                            .selective_statements_by_library_symbol
                            .entry(library.as_str().to_string())
                            .or_default()
                            .entry(symbol.as_str().to_string())
                            .or_default()
                            .push(statement_index);
                    }
                }
            }
        }
        index
    }
}

/// A borrowed snapshot of the ordered load statements visible at one source position.
///
/// Creating a snapshot is allocation-free. Availability uses indexes owned by
/// [`LoadedLibraries`], while source-order results borrow names from the visible
/// statement prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoadState<'a> {
    loaded: &'a LoadedLibraries,
    statement_end: usize,
}

impl<'a> LoadState<'a> {
    #[must_use]
    pub(crate) fn is_symbol_available(&self, library: &str, symbol: &str) -> bool {
        has_statement_before(
            self.loaded.index.full_statements_by_library.get(library),
            self.statement_end,
        ) || has_statement_before(
            self.loaded
                .index
                .selective_statements_by_library_symbol
                .get(library)
                .and_then(|symbols| symbols.get(symbol)),
            self.statement_end,
        )
    }

    #[must_use]
    pub(crate) fn visible_statement_count(self) -> usize {
        self.statement_end
    }

    #[must_use]
    pub(crate) fn unknown_load_can_shadow_symbol(
        self,
        symbol: &str,
        environment: djls_project::TemplateEnvironment<'_>,
    ) -> bool {
        self.statements()
            .iter()
            .any(|statement| match &statement.kind {
                LoadKind::FullLoad { libraries } => libraries.iter().any(|library| {
                    matches!(
                        environment.loadable_library_str(library.as_str()),
                        LoadableLibraryLookup::Inconclusive(_)
                    )
                }),
                LoadKind::SelectiveImport { symbols, library } => {
                    matches!(
                        environment.loadable_library_str(library.as_str()),
                        LoadableLibraryLookup::Inconclusive(_)
                    ) && symbols.iter().any(|loaded| loaded.as_str() == symbol)
                }
            })
    }

    #[must_use]
    pub(crate) fn libraries_loading_symbol(&self, symbol: &str) -> Vec<&'a str> {
        let mut libraries = Vec::new();
        self.write_libraries_loading_symbol(symbol, &mut libraries);
        libraries
    }

    pub(crate) fn write_libraries_loading_symbol(
        &self,
        symbol: &str,
        libraries: &mut Vec<&'a str>,
    ) {
        libraries.clear();
        let full_end = self
            .loaded
            .index
            .full_events
            .partition_point(|event| event.statement < self.statement_end);
        let selective = self
            .loaded
            .index
            .selective_events_by_symbol
            .get(symbol)
            .map_or(&[][..], Vec::as_slice);
        let selective_end = selective.partition_point(|event| event.statement < self.statement_end);
        let mut full = self.loaded.index.full_events[..full_end].iter().peekable();
        let mut selective = selective[..selective_end].iter().peekable();
        libraries.reserve(full_end + selective_end);

        while full.peek().is_some() || selective.peek().is_some() {
            if selective.peek().is_none_or(|selective_event| {
                full.peek()
                    .is_some_and(|full_event| *full_event <= *selective_event)
            }) {
                let event = full.next().expect("a full event was selected");
                libraries.push(self.full_event_library(*event));
                continue;
            }

            let event = *selective.next().expect("a selective event was selected");
            let library = self.selective_event_library(event);
            if libraries.last().copied() != Some(library) {
                libraries.push(library);
            }
        }
    }

    fn full_event_library(self, event: IndexedLoadEvent) -> &'a str {
        let LoadKind::FullLoad { libraries } = &self.loaded.statements[event.statement].kind else {
            unreachable!("the full-load index must address a full load")
        };
        libraries[event.argument].as_str()
    }

    fn selective_event_library(self, event: IndexedLoadEvent) -> &'a str {
        let LoadKind::SelectiveImport { library, .. } =
            &self.loaded.statements[event.statement].kind
        else {
            unreachable!("the selective-load index must address a selective import")
        };
        library.as_str()
    }

    fn statements(self) -> &'a [LoadStatement] {
        &self.loaded.statements[..self.statement_end]
    }
}

fn has_statement_before(statements: Option<&Vec<usize>>, end: usize) -> bool {
    statements.is_some_and(|statements| statements.partition_point(|index| *index < end) > 0)
}

/// Advances through ordered load statements for source-ordered occurrence walks.
pub(crate) struct LoadCursor<'a> {
    loaded: &'a LoadedLibraries,
    end: usize,
    position: u32,
}

impl<'a> LoadCursor<'a> {
    #[must_use]
    pub(crate) fn advance_to(&mut self, position: u32) -> LoadState<'a> {
        debug_assert!(
            position >= self.position,
            "load cursor must advance in source order"
        );
        self.position = position;
        while self
            .loaded
            .statements
            .get(self.end)
            .is_some_and(|statement| statement.span.end() <= position)
        {
            self.end += 1;
        }
        LoadState {
            loaded: self.loaded,
            statement_end: self.end,
        }
    }
}

/// An ordered collection of `LoadStatement` values extracted from a template.
///
/// Supports querying what libraries/symbols are available at a given byte
/// position by filtering load statements whose span ends before the query
/// position and applying state-machine semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct LoadedLibraries {
    statements: Vec<LoadStatement>,
    index: LoadIndex,
}

impl LoadedLibraries {
    #[must_use]
    pub(crate) fn new(statements: Vec<LoadStatement>) -> Self {
        debug_assert!(
            statements
                .windows(2)
                .all(|pair| pair[0].span.end() <= pair[1].span.end()),
            "load statements must remain in source order"
        );
        let index = LoadIndex::build(&statements);
        Self { statements, index }
    }

    /// Borrow the ordered load-statement prefix visible at `position`.
    #[must_use]
    pub(crate) fn available_at(&self, position: u32) -> LoadState<'_> {
        LoadState {
            loaded: self,
            statement_end: self.statement_end_before(position),
        }
    }

    #[must_use]
    pub(crate) fn cursor(&self) -> LoadCursor<'_> {
        LoadCursor {
            loaded: self,
            end: 0,
            position: 0,
        }
    }

    fn statement_end_before(&self, position: u32) -> usize {
        self.statements
            .partition_point(|statement| statement.span.end() <= position)
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
        assert_eq!(state.visible_statement_count(), 0);
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
        assert!(state.is_symbol_available("i18n", "trans"));
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
        assert!(!state.is_symbol_available("i18n", "trans"));
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
        assert!(state.is_symbol_available("i18n", "trans"));
        assert!(!state.is_symbol_available("i18n", "blocktrans"));

        // After full load, every symbol in the library is available.
        let state = libs.available_at(80);
        assert!(state.is_symbol_available("i18n", "trans"));
        assert!(state.is_symbol_available("i18n", "blocktrans"));
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

        // After both, the full load keeps every symbol available and the
        // adjacent selective load does not duplicate its candidate.
        let state = libs.available_at(80);
        assert!(state.is_symbol_available("i18n", "blocktrans"));
        assert_eq!(state.libraries_loading_symbol("trans"), ["i18n"]);
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
        assert!(state.is_symbol_available("i18n", "trans"));
        assert!(state.is_symbol_available("i18n", "blocktrans"));
    }

    #[test]
    fn indexed_symbol_libraries_preserve_selective_reload_order() {
        let libs = LoadedLibraries::new(vec![
            make_load(
                (0, 5),
                LoadKind::FullLoad {
                    libraries: vec!["alpha".into()],
                },
            ),
            make_load(
                (10, 5),
                LoadKind::FullLoad {
                    libraries: vec!["beta".into()],
                },
            ),
            make_load(
                (20, 5),
                LoadKind::SelectiveImport {
                    symbols: vec!["shared".into()],
                    library: "alpha".into(),
                },
            ),
        ]);

        assert_eq!(
            libs.available_at(30).libraries_loading_symbol("shared"),
            ["alpha", "beta", "alpha"]
        );
    }

    #[test]
    fn indexed_symbol_libraries_preserve_load_order_and_full_load_semantics() {
        let libs = LoadedLibraries::new(vec![
            make_load(
                (0, 5),
                LoadKind::SelectiveImport {
                    symbols: vec!["shared".into()],
                    library: "alpha".into(),
                },
            ),
            make_load(
                (10, 5),
                LoadKind::FullLoad {
                    libraries: vec!["beta".into(), "alpha".into()],
                },
            ),
            make_load(
                (20, 5),
                LoadKind::SelectiveImport {
                    symbols: vec!["shared".into()],
                    library: "alpha".into(),
                },
            ),
        ]);

        assert_eq!(
            libs.available_at(30).libraries_loading_symbol("shared"),
            ["alpha", "beta", "alpha"]
        );
    }

    #[test]
    fn position_at_exact_span_end() {
        // Span: start=10, length=20, so end=30
        // Position exactly at 30 should include this load
        // (we use > not >=, so end == position means it is included)
        let libs = LoadedLibraries::new(vec![make_load(
            (10, 20),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // At exactly end position (30) — the load's span ends at 30,
        // and 30 > 30 is false, so it IS included
        let state = libs.available_at(30);
        assert!(state.is_symbol_available("i18n", "trans"));

        // Just before end (29) — 30 > 29 is true, so NOT included
        let state = libs.available_at(29);
        assert!(!state.is_symbol_available("i18n", "trans"));
    }

    #[test]
    fn cursor_indexes_a_large_ordered_stream_once() {
        let statements = (0..1_000)
            .map(|index| {
                make_load(
                    (index * 10, 5),
                    LoadKind::FullLoad {
                        libraries: vec![format!("library_{index}").into()],
                    },
                )
            })
            .collect();
        let loaded = LoadedLibraries::new(statements);
        let mut cursor = loaded.cursor();

        for index in 0..1_000 {
            let state = cursor.advance_to(index * 10 + 5);
            assert_eq!(state.visible_statement_count(), index as usize + 1);
            assert!(state.is_symbol_available(&format!("library_{index}"), "anything"));
        }
    }
}
